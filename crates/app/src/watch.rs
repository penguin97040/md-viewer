//! Live reload: reports when an open file changes on disk.
//!
//! Like 0.2.6 this looks at the path rather than at one file object, so an editor that saves by
//! writing a temporary file and renaming it over the original is seen as a change, and so is the
//! file going missing and coming back. A change is reported once the file has been quiet for 200 ms.
//! One thread polls every watched file by its modification time, size and (on Unix) inode; it
//! exists only while something is watched. Dropping a [`Watch`] ends it, so a closed tab leaves
//! nothing behind, and a tab that moves between windows takes its watch with it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

/// How long a file must stay unchanged before the change is reported (0.2.6's debounce).
pub const QUIET: Duration = Duration::from_millis(200);
const POLL: Duration = Duration::from_millis(100);

/// What is compared to see that a file changed. `None` is a missing file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Signature(Option<(Option<SystemTime>, u64, u64)>);

impl Signature {
    pub fn of(path: &std::path::Path) -> Self {
        let Ok(m) = std::fs::metadata(path) else { return Signature(None) };
        #[cfg(unix)]
        let id = std::os::unix::fs::MetadataExt::ino(&m);
        #[cfg(not(unix))]
        let id = 0;
        Signature(Some((m.modified().ok(), m.len(), id)))
    }
}

/// The debounce for one file, kept apart from the clock and the disk so it can be tested.
#[derive(Debug)]
pub struct Debounce {
    seen: Signature,
    changed_at: Option<Instant>,
}

impl Debounce {
    pub fn new(seen: Signature) -> Self {
        Self { seen, changed_at: None }
    }

    /// Feeds one look at the file and returns true when a change has settled and is to be reported.
    pub fn step(&mut self, now_sig: Signature, now: Instant) -> bool {
        if now_sig != self.seen {
            self.seen = now_sig;
            self.changed_at = Some(now);
            return false;
        }
        match self.changed_at {
            Some(at) if now.duration_since(at) >= QUIET => {
                self.changed_at = None;
                true
            }
            _ => false,
        }
    }
}

struct Entry {
    path: PathBuf,
    debounce: Debounce,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Hub {
    entries: HashMap<u64, Entry>,
    next: u64,
    running: bool,
}

static HUB: Mutex<Option<Hub>> = Mutex::new(None);

fn hub() -> MutexGuard<'static, Option<Hub>> {
    HUB.lock().unwrap_or_else(|e| e.into_inner())
}

/// A file being watched. Dropping it stops the watch.
pub struct Watch {
    id: u64,
}

impl Watch {
    /// Starts watching `path`. `on_change` runs on the polling thread, so it should only hand the
    /// news to the UI thread.
    pub fn new(path: PathBuf, on_change: impl Fn() + Send + Sync + 'static) -> Watch {
        let seen = Signature::of(&path);
        let mut guard = hub();
        let h = guard.get_or_insert_with(Hub::default);
        h.next += 1;
        let id = h.next;
        h.entries.insert(id, Entry { path, debounce: Debounce::new(seen), on_change: Arc::new(on_change) });
        if !h.running {
            if std::thread::Builder::new().name("live-reload".into()).spawn(poll).is_ok() {
                h.running = true;
            }
        }
        Watch { id }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(h) = hub().as_mut() {
            h.entries.remove(&self.id);
        }
    }
}

/// How many files are being watched.
#[cfg(test)]
pub fn active() -> usize {
    hub().as_ref().map_or(0, |h| h.entries.len())
}

fn poll() {
    loop {
        std::thread::sleep(POLL);
        let paths: Vec<(u64, PathBuf)> = {
            let mut guard = hub();
            let Some(h) = guard.as_mut() else { return };
            if h.entries.is_empty() {
                h.running = false;
                return;
            }
            h.entries.iter().map(|(id, e)| (*id, e.path.clone())).collect()
        };
        // Looking at the disk happens outside the lock: a slow network share must not stall the UI.
        let looked: Vec<(u64, Signature)> = paths.into_iter().map(|(id, p)| (id, Signature::of(&p))).collect();
        let now = Instant::now();
        let mut fire = Vec::new();
        {
            let mut guard = hub();
            let Some(h) = guard.as_mut() else { return };
            for (id, sig) in looked {
                if let Some(e) = h.entries.get_mut(&id) {
                    if e.debounce.step(sig, now) {
                        fire.push(e.on_change.clone());
                    }
                }
            }
        }
        for f in fire {
            // A panic here would end live reload for every tab.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(n: u64) -> Signature {
        Signature(Some((None, n, 0)))
    }

    #[test]
    fn reports_after_quiet() {
        let t0 = Instant::now();
        let mut d = Debounce::new(sig(1));
        assert!(!d.step(sig(1), t0));
        assert!(!d.step(sig(2), t0 + Duration::from_millis(100)));
        assert!(!d.step(sig(2), t0 + Duration::from_millis(250)));
        assert!(d.step(sig(2), t0 + Duration::from_millis(300)));
        assert!(!d.step(sig(2), t0 + Duration::from_millis(500)), "reported once");
    }

    #[test]
    fn a_burst_is_one_report() {
        let t0 = Instant::now();
        let mut d = Debounce::new(sig(1));
        let mut reports = 0;
        for i in 0..20u64 {
            reports += d.step(sig(2 + i % 3 + i), t0 + Duration::from_millis(50 * i)) as u32;
        }
        assert_eq!(reports, 0, "still changing");
        assert!(d.step(sig(1000), t0 + Duration::from_millis(2000)) == false);
        assert!(d.step(sig(1000), t0 + Duration::from_millis(2300)));
    }

    #[test]
    fn deletion_and_return_are_changes() {
        let t0 = Instant::now();
        let mut d = Debounce::new(sig(1));
        assert!(!d.step(Signature(None), t0));
        assert!(d.step(Signature(None), t0 + QUIET));
        assert!(!d.step(sig(1), t0 + QUIET * 2));
        assert!(d.step(sig(1), t0 + QUIET * 3));
    }

    #[test]
    fn sees_a_rename_over_the_file_and_stops_on_drop() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = std::env::temp_dir().join(format!("mdv-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.md");
        std::fs::write(&file, "one").unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let w = Watch::new(file.clone(), move || {
            h.fetch_add(1, Ordering::SeqCst);
        });
        assert!(active() >= 1);
        let wait = |n: usize| {
            for _ in 0..40 {
                if hits.load(Ordering::SeqCst) >= n {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            false
        };
        // An atomic save: write beside it, then rename over it (same size, so only the file differs).
        let tmp = dir.join("a.md.tmp");
        std::fs::write(&tmp, "two").unwrap();
        std::fs::rename(&tmp, &file).unwrap();
        assert!(wait(1), "rename over the file was not seen");
        std::fs::remove_file(&file).unwrap();
        assert!(wait(2), "deletion was not seen");
        drop(w);
        let before = hits.load(Ordering::SeqCst);
        std::fs::write(&file, "three, longer").unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(hits.load(Ordering::SeqCst), before, "a dropped watch still reported");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
