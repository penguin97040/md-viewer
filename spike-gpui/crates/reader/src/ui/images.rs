//! Local and data-URI images. Nothing here ever touches the network: web images stay held by the
//! model, and paths go through `paths::resolve`, which refuses network and device paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui::RenderImage;

use crate::layout::{Assets, ImageState};
use crate::model::ImageSource;
use crate::paths;

const MAX_IMAGE_BYTES: u64 = 25 << 20;
const MAX_IMAGE_SIDE: u32 = 16_384;

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
}

impl ImageStore {
    pub fn new(document: Option<PathBuf>) -> Self {
        Self { document, entries: Arc::default(), finished: Arc::default(), started: Mutex::new(Vec::new()) }
    }

    pub fn finished(&self) -> u64 {
        *self.finished.lock().expect("lock")
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
        let entries = self.entries.clone();
        let finished = self.finished.clone();
        let doc = self.document.clone();
        std::thread::spawn(move || {
            let entry = read_image(&id, doc.as_deref()).unwrap_or(Entry::Failed);
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

fn read_image(id: &str, doc: Option<&Path>) -> Option<Entry> {
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
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let mut rgba = reader.decode().ok()?.into_rgba8();
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
            assert!(read_image(p, Some(doc)).is_none(), "{p}");
        }
    }
}
