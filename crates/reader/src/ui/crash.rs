//! What happens when a bug is found, usually by a hostile document. One process hosts every window,
//! so a panic that reaches the top of the UI thread closes them all, and on Windows it would cross
//! the window procedure, which aborts. Document work on the UI thread therefore runs inside
//! [`contain`], which turns a panic into a failed document, and [`install_hook`] explains the rest.
//!
//! GPUI itself isn't panic safe: a panic that unwinds through `Entity::update` loses the entity, and
//! one through `with_content_mask` leaves its stack unbalanced for the rest of the window's life. So
//! these are caught inside those calls, never outside them. See `ReaderView::contained` and
//! `masked`. Stack overflow and allocation failure abort the process and can't be contained here at
//! all; those are prevented by limits on nesting and sizes instead.

use std::cell::Cell;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};

thread_local! {
    static CONTAINED: Cell<u32> = const { Cell::new(0) };
}

/// Runs `f`, returning `None` if it panics.
pub fn contain<T>(f: impl FnOnce() -> T) -> Option<T> {
    CONTAINED.with(|c| c.set(c.get() + 1));
    let r = catch_unwind(AssertUnwindSafe(f));
    CONTAINED.with(|c| c.set(c.get() - 1));
    r.ok()
}

/// `window.with_content_mask`, with `f`'s panic carried past GPUI's push and pop.
pub fn masked<T>(window: &mut gpui::Window, mask: gpui::ContentMask<gpui::Pixels>, f: impl FnOnce(&mut gpui::Window) -> T) -> T {
    match window.with_content_mask(Some(mask), |w| catch_unwind(AssertUnwindSafe(|| f(w)))) {
        Ok(v) => v,
        Err(p) => resume_unwind(p),
    }
}

/// True for a panic that will end the process: on the UI thread and outside [`contain`]. Other
/// threads' panics end only that thread, and their work reports failure.
fn fatal() -> bool {
    std::thread::current().name() == Some("main") && CONTAINED.with(|c| c.get()) == 0
}

/// The message for a panic that ends the process. Only the code location goes in it: a panic's own
/// text can quote the document. Nothing is written anywhere.
fn message(location: Option<&std::panic::Location<'_>>) -> String {
    let at = location.map_or(String::new(), |l| format!("\n\nFor a bug report: {}:{}", l.file(), l.line()));
    format!("MD Viewer hit an internal error and has to close. Your files have not been changed.{at}")
}

/// Keeps the default report on standard error, and adds `explain` for a panic that ends the process.
pub fn install_hook(explain: fn(&str)) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        if fatal() {
            explain(&message(info.location()));
        }
    }));
}

/// A test switch that panics at a named point on the UI thread.
#[cfg(feature = "dev-harness")]
pub fn fault(name: &str) {
    if std::env::var("MDV_FAULT").is_ok_and(|f| f.split(',').any(|x| x == name)) {
        panic!("injected fault: {name}");
    }
}

#[cfg(not(feature = "dev-harness"))]
#[inline(always)]
pub fn fault(_name: &str) {}

/// Whether a test run wants frame timings kept.
#[cfg(feature = "dev-harness")]
pub fn perf_requested() -> bool {
    std::env::var("MDV_PERF").is_ok()
}

#[cfg(not(feature = "dev-harness"))]
pub fn perf_requested() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contained_panic_is_only_a_none() {
        assert_eq!(contain(|| 7), Some(7));
        assert_eq!(contain(|| -> u32 { panic!("contained") }), None);
        // The count returns, so a later panic is treated as what it is.
        assert_eq!(CONTAINED.with(|c| c.get()), 0);
    }

    #[test]
    fn the_message_names_code_and_not_the_document() {
        let m = message(Some(std::panic::Location::caller()));
        assert!(m.contains("crash.rs:"));
        assert!(message(None).ends_with("changed."));
    }

    #[test]
    fn only_the_ui_thread_outside_containment_is_fatal() {
        // Test threads are named after the test, not "main".
        assert!(!fatal());
        let h = std::thread::Builder::new().name("main".into()).spawn(|| (fatal(), contain(fatal))).unwrap();
        assert_eq!(h.join().unwrap(), (true, Some(false)));
    }
}
