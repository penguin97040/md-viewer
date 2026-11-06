//! Test switches read from the environment, all in one place. They exist only with the `dev-harness`
//! feature, for driving the app from a test or measuring it. A default build gets the answers below
//! and has no `MDV_` name in it: a document viewer shouldn't take instructions from its environment.

#[cfg(not(feature = "dev-harness"))]
mod switches {
    use gpui::Entity;
    use mdv_reader::ui::pane::ReaderView;
    use std::ffi::OsString;

    pub fn window_size() -> (f32, f32) {
        (1280.0, 800.0)
    }
    pub fn no_single_instance() -> bool {
        false
    }
    pub fn mac_layout() -> bool {
        false
    }
    pub fn perf() -> bool {
        false
    }
    pub fn timing() -> Option<OsString> {
        None
    }
    pub fn print_save_to() -> Option<OsString> {
        None
    }
    pub fn a11y_sample() -> Option<(usize, usize)> {
        None
    }
    pub fn tab_log() -> Option<OsString> {
        None
    }
    pub fn open_queue() -> Option<OsString> {
        None
    }
    pub fn tab_move_fault(_name: &str) -> bool {
        false
    }
    pub fn start_script(_reader: Entity<ReaderView>, _cx: &mut gpui::Context<crate::view::AppView>) {}
}

#[cfg(feature = "dev-harness")]
mod switches {
    use gpui::Entity;
    use mdv_reader::ui::pane::ReaderView;
    use std::env::var_os;
    use std::ffi::OsString;

    /// `MDV_SIZE=1000x700`: the window size at start-up.
    pub fn window_size() -> (f32, f32) {
        std::env::var("MDV_SIZE").ok().and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1280.0), b.parse().unwrap_or(800.0)))).unwrap_or((1280.0, 800.0))
    }
    /// Run alongside other instances instead of handing files to the first.
    pub fn no_single_instance() -> bool {
        var_os("MDV_NO_SINGLE_INSTANCE").is_some()
    }
    /// `MDV_PLATFORM=mac`: lay out the chrome as on macOS.
    pub fn mac_layout() -> bool {
        var_os("MDV_PLATFORM").is_some_and(|v| v == "mac")
    }
    /// Keep frame timings.
    pub fn perf() -> bool {
        var_os("MDV_PERF").is_some()
    }
    /// The file `timing::log` writes to.
    pub fn timing() -> Option<OsString> {
        var_os("MDV_TIMING")
    }
    /// Where to save a PDF instead of asking; `-` stands in for cancelling the dialog.
    pub fn print_save_to() -> Option<OsString> {
        var_os("MDV_PRINT_SAVE_TO")
    }
    /// `MDV_A11Y_DOC` paragraphs and `MDV_A11Y_OVERSCAN` for the accessibility sample document.
    pub fn a11y_sample() -> Option<(usize, usize)> {
        let paragraphs = std::env::var("MDV_A11Y_DOC").ok()?.parse().ok()?;
        let overscan = std::env::var("MDV_A11Y_OVERSCAN").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        Some((paragraphs, overscan))
    }
    /// The tab movement test log.
    pub fn tab_log() -> Option<OsString> {
        var_os("MDV_TAB_LOG")
    }
    /// Files Ctrl+O opens in turn, in place of the file dialog.
    pub fn open_queue() -> Option<OsString> {
        var_os("MDV_OPEN_QUEUE")
    }
    /// `MDV_FAULT`: whether a tab movement step should fail on purpose.
    pub fn tab_move_fault(name: &str) -> bool {
        std::env::var("MDV_FAULT").is_ok_and(|f| f.split(',').any(|x| x == name))
    }
    /// Starts the scripted test run (`MDV_SCRIPT`), if one was asked for.
    pub fn start_script(reader: Entity<ReaderView>, cx: &mut gpui::Context<crate::view::AppView>) {
        mdv_reader::ui::script::start(reader, cx);
    }
}

pub use switches::*;
