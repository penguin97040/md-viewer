//! Print and Ctrl+P: renders the active tab to a PDF off the UI thread, shows the progress toast
//! with Cancel, then opens the file in the system's PDF viewer. See `mdv_print` for the engine
//! and for why this is a hand-off and not a print dialog.

use std::time::Duration;

use gpui::{Context, Entity};
use mdv_print::{PrintJob, PrintOptions, PrintState};
use mdv_reader::ui::pane::ReaderView;

use crate::view::AppView;

/// A print in progress. It belongs to one reader and one parse of its file.
pub struct Running {
    job: PrintJob,
    reader: Entity<ReaderView>,
}

impl AppView {
    pub fn start_print(&mut self, cx: &mut Context<Self>) {
        if self.print.is_some() {
            return;
        }
        let Some(reader) = self.reader() else {
            self.show_toast("Open a file to save it as a PDF", false, cx);
            return;
        };
        let Some(doc) = reader.read(cx).doc.clone() else {
            self.show_toast("Open a file to save it as a PDF", false, cx);
            return;
        };
        mdv_print::handoff::sweep(Duration::from_secs(24 * 3600));
        let s = self.reader_settings();
        let title = self.tabs.get(self.active).map(|t| t.name.to_string());
        let job = PrintJob::start(doc, PrintOptions { serif: s.serif, line_height: s.line_height, title, ..Default::default() });
        self.print = Some(Running { job, reader });
        self.toast = Some(("Preparing PDF… 0%".into(), true));
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let go = this.update(cx, |t, cx| t.print_tick(cx)).unwrap_or(false);
            if !go {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    /// Cancel on the toast, or the print's tab changing. Stops the worker and drops its result.
    pub fn cancel_print(&mut self, cx: &mut Context<Self>) {
        if let Some(r) = self.print.take() {
            r.job.cancel();
        }
        self.toast = None;
        cx.notify();
    }

    /// Returns whether to keep polling.
    fn print_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(run) = self.print.as_ref() else { return false };
        // The PDF must come from what the tab shows: stop if the tab, the file or the parse changed.
        let same_tab = self.reader().is_some_and(|r| r == run.reader);
        let same_doc = run.reader.read(cx).doc.as_ref().is_some_and(|d| d.id == run.job.doc_id);
        if !same_tab || !same_doc {
            self.cancel_print(cx);
            self.show_toast("PDF stopped because the tab or the file changed", false, cx);
            return false;
        }
        match run.job.state() {
            PrintState::Running { fraction } => {
                // Only the text changes; the Cancel link keeps its identity (MISTAKES 12).
                self.toast = Some((format!("Preparing PDF… {}%", (fraction * 100.0).round() as u32), true));
                cx.notify();
                true
            }
            PrintState::Done { path, report, bytes } => {
                self.print = None;
                let open = std::env::var("MDV_PRINT_OPEN").map(|v| v != "0").unwrap_or(true);
                eprintln!("[print] {} pages, {} bytes, {}", report.pages, bytes, path.display());
                if open {
                    match mdv_print::handoff::open_default(&path) {
                        Ok(()) => self.show_toast(&format!("PDF ready ({} pages): opened in your PDF viewer", report.pages), false, cx),
                        Err(e) => self.show_toast(&format!("PDF saved to {} but no viewer could open it ({e})", path.display()), false, cx),
                    }
                } else {
                    self.show_toast(&format!("PDF ready ({} pages)", report.pages), false, cx);
                }
                false
            }
            PrintState::Failed(e) => {
                self.print = None;
                self.show_toast(&e, false, cx);
                false
            }
            PrintState::Cancelled => {
                self.print = None;
                self.toast = None;
                cx.notify();
                false
            }
        }
    }
}
