//! Linux and macOS: a Unix socket in a directory only this user can enter. Run on Linux under X11.
//! macOS has not been built or run.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::sanitise;

pub type Conn = UnixStream;

const CLIENT_TIMEOUT: Duration = Duration::from_secs(2);
/// A client waits this long for an answer, which covers the owner still starting up.
const ACK_TIMEOUT: Duration = Duration::from_secs(15);

fn base_dir() -> io::Result<PathBuf> {
    if cfg!(target_os = "macos") {
        if let Some(t) = std::env::var_os("TMPDIR") {
            return Ok(PathBuf::from(t));
        }
    }
    if let Some(r) = std::env::var_os("XDG_RUNTIME_DIR") {
        return Ok(PathBuf::from(r));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME"))?;
    Ok(PathBuf::from(home).join(".cache"))
}

/// The private directory, checked rather than trusted: not a link, ours, and closed to others.
fn private_dir() -> io::Result<PathBuf> {
    let dir = base_dir()?.join("md-viewer-gpui");
    let _ = DirBuilder::new().recursive(true).mode(0o700).create(&dir);
    let meta = fs::symlink_metadata(&dir)?;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.uid() != me || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "instance directory is not private"));
    }
    Ok(dir)
}

/// One owner per display, as a person can run several sessions as the same user. Windows on another
/// display would not be reachable, so a hand-off across them would lose the file.
fn session() -> String {
    let display = std::env::var("WAYLAND_DISPLAY").or_else(|_| std::env::var("DISPLAY")).unwrap_or_default();
    sanitise(&display)
}

pub fn lock_path() -> io::Result<PathBuf> {
    Ok(private_dir()?.join(format!("{}.lock", session())))
}

fn socket_path() -> io::Result<PathBuf> {
    let path = private_dir()?.join(format!("{}.sock", session()));
    if path.as_os_str().len() >= 100 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "socket path too long"));
    }
    Ok(path)
}

pub struct Listener(UnixListener);

impl Listener {
    /// Only called while holding the lock, so a socket file already here is left from a crash.
    pub fn bind() -> io::Result<Self> {
        let path = socket_path()?;
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(Self(listener))
    }

    pub fn accept(&self) -> io::Result<Conn> {
        let (conn, _) = self.0.accept()?;
        conn.set_read_timeout(Some(CLIENT_TIMEOUT))?;
        conn.set_write_timeout(Some(CLIENT_TIMEOUT))?;
        Ok(conn)
    }
}

pub fn connect() -> io::Result<Conn> {
    let conn = UnixStream::connect(socket_path()?)?;
    conn.set_read_timeout(Some(ACK_TIMEOUT))?;
    conn.set_write_timeout(Some(CLIENT_TIMEOUT))?;
    Ok(conn)
}

pub fn allow_foreground(_owner_pid: u32) {}

pub fn path_bytes(p: &Path) -> Vec<u8> {
    p.as_os_str().as_bytes().to_vec()
}

pub fn path_from_bytes(b: &[u8]) -> io::Result<PathBuf> {
    Ok(PathBuf::from(std::ffi::OsString::from_vec(b.to_vec())))
}

pub fn is_remote_path(_p: &Path) -> bool {
    false
}
