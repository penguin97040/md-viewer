//! Local and data-URI images. Nothing here ever touches the network: web images stay held by the
//! model, and paths go through `paths::resolve`, which refuses network and device paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::RenderImage;

use crate::layout::{Assets, ImageState};
use crate::model::ImageSource;
use crate::paths;

const MAX_IMAGE_BYTES: u64 = 25 << 20;
const MAX_IMAGE_SIDE: u32 = 16_384;
/// Decoded size of one image. A small PNG can claim 16,384 x 16,384 pixels, which is 1 GB.
const MAX_DECODED_BYTES: u64 = 400 << 20;
/// Decoded images held across all tabs. Running out of memory aborts the process and so closes every
/// window, which `catch_unwind` cannot contain; past this, images show as failed instead.
const DECODED_BUDGET: u64 = 1 << 30;

static DECODED: AtomicU64 = AtomicU64::new(0);

/// Takes `need` bytes from `total`, unless that would pass the budget or the single-image limit.
fn reserve(total: &AtomicU64, need: u64, budget: u64) -> bool {
    need <= MAX_DECODED_BYTES && total.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |t| (t + need <= budget).then_some(t + need)).is_ok()
}

#[derive(Clone)]
enum Entry {
    Pending,
    Ready { w: f32, h: f32, key: u64, image: Arc<RenderImage> },
    Failed,
}

/// Per-document image cache. A request starts a load; `drain` reports what finished.
pub struct ImageStore {
    document: Option<PathBuf>,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
    finished: Arc<Mutex<u64>>,
    started: Mutex<Vec<String>>,
    /// This store's share of `DECODED`, given back when the document closes.
    decoded: Arc<AtomicU64>,
    loads_begun: AtomicU64,
}

impl Drop for ImageStore {
    fn drop(&mut self) {
        DECODED.fetch_sub(self.decoded.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

impl ImageStore {
    pub fn new(document: Option<PathBuf>) -> Self {
        Self { document, entries: Arc::default(), finished: Arc::default(), started: Mutex::new(Vec::new()), decoded: Arc::default(), loads_begun: AtomicU64::new(0) }
    }

    pub fn finished(&self) -> u64 {
        *self.finished.lock().expect("lock")
    }

    /// Loads started and not yet finished, including any requested by the layout just done.
    pub fn pending(&self) -> u64 {
        self.loads_begun.load(Ordering::SeqCst).saturating_sub(self.finished())
    }

    pub fn by_key(&self, key: u64) -> Option<Arc<RenderImage>> {
        self.entries.lock().expect("lock").values().find_map(|e| match e {
            Entry::Ready { key: k, image, .. } if *k == key => Some(image.clone()),
            _ => None,
        })
    }

    /// Requests made while laying out. The view hands them to a background thread.
    pub fn take_started(&self) -> Vec<String> {
        std::mem::take(&mut *self.started.lock().expect("lock"))
    }

    /// Loads one image on a background thread.
    pub fn load(&self, id: String) {
        self.loads_begun.fetch_add(1, Ordering::SeqCst);
        let entries = self.entries.clone();
        let finished = self.finished.clone();
        let doc = self.document.clone();
        let decoded = self.decoded.clone();
        std::thread::spawn(move || {
            // A decoder panic must still count as finished, or the repaint watcher never stops.
            let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_image(&id, doc.as_deref(), &decoded)));
            let entry = read.ok().flatten().unwrap_or(Entry::Failed);
            entries.lock().expect("lock").insert(id, entry);
            *finished.lock().expect("lock") += 1;
        });
    }
}

fn source_id(s: &ImageSource) -> Option<String> {
    match s {
        ImageSource::Local(p) => Some(format!("L{p}")),
        ImageSource::Data(d) => Some(format!("D{d}")),
        ImageSource::Web(_) => None,
    }
}

impl Assets for ImageStore {
    fn image(&self, source: &ImageSource) -> ImageState {
        let Some(id) = source_id(source) else { return ImageState::Failed };
        let mut e = self.entries.lock().expect("lock");
        match e.get(&id) {
            Some(Entry::Ready { w, h, key, .. }) => ImageState::Ready { width: *w, height: *h, key: *key },
            Some(Entry::Failed) => ImageState::Failed,
            Some(Entry::Pending) => ImageState::Pending,
            None => {
                e.insert(id.clone(), Entry::Pending);
                self.started.lock().expect("lock").push(id);
                ImageState::Pending
            }
        }
    }
}

fn read_image(id: &str, doc: Option<&Path>, decoded: &AtomicU64) -> Option<Entry> {
    let bytes = if let Some(href) = id.strip_prefix('L') {
        // The security check comes before anything can touch the file system.
        let path = paths::resolve(doc?, href).ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > MAX_IMAGE_BYTES {
            return None;
        }
        std::fs::read(&path).ok()?
    } else {
        let d = id.strip_prefix('D')?;
        base64(d.split_once(',')?.1)?
    };
    let mut reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?;
    // GPUI turns on every decoder the `image` crate has (TIFF, EXR, DDS and more). A document only
    // gets the formats on the Go reader's list, so the others are not reachable from one.
    use image::ImageFormat as F;
    if !matches!(reader.format()?, F::Png | F::Jpeg | F::Gif | F::WebP | F::Bmp | F::Ico) {
        return None;
    }
    let (claimed_w, claimed_h) = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
    if claimed_w > MAX_IMAGE_SIDE || claimed_h > MAX_IMAGE_SIDE {
        return None;
    }
    let need = claimed_w as u64 * claimed_h as u64 * 4;
    if !reserve(&DECODED, need, DECODED_BUDGET) {
        return None;
    }
    let give_back = || {
        DECODED.fetch_sub(need, Ordering::SeqCst);
    };
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let Ok(picture) = reader.decode() else {
        give_back();
        return None;
    };
    decoded.fetch_add(need, Ordering::SeqCst);
    let mut rgba = picture.into_rgba8();
    let (w, h) = rgba.dimensions();
    for p in rgba.pixels_mut() {
        p.0.swap(0, 2);
    }
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]));
    let key = image.id.0 as u64;
    Some(Entry::Ready { w: w as f32, h: h as f32, key, image })
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return None,
        };
        buf = buf << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes() {
        assert_eq!(base64("aGk=").unwrap(), b"hi");
        assert!(base64("a*").is_none());
    }

    #[test]
    fn network_paths_never_reach_the_file_system() {
        let doc = Path::new("/home/me/a.md");
        for p in [r"L\\attacker\share\a.png", "L//attacker/share/a.png", r"L\\?\C:\a.png", "L%5C%5Cattacker%5Cshare%5Ca.png"] {
            assert!(read_image(p, Some(doc), &AtomicU64::new(0)).is_none(), "{p}");
        }
    }

    #[test]
    fn decoded_images_stay_within_the_budget() {
        let total = AtomicU64::new(0);
        assert!(!reserve(&total, 16_384 * 16_384 * 4, u64::MAX), "one bomb is refused by itself");
        assert_eq!(total.load(Ordering::SeqCst), 0);
        assert!(reserve(&total, 300 << 20, 1 << 30));
        assert!(reserve(&total, 300 << 20, 1 << 30));
        assert!(reserve(&total, 300 << 20, 1 << 30));
        assert!(!reserve(&total, 300 << 20, 1 << 30), "the fourth would pass the budget");
        assert_eq!(total.load(Ordering::SeqCst), 900 << 20);
    }

    #[test]
    fn only_listed_formats_decode() {
        // A valid one-pixel binary PPM: the decoder is compiled in, but no document can reach it.
        let (bytes, doc) = (b"P6 1 1 255 abc".to_vec(), std::env::temp_dir().join(format!("mdv-img-{}.md", std::process::id())));
        let img = doc.with_extension("ppm");
        std::fs::write(&img, bytes).unwrap();
        assert!(read_image(&format!("L{}", img.display()), Some(&doc), &AtomicU64::new(0)).is_none());
        std::fs::remove_file(&img).unwrap();
    }
}
