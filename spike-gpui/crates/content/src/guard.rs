//! Runs a render on a worker thread with a time budget and panic containment.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use crate::{limits, ContentError};

/// Timed-out jobs still running, counted separately so a runaway diagram cannot block maths.
pub(crate) static MATH_ABANDONED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MERMAID_ABANDONED: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn run<T: Send + 'static>(
    abandoned: &'static AtomicUsize,
    budget: Duration,
    job: impl FnOnce() -> Result<T, ContentError> + Send + 'static,
) -> Result<T, ContentError> {
    if abandoned.load(Ordering::SeqCst) >= limits::MAX_ABANDONED {
        return Err(ContentError::TooLarge("too many earlier renders are still running".into()));
    }
    let (tx, rx) = mpsc::channel();
    // 0 = running, 1 = finished, 2 = abandoned by the caller after the budget expired.
    let state = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let flag = state.clone();
    let spawned = std::thread::Builder::new()
        .name("mdv-content".into())
        .stack_size(limits::WORKER_STACK)
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or_else(|p| {
                let m = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "panic".into());
                Err(ContentError::Panic(m))
            });
            if flag.swap(1, Ordering::SeqCst) == 2 {
                abandoned.fetch_sub(1, Ordering::SeqCst);
            }
            let _ = tx.send(r);
        })
        .map_err(|e| ContentError::Panic(format!("could not start worker: {e}")))?;
    match rx.recv_timeout(budget) {
        Ok(r) => {
            let _ = spawned.join();
            r
        }
        Err(_) => {
            // The thread cannot be cancelled. Count it so a hostile document cannot pile them up.
            abandoned.fetch_add(1, Ordering::SeqCst);
            if state.compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                abandoned.fetch_sub(1, Ordering::SeqCst);
            }
            Err(ContentError::Timeout)
        }
    }
}
