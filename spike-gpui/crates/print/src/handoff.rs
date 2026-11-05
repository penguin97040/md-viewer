//! Getting the PDF to the person. GPUI has no print API, so the file is written to a private
//! temporary folder and opened with the system's default PDF handler, where the person can print
//! or save it. Nothing from the document is ever part of a command line or a file name.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

const PREFIX: &str = "mdv-print-";

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

/// Writes the PDF to `<temp>/mdv-print-<random>/document.pdf`, in a folder only this user can
/// enter on Unix. Both are created new, so an existing path is never followed or overwritten.
pub fn write_temp(bytes: &[u8]) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("{PREFIX}{}", token()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir(&dir)?;
    let path = dir.join("document.pdf");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&path)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    Ok(path)
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

/// Removes print folders older than `max_age`. A handler may still be showing a recent file, so
/// the app calls this with a generous age at start-up and before each new print.
pub fn sweep(max_age: Duration) -> usize {
    let mut removed = 0;
    let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return 0 };
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(n) = name.to_str() else { continue };
        if !n.starts_with(PREFIX) || n.len() != PREFIX.len() + 32 || !n[PREFIX.len()..].bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|a| a > max_age);
        // Only a real folder of ours: never follow a link placed under that name.
        if old && e.file_type().is_ok_and(|t| t.is_dir() && !t.is_symlink()) && std::fs::remove_dir_all(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
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

    #[test]
    fn write_and_sweep() {
        let p = write_temp(b"%PDF-1.7\n").expect("write");
        assert!(p.ends_with("document.pdf"));
        assert!(p.exists());
        std::fs::remove_dir_all(p.parent().expect("dir")).expect("remove");
    }
}
