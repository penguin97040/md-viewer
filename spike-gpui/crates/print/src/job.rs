//! Runs a print job off the UI thread. The UI polls [`PrintJob::state`] for the progress toast and
//! calls [`PrintJob::cancel`] from its Cancel button; the job checks the flag between chunks and
//! pages, so cancelling stops it within one chunk's work and drops everything it built.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use mdv_reader::doc::Document;

use crate::{PrintError, PrintOptions, Report};

#[derive(Debug)]
pub enum PrintState {
    Running { fraction: f32 },
    Done { path: std::path::PathBuf, report: Report, bytes: usize },
    Failed(String),
    Cancelled,
}

pub struct PrintJob {
    cancel: Arc<AtomicBool>,
    fraction: Arc<AtomicU32>,
    result: Arc<Mutex<Option<PrintState>>>,
    /// Which document the job prints, so a tab switch can tell whether it still applies.
    pub doc_id: u64,
}

impl PrintJob {
    /// Starts printing `doc`. The finished PDF is written to a fresh private temporary file.
    pub fn start(doc: Arc<Document>, opts: PrintOptions) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let fraction = Arc::new(AtomicU32::new(0));
        let result: Arc<Mutex<Option<PrintState>>> = Arc::default();
        let (c2, f2, r2) = (cancel.clone(), fraction.clone(), result.clone());
        let doc_id = doc.id;
        let spawned = std::thread::Builder::new().name("mdv-print".into()).stack_size(64 << 20).spawn(move || {
            let mut ctl = |p: crate::Progress| {
                f2.store(p.fraction().to_bits(), Ordering::Relaxed);
                !c2.load(Ordering::Relaxed)
            };
            let state = match crate::render_pdf(&doc, &opts, &mut ctl) {
                Ok((bytes, report)) => {
                    if c2.load(Ordering::Relaxed) {
                        PrintState::Cancelled
                    } else {
                        match crate::handoff::write_temp(&bytes) {
                            Ok(path) => PrintState::Done { path, report, bytes: bytes.len() },
                            Err(e) => PrintState::Failed(format!("could not write the PDF: {e}")),
                        }
                    }
                }
                Err(PrintError::Cancelled) => PrintState::Cancelled,
                Err(e) => PrintState::Failed(e.to_string()),
            };
            *r2.lock().expect("lock") = Some(state);
        });
        if spawned.is_err() {
            *result.lock().expect("lock") = Some(PrintState::Failed("could not start the print thread".into()));
        }
        Self { cancel, fraction, result, doc_id }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The current state; a finished state is returned once, then the job reads as cancelled.
    pub fn state(&self) -> PrintState {
        let mut r = self.result.lock().expect("lock");
        match r.take() {
            Some(s) => {
                *r = Some(PrintState::Cancelled);
                s
            }
            None => PrintState::Running { fraction: f32::from_bits(self.fraction.load(Ordering::Relaxed)) },
        }
    }
}
