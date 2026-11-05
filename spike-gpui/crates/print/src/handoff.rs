//! Getting the PDF to the person. GPUI has no print API, so the person picks a destination in the
//! system's Save As dialog, the PDF is written there, and the Open action on the toast hands it to
//! the system's default PDF handler, where it can be printed. The destination is the person's own
//! choice; nothing from the document is part of a command line, and the handler gets the path as
//! one argument, never through a shell.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

/// 128 bits from the process's random hash keys, the clock and the process id.
fn token() -> String {
    let mut out = String::new();
    for salt in 0u64..2 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u32(std::process::id());
        if let Ok(d) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
            h.write_u128(d.as_nanos());
        }
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

/// A hidden temporary name beside `dest`, so the final rename never crosses a file system.
fn temp_beside(dest: &Path) -> PathBuf {
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Keep the name short enough for FAT and old limits whatever the document is called.
    let stem: String = name.chars().take(40).collect();
    dest.with_file_name(format!(".{stem}.{}.tmp", &token()[..12]))
}

/// Checks early that a file can be created beside `dest`, so a read-only folder fails before the
/// PDF is built and not after. Leaves nothing behind.
pub fn probe(dest: &Path) -> std::io::Result<()> {
    let tmp = temp_beside(dest);
    let f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    drop(f);
    std::fs::remove_file(&tmp)
}

/// Writes `bytes` to `dest` by way of a temporary file in the same folder and a rename, so a
/// failure never leaves a partial file and never damages a file that is already there. The
/// temporary file is removed on every error.
pub fn write_atomic(bytes: &[u8], dest: &Path) -> std::io::Result<()> {
    let tmp = temp_beside(dest);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// What to tell the person when a save fails, in plain words.
pub fn explain(e: &std::io::Error, dest: &Path) -> String {
    // The folder's own name only: a full path would not fit in the toast.
    let folder = dest.parent().and_then(|p| p.file_name()).map(|n| format!("\"{}\"", n.to_string_lossy())).unwrap_or_else(|| "that folder".into());
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => format!("Could not save the PDF: the folder {folder} is read-only, or you cannot write to it"),
        std::io::ErrorKind::NotFound => format!("Could not save the PDF: the folder {folder} does not exist any more"),
        _ => format!("Could not save the PDF: {e}"),
    }
}

/// Opens a file with the system's default handler. The path is passed as one argument, never
/// through a shell.
pub fn open_default(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    let mut cmd = {
        // Compile-checked only. `explorer.exe <file>` opens the default handler for the type.
        let mut c = Command::new("explorer.exe");
        c.arg(path);
        c
    };
    #[cfg(target_os = "macos")]
    let mut cmd = {
        // Compile-checked only.
        let mut c = Command::new("/usr/bin/open");
        c.arg(path);
        c
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().map(|mut child| {
        // Reap it in the background so it does not linger as a zombie.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_names_are_hex_and_differ() {
        let (a, b) = (token(), token());
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mdv-handoff-test-{tag}-{}", token()));
        std::fs::create_dir_all(&d).expect("dir");
        d
    }

    #[test]
    fn writes_and_replaces_without_leftovers() {
        let d = scratch("w");
        let p = d.join("a.pdf");
        probe(&p).expect("probe");
        write_atomic(b"one", &p).expect("write");
        write_atomic(b"two", &p).expect("replace");
        assert_eq!(std::fs::read(&p).expect("read"), b"two");
        assert_eq!(std::fs::read_dir(&d).expect("ls").count(), 1, "no temporary file remains");
        std::fs::remove_dir_all(&d).expect("clean");
    }

    #[test]
    fn a_failed_write_keeps_the_old_file_and_leaves_no_temp() {
        let d = scratch("f");
        let p = d.join("a.pdf");
        write_atomic(b"keep", &p).expect("write");
        // The destination is a directory, so the rename fails after the temp file was written.
        let dir_dest = d.join("sub.pdf");
        std::fs::create_dir(&dir_dest).expect("dir");
        std::fs::write(dir_dest.join("x"), b"x").expect("x");
        assert!(write_atomic(b"new", &dir_dest).is_err());
        assert_eq!(std::fs::read(&p).expect("read"), b"keep");
        let names: Vec<_> = std::fs::read_dir(&d).expect("ls").flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
        // A folder that does not exist.
        assert!(probe(&d.join("missing").join("a.pdf")).is_err());
        std::fs::remove_dir_all(&d).expect("clean");
    }

    #[test]
    fn explains_in_plain_words() {
        let p = Path::new("/x/y/a.pdf");
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert!(explain(&e, p).contains("read-only"));
        assert!(explain(&std::io::Error::from(std::io::ErrorKind::NotFound), p).contains("does not exist"));
    }
}
