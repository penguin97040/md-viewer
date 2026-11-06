//! Print and Ctrl+P: asks where to save with the system's Save As dialog, then renders the active
//! tab to a PDF off the UI thread behind the progress toast with Cancel. On success the toast
//! offers Open, which hands the file to the system's PDF viewer (see `mdv_print::handoff`).
//!
//! With `dev-harness`, a path can stand in for the dialog in tests, and `-` stands in
//! for cancelling it.

use std::path::{Path, PathBuf};

use std::time::Duration;

use gpui::{Context, Entity};
use mdv_print::{PrintJob, PrintOptions, PrintState};
use mdv_reader::ui::pane::ReaderView;

use crate::view::AppView;

/// A print in progress. It belongs to one reader and one parse of its file. While the Save As
/// dialog is open there is no job yet.
pub struct Running {
    job: Option<PrintJob>,
    reader: Entity<ReaderView>,
}

/// Where the dialog starts and what it suggests: the document's own folder and `<name>.pdf`.
fn suggestion(tab_path: &str, tab_name: &str) -> (PathBuf, String) {
    let p = Path::new(tab_path);
    let dir = p.parent().filter(|d| d.is_dir()).map(Path::to_path_buf).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
    let stem = Path::new(tab_name).file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()).unwrap_or_else(|| "document".into());
    (dir, format!("{stem}.pdf"))
}

/// The dialog's filter is "All files" and it adds no extension, so a bare name gets `.pdf`.
fn with_pdf_extension(p: PathBuf) -> (PathBuf, bool) {
    if p.extension().is_some() {
        (p, false)
    } else {
        let mut s = p.into_os_string();
        s.push(".pdf");
        (s.into(), true)
    }
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
        let (tab_path, tab_name) = self.tabs.get(self.active).map(|t| (t.path.to_string(), t.name.to_string())).unwrap_or_default();
        let (dir, name) = suggestion(&tab_path, &tab_name);
        let doc_id = doc.id;
        self.print = Some(Running { job: None, reader });
        // Ask first: nothing is built until there is somewhere to put it.
        let asked: Result<Option<PathBuf>, String> = match crate::harness::print_save_to() {
            Some(v) if v == "-" => Ok(None),
            Some(v) => Ok(Some(PathBuf::from(v))),
            None => {
                let rx = cx.prompt_for_new_path(&dir, Some(&name));
                cx.spawn(async move |this, cx| {
                    let got = match rx.await {
                        Ok(Ok(p)) => Ok(p),
                        Ok(Err(e)) => {
                            eprintln!("[print] Save As dialog unavailable: {e}");
                            Err("Could not show the Save As dialog: this system has no file chooser available".to_string())
                        }
                        Err(_) => Err("The Save As dialog closed unexpectedly".to_string()),
                    };
                    let _ = this.update(cx, |t, cx| t.print_chosen(got, doc_id, cx));
                })
                .detach();
                cx.notify();
                return;
            }
        };
        self.print_chosen(asked, doc_id, cx);
    }

    /// The Save As dialog answered: cancel does nothing, an error is reported, a path starts the job.
    fn print_chosen(&mut self, got: Result<Option<PathBuf>, String>, doc_id: u64, cx: &mut Context<Self>) {
        let Some(run) = self.print.as_ref().filter(|r| r.job.is_none()) else { return };
        let (dest, added_ext) = match got {
            Ok(Some(p)) => with_pdf_extension(p),
            Ok(None) => {
                self.print = None;
                cx.notify();
                return;
            }
            Err(e) => {
                self.print = None;
                self.show_toast(&e, false, cx);
                return;
            }
        };
        // The dialog already asked about replacing the name the person typed. If `.pdf` was added
        // here, that file was not the one confirmed, so it is never replaced.
        if (added_ext && dest.exists()) || dest.is_dir() {
            self.print = None;
            let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.show_toast(&format!("Could not save the PDF: {name} already exists. Choose another name"), false, cx);
            return;
        }
        // The tab or file may have changed while the dialog was open.
        let same_tab = self.reader().is_some_and(|r| r == run.reader);
        let web_images = run.reader.read(cx).web_images_allowed();
        let doc = run.reader.read(cx).doc.clone().filter(|d| d.id == doc_id);
        let (true, Some(doc)) = (same_tab, doc) else {
            self.print = None;
            self.show_toast("PDF stopped because the tab or the file changed", false, cx);
            return;
        };
        let s = self.reader_settings();
        let title = self.tabs.get(self.active).map(|t| t.name.to_string());
        let job = PrintJob::start(doc, PrintOptions { serif: s.serif, line_height: s.line_height, title, web_images, ..Default::default() }, dest);
        if let Some(r) = self.print.as_mut() {
            r.job = Some(job);
        }
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
            if let Some(job) = &r.job {
                job.cancel();
            }
        }
        self.toast = None;
        cx.notify();
    }

    /// Returns whether to keep polling.
    fn print_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(run) = self.print.as_ref() else { return false };
        let Some(job) = run.job.as_ref() else { return false };
        // The PDF must come from what the tab shows: stop if the tab, the file or the parse changed.
        let same_tab = self.reader().is_some_and(|r| r == run.reader);
        let same_doc = run.reader.read(cx).doc.as_ref().is_some_and(|d| d.id == job.doc_id);
        if !same_tab || !same_doc {
            self.cancel_print(cx);
            self.show_toast("PDF stopped because the tab or the file changed", false, cx);
            return false;
        }
        match job.state() {
            PrintState::Running { fraction } => {
                // Only the text changes; the Cancel link keeps its identity (MISTAKES 12).
                self.toast = Some((format!("Preparing PDF… {}%", (fraction * 100.0).round() as u32), true));
                cx.notify();
                true
            }
            PrintState::Done { path, report, bytes } => {
                self.print = None;
                eprintln!("[print] {} pages, {} bytes, {}", report.pages, bytes, path.display());
                self.show_saved(path, cx);
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

    /// "Saved <name>" with an Open action. It stays for a while so there is time to reach it.
    fn show_saved(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "PDF".into());
        // Not `show_toast`, whose four-second timer would hide it before there is time to click.
        self.toast = Some((format!("Saved {name}"), false));
        self.toast_open = Some(path.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(12)).await;
            let _ = this.update(cx, |t, cx| {
                if t.toast_open.as_ref() == Some(&path) {
                    t.toast_open = None;
                    t.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The Open action: the viewer gets the path as one argument, with no shell.
    pub fn open_saved(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.toast_open = None;
        self.toast = None;
        // Windows goes through ShellExecuteW: `explorer.exe` splits its arguments at commas, and the
        // suggested name comes from the document's, which may contain one. Compile-checked only.
        #[cfg(target_os = "windows")]
        cx.open_with_system(&path);
        #[cfg(not(target_os = "windows"))]
        if let Err(e) = mdv_print::handoff::open_default(&path) {
            self.show_toast(&format!("Could not open the PDF ({e}). It is saved at {}", path.display()), false, cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_the_documents_folder_and_name() {
        let d = std::env::temp_dir();
        let f = d.join("kepler-track.md");
        let (dir, name) = suggestion(&f.to_string_lossy(), "kepler-track.md");
        assert_eq!(dir, d);
        assert_eq!(name, "kepler-track.pdf");
        // A document with no folder of its own still gets a usable start folder and name.
        let (dir, name) = suggestion("", "");
        assert!(dir.as_os_str().is_empty() || dir.is_dir());
        assert_eq!(name, "document.pdf");
    }

    #[test]
    fn adds_pdf_only_when_there_is_no_extension() {
        assert_eq!(with_pdf_extension(PathBuf::from("/a/b")), (PathBuf::from("/a/b.pdf"), true));
        assert_eq!(with_pdf_extension(PathBuf::from("/a/b.pdf")), (PathBuf::from("/a/b.pdf"), false));
        assert_eq!(with_pdf_extension(PathBuf::from("/a/b.txt")), (PathBuf::from("/a/b.txt"), false));
    }
}
