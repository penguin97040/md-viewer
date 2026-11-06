//! The reader: markdown document model, parser and on-screen view.

pub mod access;
pub mod autolink;
pub mod doc;
pub mod fold;
pub mod highlight;
pub mod html;
pub mod layout;
pub mod model;
pub mod parse;
pub mod paths;
pub mod slug;
pub mod sort;
pub mod split;
#[cfg(feature = "ui")]
pub mod ui;
#[cfg(test)]
mod hostile;

use std::sync::{Mutex, MutexGuard};

/// A panic on another thread must not take every later user of a lock with it. The data behind
/// these locks is caches, queues and results, each changed in a single step, so it stays usable.
pub fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
