//! Runs a render on a worker thread with a time budget and panic containment.
//!
//! A Rust thread can't be stopped from outside, so a render that passes its budget keeps running,
//! and its result is thrown away when it ends. Those threads are what this module bounds. Each kind
//! of render has a hard cap on threads, and one document can only hold so many of them past their
//! budget, so a document full of slow diagrams can't use up the threads other documents need, and
//! its own diagrams keep being tried while there is room. At the cap, a render is refused with
//! `ContentError::Busy` straight away rather than queueing behind something that might never end.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use crate::{limits, ContentError};

/// Counts of render threads alive, for one kind of render.
pub(crate) struct Gate {
    max_threads: usize,
    max_stuck_per_owner: usize,
    state: Mutex<GateState>,
}

#[derive(Default)]
struct GateState {
    threads: usize,
    /// Threads past their budget, by the owner that started them.
    stuck: HashMap<u64, usize>,
}

impl Gate {
    pub(crate) fn new(max_threads: usize, max_stuck_per_owner: usize) -> Self {
        Self { max_threads, max_stuck_per_owner, state: Mutex::default() }
    }

    fn lock(&self) -> MutexGuard<'_, GateState> {
        // Every change is one step on plain counters, so a panic can't leave them half done.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn enter(&self, owner: u64) -> bool {
        let mut s = self.lock();
        if s.threads >= self.max_threads || s.stuck.get(&owner).copied().unwrap_or(0) >= self.max_stuck_per_owner {
            return false;
        }
        s.threads += 1;
        true
    }

    fn leave(&self) {
        self.lock().threads -= 1;
    }

    fn mark_stuck(&self, owner: u64) {
        *self.lock().stuck.entry(owner).or_insert(0) += 1;
    }

    fn clear_stuck(&self, owner: u64) {
        let mut s = self.lock();
        if let Some(n) = s.stuck.get_mut(&owner) {
            *n -= 1;
            if *n == 0 {
                s.stuck.remove(&owner);
            }
        }
    }
}

/// Maths is quick and diagrams slow, so a runaway diagram can't block maths.
pub(crate) static MATH: LazyLock<Gate> = LazyLock::new(|| Gate::new(limits::MAX_MATH_THREADS, limits::MAX_STUCK_PER_DOCUMENT));
pub(crate) static MERMAID: LazyLock<Gate> = LazyLock::new(|| Gate::new(limits::MAX_DIAGRAM_THREADS, limits::MAX_STUCK_PER_DOCUMENT));

thread_local! {
    static OWNER: Cell<u64> = const { Cell::new(0) };
}

/// A new identity for `set_render_owner`.
pub fn new_render_owner() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Says which document this thread's renders are for, so that their limits are counted against it.
pub fn set_render_owner(owner: u64) {
    OWNER.with(|o| o.set(owner));
}

pub(crate) fn run<T: Send + 'static>(
    gate: &'static Gate,
    budget: Duration,
    job: impl FnOnce() -> Result<T, ContentError> + Send + 'static,
) -> Result<T, ContentError> {
    let owner = OWNER.with(|o| o.get());
    if !gate.enter(owner) {
        return Err(ContentError::Busy);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    // 0 = running, 1 = finished, 2 = abandoned by the caller after the budget expired.
    let state = Arc::new(AtomicU8::new(0));
    let flag = state.clone();
    let spawned = std::thread::Builder::new().name("mdv-content".into()).stack_size(limits::WORKER_STACK).spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or_else(|p| {
            let m = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "panic".into());
            Err(ContentError::Panic(m))
        });
        if flag.swap(1, Ordering::SeqCst) == 2 {
            gate.clear_stuck(owner);
        }
        gate.leave();
        let _ = tx.send(r);
    });
    let spawned = match spawned {
        Ok(h) => h,
        Err(e) => {
            gate.leave();
            return Err(ContentError::Panic(format!("could not start worker: {e}")));
        }
    };
    match rx.recv_timeout(budget) {
        Ok(r) => {
            let _ = spawned.join();
            r
        }
        Err(_) => {
            // Marked before the state changes, so that the thread can't clear it first.
            gate.mark_stuck(owner);
            if state.compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                gate.clear_stuck(owner);
            }
            Err(ContentError::Timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, Sender};
    use std::time::Instant;

    const QUICK: Duration = Duration::from_millis(20);

    fn gate(max: usize, per_owner: usize) -> &'static Gate {
        Box::leak(Box::new(Gate::new(max, per_owner)))
    }

    /// A render that doesn't end until told to.
    fn stuck(gate: &'static Gate, owner: u64) -> (Result<(), ContentError>, Sender<()>) {
        set_render_owner(owner);
        let (tx, rx) = channel::<()>();
        (run(gate, QUICK, move || { let _ = rx.recv(); Ok(()) }), tx)
    }

    fn quick(gate: &'static Gate, owner: u64) -> Result<u32, ContentError> {
        set_render_owner(owner);
        run(gate, Duration::from_secs(5), || Ok(7))
    }

    fn wait_until_free(gate: &'static Gate, owner: u64) {
        let t = Instant::now();
        while quick(gate, owner).is_err() {
            assert!(t.elapsed() < Duration::from_secs(5), "the slot never came back");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn stuck_renders_do_not_starve_later_ones() {
        let g = gate(8, 6);
        let mut held = Vec::new();
        // Three used to block every later diagram for good.
        for _ in 0..3 {
            let (r, tx) = stuck(g, 1);
            assert_eq!(r, Err(ContentError::Timeout));
            held.push(tx);
        }
        assert_eq!(quick(g, 1), Ok(7));
        assert_eq!(quick(g, 2), Ok(7));
        drop(held);
    }

    #[test]
    fn one_document_cannot_take_every_thread() {
        let g = gate(8, 3);
        let mut held = Vec::new();
        for _ in 0..3 {
            held.push(stuck(g, 1).1);
        }
        assert_eq!(quick(g, 1), Err(ContentError::Busy), "this document is at its limit");
        assert_eq!(quick(g, 2), Ok(7), "another isn't held up");
        drop(held);
        wait_until_free(g, 1);
    }

    #[test]
    fn the_thread_cap_is_hard_and_slots_come_back() {
        let g = gate(4, 100);
        let mut held = Vec::new();
        for owner in 0..4 {
            let (r, tx) = stuck(g, owner);
            assert_eq!(r, Err(ContentError::Timeout));
            held.push(tx);
        }
        assert_eq!(g.lock().threads, 4);
        assert_eq!(quick(g, 9), Err(ContentError::Busy));
        assert_eq!(g.lock().threads, 4, "a refused render starts nothing");
        drop(held);
        wait_until_free(g, 9);
        assert!(g.lock().stuck.is_empty(), "nothing still counted as stuck");
    }

    #[test]
    fn a_panic_gives_its_thread_back() {
        let g = gate(1, 1);
        set_render_owner(1);
        let r: Result<(), _> = run(g, Duration::from_secs(5), || panic!("contained"));
        assert_eq!(r, Err(ContentError::Panic("contained".into())));
        assert_eq!(quick(g, 1), Ok(7));
    }
}
