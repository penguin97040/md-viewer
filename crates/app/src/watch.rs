//! Live reload: reports when an open file changes on disk.
//!
//! Like 0.2.6 this looks at the path rather than at one file object, so an editor that saves by
//! writing a temporary file and renaming it over the original is seen as a change, and so is the
//! file going missing and coming back. The operating system reports changes in the file's folder
//! (one watch per folder, shared by the files in it and released with the last of them), events
//! for other files in the folder are dropped by name without touching the disk, and a report is
//! made once the file has been quiet for 200 ms and its [`Signature`] really differs.
//!
//! One worker thread sleeps until an event arrives or a debounce is due to end; with nothing
//! happening it never wakes. If a folder cannot be watched (some network drives, the inotify
//! limit) the files in it are checked every 2 s instead. Dropping a [`Watch`] releases the folder
//! watch, and the thread and the watcher go when the last watch does.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use notify::{EventKind, RecursiveMode, Watcher};

/// How long a file must stay unchanged before the change is reported (0.2.6's debounce).
pub const QUIET: Duration = Duration::from_millis(200);
/// How often a file whose folder cannot be watched is looked at.
const SLOW_POLL: Duration = Duration::from_secs(2);

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

    /// Notes that the operating system reported the file being replaced (renamed over, deleted or
    /// created), which is a change even if the [`Signature`] still looks the same: on Windows the
    /// file has no identity in it, and two writes in the same clock tick have equal times.
    pub fn replaced(&mut self, now: Instant) {
        if self.changed_at.is_none() {
            self.changed_at = Some(now);
        }
    }

    /// When the file next needs looking at to end a pending change, if one is pending.
    pub fn due(&self) -> Option<Instant> {
        self.changed_at.map(|at| at + QUIET)
    }
}

struct Entry {
    path: PathBuf,
    /// The folder watched for this file, or None when it is polled instead.
    dir: Option<PathBuf>,
    next_poll: Option<Instant>,
    debounce: Debounce,
    on_change: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Hub {
    entries: HashMap<u64, Entry>,
    next: u64,
    running: bool,
    watcher: Option<notify::RecommendedWatcher>,
    /// Watched folders and how many files use each.
    dirs: HashMap<PathBuf, usize>,
}

/// Held only briefly and never while calling into the watcher, so the watcher's own thread can
/// always deliver events (some platforms make `watch` wait for that thread).
#[derive(Default)]
struct Inbox {
    /// Every path being watched. Events for any other path are dropped on arrival.
    wanted: HashSet<PathBuf>,
    hit: HashSet<PathBuf>,
    /// Paths among those in `hit` that were reported created, removed or renamed.
    replaced: HashSet<PathBuf>,
    all: bool,
    kick: bool,
}

static HUB: Mutex<Option<Hub>> = Mutex::new(None);
static INBOX: Mutex<Option<Inbox>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();

fn hub() -> MutexGuard<'static, Option<Hub>> {
    HUB.lock().unwrap_or_else(|e| e.into_inner())
}

fn inbox() -> MutexGuard<'static, Option<Inbox>> {
    INBOX.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs on the watcher's thread. Cheap: a set lookup per path, no disk access.
fn on_event(res: notify::Result<notify::Event>) {
    let mut guard = inbox();
    let Some(ib) = guard.as_mut() else { return };
    match res {
        Ok(ev) => {
            if matches!(ev.kind, EventKind::Access(_)) {
                return;
            }
            if ev.need_rescan() {
                ib.all = true;
            }
            let mut any = ib.all;
            // FSEvents repeats a file's recent history in later events, so there a create or
            // rename flag proves nothing and the file's identity is compared instead.
            let replaced = !cfg!(target_os = "macos")
                && matches!(ev.kind, EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_)));
            for p in &ev.paths {
                if ib.wanted.contains(p) {
                    ib.hit.insert(p.clone());
                    if replaced {
                        ib.replaced.insert(p.clone());
                    }
                    any = true;
                }
            }
            if !any {
                return;
            }
        }
        // The watcher lost track of something: look at everything once.
        Err(_) => ib.all = true,
    }
    ib.kick = true;
    WAKE.notify_one();
}

/// Asks the worker to look at every file once. Needed after the folders being watched change:
/// some platforms (FSEvents) restart their stream for that, and anything that happened to the
/// other files during the restart is never reported.
fn kick_all() {
    if let Some(ib) = inbox().as_mut() {
        ib.all = true;
    }
    kick();
}

fn kick() {
    if let Some(ib) = inbox().as_mut() {
        ib.kick = true;
    }
    WAKE.notify_one();
}

/// A file being watched. Dropping it stops the watch.
pub struct Watch {
    id: u64,
}

impl Watch {
    /// Starts watching `path`. `on_change` runs on the worker thread, so it should only hand the
    /// news to the UI thread.
    pub fn new(path: PathBuf, on_change: impl Fn() + Send + Sync + 'static) -> Watch {
        Self::start(path, Arc::new(on_change), true)
    }

    fn start(path: PathBuf, on_change: Arc<dyn Fn() + Send + Sync>, events: bool) -> Watch {
        // Folder and name are kept apart: the folder is made canonical so that it matches the
        // paths the platform reports, and the file itself may not exist yet.
        let (dir, key) = match (path.parent(), path.file_name()) {
            (Some(d), Some(n)) if events => match dunce::canonicalize(if d.as_os_str().is_empty() { Path::new(".") } else { d }) {
                Ok(d) => (Some(d.clone()), d.join(n)),
                Err(_) => (None, path.clone()),
            },
            _ => (None, path.clone()),
        };
        let mut guard = hub();
        let h = guard.get_or_insert_with(Hub::default);
        // Order matters: watch first, then look, so a change in between is not lost.
        let mut watched = None;
        if let Some(d) = dir {
            if let Some(n) = h.dirs.get_mut(&d) {
                *n += 1;
                watched = Some(d);
            } else {
                if h.watcher.is_none() {
                    h.watcher = notify::recommended_watcher(on_event).ok();
                }
                if let Some(w) = h.watcher.as_mut() {
                    if w.watch(&d, RecursiveMode::NonRecursive).is_ok() {
                        h.dirs.insert(d.clone(), 1);
                        watched = Some(d);
                    }
                }
            }
        }
        let now = Instant::now();
        h.next += 1;
        let id = h.next;
        let next_poll = watched.is_none().then(|| now + SLOW_POLL);
        inbox().get_or_insert_with(Inbox::default).wanted.insert(key.clone());
        let seen = Signature::of(&key);
        h.entries.insert(id, Entry { path: key, dir: watched, next_poll, debounce: Debounce::new(seen), on_change });
        if !h.running && std::thread::Builder::new().name("live-reload".into()).spawn(work).is_ok() {
            h.running = true;
        }
        drop(guard);
        kick_all();
        Watch { id }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut guard = hub();
        let Some(h) = guard.as_mut() else { return };
        let Some(e) = h.entries.remove(&self.id) else { return };
        if !h.entries.values().any(|o| o.path == e.path) {
            if let Some(ib) = inbox().as_mut() {
                ib.wanted.remove(&e.path);
                ib.hit.remove(&e.path);
                ib.replaced.remove(&e.path);
            }
        }
        if let Some(d) = e.dir {
            if let Some(n) = h.dirs.get_mut(&d) {
                *n -= 1;
                if *n == 0 {
                    h.dirs.remove(&d);
                    if let Some(w) = h.watcher.as_mut() {
                        let _ = w.unwatch(&d);
                    }
                }
            }
        }
        if h.entries.is_empty() {
            // Releases the watcher's thread and descriptors.
            h.watcher = None;
        }
        drop(guard);
        kick_all();
    }
}

/// How many files are being watched.
#[cfg(test)]
pub fn active() -> usize {
    hub().as_ref().map_or(0, |h| h.entries.len())
}

/// Whether the folder holding `file` currently has an operating system watch.
#[cfg(test)]
fn folder_watched(file: &Path) -> bool {
    let d = dunce::canonicalize(file.parent().unwrap()).unwrap();
    hub().as_ref().is_some_and(|h| h.dirs.contains_key(&d))
}

/// The worker: sleeps until an event or the end of a debounce or slow poll, looks only at the
/// files concerned, and reports the ones that have settled.
fn work() {
    loop {
        let (hit, replaced, all) = {
            let mut guard = inbox();
            let Some(ib) = guard.as_mut() else { return };
            ib.kick = false;
            (std::mem::take(&mut ib.hit), std::mem::take(&mut ib.replaced), std::mem::take(&mut ib.all))
        };
        let now = Instant::now();
        let to_look: Vec<(u64, PathBuf, bool)> = {
            let mut guard = hub();
            let Some(h) = guard.as_mut() else { return };
            if h.entries.is_empty() {
                h.running = false;
                return;
            }
            h.entries
                .iter_mut()
                .filter_map(|(id, e)| {
                    let polled = e.next_poll.is_some_and(|t| t <= now);
                    if polled {
                        e.next_poll = Some(now + SLOW_POLL);
                    }
                    let due = e.debounce.due().is_some_and(|t| t <= now);
                    (all || polled || due || hit.contains(&e.path)).then(|| (*id, e.path.clone(), replaced.contains(&e.path)))
                })
                .collect()
        };
        // Looking at the disk happens outside the lock: a slow network share must not stall the UI.
        let looked: Vec<(u64, Signature, bool)> = to_look.into_iter().map(|(id, p, rep)| (id, Signature::of(&p), rep)).collect();
        let now = Instant::now();
        let mut fire = Vec::new();
        let wake_at = {
            let mut guard = hub();
            let Some(h) = guard.as_mut() else { return };
            for (id, sig, rep) in looked {
                if let Some(e) = h.entries.get_mut(&id) {
                    if rep {
                        e.debounce.replaced(now);
                    }
                    if e.debounce.step(sig, now) {
                        fire.push(e.on_change.clone());
                    }
                }
            }
            h.entries.values().flat_map(|e| [e.debounce.due(), e.next_poll]).flatten().min()
        };
        for f in fire {
            // A panic here would end live reload for every tab.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f()));
        }
        let mut guard = inbox();
        let Some(ib) = guard.as_mut() else { return };
        if ib.kick {
            continue;
        }
        match wake_at {
            Some(t) => {
                let wait = t.saturating_duration_since(Instant::now());
                drop(WAKE.wait_timeout(guard, wait));
            }
            None => {
                drop(WAKE.wait(guard));
            }
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
    fn a_replacement_with_an_identical_signature_is_still_a_change() {
        let t0 = Instant::now();
        let mut d = Debounce::new(sig(3));
        d.replaced(t0);
        assert!(!d.step(sig(3), t0));
        assert!(d.step(sig(3), t0 + QUIET));
        assert!(!d.step(sig(3), t0 + QUIET * 2), "reported once");
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

    fn counter(dir: &Path, name: &str, events: bool) -> (Watch, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let w = Watch::start(dir.join(name), Arc::new(move || {
            h.fetch_add(1, Ordering::SeqCst);
        }), events);
        (w, hits)
    }

    fn wait_for(hits: &std::sync::atomic::AtomicUsize, n: usize, ms: u64) -> bool {
        for _ in 0..ms / 25 {
            if hits.load(std::sync::atomic::Ordering::SeqCst) >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mdv-watch-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn in_place_write_delete_and_recreate_are_seen() {
        let dir = scratch("inplace");
        let file = dir.join("a.md");
        std::fs::write(&file, "one").unwrap();
        let (w, hits) = counter(&dir, "a.md", true);
        std::fs::write(&file, "two, longer").unwrap();
        assert!(wait_for(&hits, 1, 2000), "in-place write not seen");
        std::fs::remove_file(&file).unwrap();
        assert!(wait_for(&hits, 2, 2000), "delete not seen");
        std::fs::write(&file, "back").unwrap();
        assert!(wait_for(&hits, 3, 2000), "recreate not seen");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The temporary folder on macOS is reached through a symlink (`/var` is `/private/var`).
    #[cfg(unix)]
    #[test]
    fn a_file_reached_through_a_symlinked_folder_is_seen() {
        let real = scratch("real");
        let link = std::env::temp_dir().join(format!("mdv-watch-link-{}", std::process::id()));
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        std::fs::write(real.join("a.md"), "one").unwrap();
        let (w, hits) = counter(&link, "a.md", true);
        std::fs::write(link.join("a.md"), "two, longer").unwrap();
        assert!(wait_for(&hits, 1, 3000), "change through a symlinked folder not seen");
        drop(w);
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&real);
    }

    #[test]
    fn other_files_in_the_folder_are_ignored_and_the_folder_is_released() {
        let dir = scratch("others");
        let file = dir.join("a.md");
        std::fs::write(&file, "one").unwrap();
        let (w1, hits) = counter(&dir, "a.md", true);
        let (w2, _) = counter(&dir, "b.md", true);
        assert!(folder_watched(&file));
        for i in 0..20 {
            std::fs::write(dir.join(format!("noise{i}.txt")), "x").unwrap();
        }
        std::fs::write(dir.join("b.md"), "changed").unwrap();
        assert!(!wait_for(&hits, 1, 600), "a neighbour's change was reported");
        drop(w1);
        assert!(folder_watched(&file), "shared folder released too early");
        drop(w2);
        assert!(!folder_watched(&file), "folder watch outlived its files");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_touch_that_changes_nothing_is_not_a_reload() {
        let dir = scratch("touch");
        let file = dir.join("a.md");
        std::fs::write(&file, "one").unwrap();
        let (w, hits) = counter(&dir, "a.md", true);
        std::thread::sleep(Duration::from_millis(100));
        // An open and a permissions write are events but not changes.
        let _ = std::fs::File::open(&file).unwrap();
        let p = std::fs::metadata(&file).unwrap().permissions();
        std::fs::set_permissions(&file, p).unwrap();
        assert!(!wait_for(&hits, 1, 700), "a no-op event caused a reload");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_slow_poll_catches_changes_when_events_are_unavailable() {
        let dir = scratch("poll");
        let file = dir.join("a.md");
        std::fs::write(&file, "one").unwrap();
        let (w, hits) = counter(&dir, "a.md", false);
        std::fs::write(&file, "two, longer").unwrap();
        assert!(wait_for(&hits, 1, 4000), "polled change not seen");
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
