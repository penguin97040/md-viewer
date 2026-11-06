//! One process per user session. The first launch holds an OS lock and owns a private local endpoint
//! (a Unix socket, or a named pipe on Windows). Later launches send it their paths and exit.
//!
//! The lock decides who owns the session, not the endpoint: the OS releases it when a process dies,
//! so a crash leaves nothing to clean up except a socket file, which the next owner replaces. A launch
//! exits only after the owner acknowledges, so a path cannot vanish with an owner that is on its way
//! out. If the lock or endpoint cannot be used at all, the caller runs standalone rather than losing
//! the file.
//!
//! Everything received is untrusted: frames are bounded and each path is checked before use.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as os;

const MAGIC: &[u8; 4] = b"MDV1";
pub const MAX_BODY: usize = 64 * 1024;
pub const MAX_PATHS: usize = 256;
const MAX_CONNECTIONS: usize = 32;
/// How long a launch keeps trying. Generous, as the owner may still be starting up.
const LAUNCH_DEADLINE: Duration = Duration::from_secs(30);
const ATTEMPT_LIMIT: Duration = Duration::from_secs(20);
const ACK: u8 = 0x06;

pub enum Launch {
    Owner(Owner),
    HandedOff,
}

pub struct Owner {
    // Held for the life of the process: the OS drops it however the process ends.
    _lock: File,
    listener: os::Listener,
}

/// Paths from another launch. Call `acknowledge` once they have been dealt with.
pub struct Request {
    pub paths: Vec<PathBuf>,
    reply: Box<dyn FnOnce() + Send>,
}

impl Request {
    pub fn acknowledge(self) {
        (self.reply)();
    }
}

/// Becomes the owner, or passes `paths` to the owner. An error means run standalone.
pub fn start(paths: &[PathBuf]) -> io::Result<Launch> {
    let body = encode_paths(&absolute_paths(paths));
    let lock_path = os::lock_path()?;
    let deadline = Instant::now() + LAUNCH_DEADLINE;
    let mut last_error = None;
    while Instant::now() < deadline {
        let lock = OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path)?;
        match lock.try_lock() {
            Ok(()) => {
                let listener = os::Listener::bind()?;
                return Ok(Launch::Owner(Owner { _lock: lock, listener }));
            }
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(e)) => return Err(e),
        }
        drop(lock);
        // Someone owns the session, though it may not be listening yet.
        // On a thread, as a Windows pipe read has no timeout and a hung owner must not hang a launch.
        let (tx, rx) = mpsc::channel();
        let attempt = body.clone();
        std::thread::spawn(move || {
            let _ = tx.send(hand_off(&attempt));
        });
        match rx.recv_timeout(ATTEMPT_LIMIT) {
            Ok(Ok(())) => return Ok(Launch::HandedOff),
            Ok(Err(e)) => last_error = Some(e),
            Err(_) => last_error = Some(io::Error::new(io::ErrorKind::TimedOut, "the owner did not answer")),
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Err(last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "no owner answered")))
}

fn hand_off(body: &[u8]) -> io::Result<()> {
    let mut conn = os::connect()?;
    let mut hello = [0u8; 8];
    conn.read_exact(&mut hello)?;
    if &hello[..4] != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not the owner"));
    }
    os::allow_foreground(u32::from_le_bytes([hello[4], hello[5], hello[6], hello[7]]));
    let mut frame = Vec::with_capacity(body.len() + 8);
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
    frame.extend_from_slice(body);
    conn.write_all(&frame)?;
    conn.flush()?;
    let mut ack = [0u8; 1];
    conn.read_exact(&mut ack)?;
    if ack[0] == ACK {
        Ok(())
    } else {
        Err(io::Error::new(io::ErrorKind::InvalidData, "bad acknowledgement"))
    }
}

impl Owner {
    /// Listens on a thread of its own and calls `on_request` for each valid message. A bad or
    /// hung client costs only its own thread, and there is a bound on how many can exist.
    pub fn serve(self, on_request: impl Fn(Request) + Send + Sync + 'static) {
        let Owner { _lock, listener } = self;
        let on_request = Arc::new(on_request);
        let live = Arc::new(AtomicUsize::new(0));
        let _ = std::thread::Builder::new().name("launch-listener".into()).spawn(move || {
            let _lock = _lock;
            loop {
                let Ok(conn) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                };
                if live.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                    live.fetch_sub(1, Ordering::SeqCst);
                    continue;
                }
                let (on_request, live) = (on_request.clone(), live.clone());
                let _ = std::thread::Builder::new().name("launch-client".into()).spawn(move || {
                    handle_client(conn, &*on_request);
                    live.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
    }
}

fn handle_client(mut conn: os::Conn, on_request: &(impl Fn(Request) + ?Sized)) {
    let mut hello = [0u8; 8];
    hello[..4].copy_from_slice(MAGIC);
    hello[4..].copy_from_slice(&std::process::id().to_le_bytes());
    if conn.write_all(&hello).is_err() {
        return;
    }
    let Ok(paths) = read_frame(&mut conn) else { return };
    let (done_tx, done_rx) = mpsc::channel();
    on_request(Request { paths, reply: Box::new(move || {
        let _ = done_tx.send(());
    }) });
    // Dropping the connection unanswered tells the client to retry with a fresh owner.
    if done_rx.recv_timeout(LAUNCH_DEADLINE).is_ok() {
        let _ = conn.write_all(&[ACK]);
    }
}

fn read_frame(r: &mut impl Read) -> io::Result<Vec<PathBuf>> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let len = check_header(&head)?;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    decode_paths(&body)
}

fn invalid(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

fn check_header(head: &[u8; 8]) -> io::Result<usize> {
    if &head[..4] != MAGIC {
        return Err(invalid("bad magic"));
    }
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > MAX_BODY {
        return Err(invalid("frame too large"));
    }
    Ok(len)
}

fn absolute_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter_map(|p| std::path::absolute(p).ok()).collect()
}

/// NUL-separated, as a path cannot contain NUL. Paths past a limit are left for the caller's own
/// launch to report: dropping them is better than an owner refusing the whole message.
fn encode_paths(paths: &[PathBuf]) -> Vec<u8> {
    let mut body = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let bytes = os::path_bytes(p);
        if i >= MAX_PATHS || body.len() + bytes.len() + 1 > MAX_BODY {
            eprintln!("md-viewer: too many paths for one launch; {} not sent", paths.len() - i);
            break;
        }
        if i > 0 {
            body.push(0);
        }
        body.extend_from_slice(&bytes);
    }
    body
}

fn decode_paths(body: &[u8]) -> io::Result<Vec<PathBuf>> {
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let parts: Vec<&[u8]> = body.split(|b| *b == 0).collect();
    if parts.len() > MAX_PATHS {
        return Err(invalid("too many paths"));
    }
    parts.into_iter().map(|p| if p.is_empty() { Err(invalid("empty path")) } else { os::path_from_bytes(p) }).collect()
}

/// Checks a path received from another process and returns its canonical form. It must be absolute
/// and name a regular file. On Windows, UNC and device paths are refused: opening one would make this
/// process authenticate to whatever server the sender named.
pub fn validate_path(path: &Path) -> Result<PathBuf, &'static str> {
    if !path.is_absolute() {
        return Err("not an absolute path");
    }
    if os::is_remote_path(path) {
        return Err("network and device paths are not accepted");
    }
    let canonical = dunce::canonicalize(path).map_err(|_| "cannot be read")?;
    let meta = std::fs::metadata(&canonical).map_err(|_| "cannot be read")?;
    if !meta.is_file() {
        return Err("not a regular file");
    }
    Ok(canonical)
}

/// Letters and digits only, for the session part of a file or pipe name.
fn sanitise(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).take(40).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(body: &[u8]) -> Vec<u8> {
        let mut f = MAGIC.to_vec();
        f.extend_from_slice(&(body.len() as u32).to_le_bytes());
        f.extend_from_slice(body);
        f
    }

    #[test]
    fn paths_round_trip() {
        let paths = vec![PathBuf::from("/a/b c.md"), PathBuf::from("/d/é.md")];
        let body = encode_paths(&paths);
        assert_eq!(read_frame(&mut frame(&body).as_slice()).unwrap(), paths);
    }

    #[test]
    fn no_paths_is_a_valid_message() {
        assert!(read_frame(&mut frame(&[]).as_slice()).unwrap().is_empty());
    }

    #[test]
    fn bad_frames_are_refused() {
        assert!(read_frame(&mut &b"XXXX\0\0\0\0"[..]).is_err());
        let mut huge = MAGIC.to_vec();
        huge.extend_from_slice(&((MAX_BODY + 1) as u32).to_le_bytes());
        assert!(read_frame(&mut huge.as_slice()).is_err(), "length over the limit");
        let mut short = MAGIC.to_vec();
        short.extend_from_slice(&10u32.to_le_bytes());
        short.extend_from_slice(b"abc");
        assert!(read_frame(&mut short.as_slice()).is_err(), "body shorter than its length");
        assert!(read_frame(&mut frame(b"/a\0\0/b").as_slice()).is_err(), "empty path");
        let many = vec![0x2fu8, 0].repeat(MAX_PATHS + 1);
        assert!(read_frame(&mut frame(&many[..many.len() - 1]).as_slice()).is_err(), "too many paths");
    }

    #[test]
    fn encoding_stops_at_the_limits() {
        let many: Vec<PathBuf> = (0..MAX_PATHS + 50).map(|i| PathBuf::from(format!("/p{i}"))).collect();
        assert_eq!(decode_paths(&encode_paths(&many)).unwrap().len(), MAX_PATHS);
        let long = vec![PathBuf::from(format!("/{}", "x".repeat(MAX_BODY))), PathBuf::from("/next")];
        assert!(encode_paths(&long).is_empty());
    }

    #[test]
    fn validation_wants_an_absolute_regular_file() {
        let dir = std::env::temp_dir().join(format!("mdv-validate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.md");
        std::fs::write(&file, "x").unwrap();
        assert!(validate_path(&file).is_ok());
        assert_eq!(validate_path(Path::new("a.md")), Err("not an absolute path"));
        assert_eq!(validate_path(&dir), Err("not a regular file"));
        assert_eq!(validate_path(&dir.join("missing.md")), Err("cannot be read"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn session_names_are_safe() {
        assert_eq!(sanitise(":0.1/../x"), "_0_1____x");
    }
}
