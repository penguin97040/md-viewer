//! Maths and diagrams for the on-screen reader: renders them off the UI thread through
//! `mdv_content`, rasterises at the window's scale and caches the result by source and options.
//!
//! Layout asks for a size; until the pixels exist it gets `None` and draws a placeholder, and the
//! request is queued. Finished work is listed in `take_done` so the view re-lays-out only the
//! chunks that were waiting for it.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use gpui::RenderImage;
use mdv_content::{rasterise, ContentTheme, DiagramOptions, MathOptions, RenderedAsset};

use crate::layout::{AssetBox, Assets, ImageState};
use crate::model::ImageSource;
use mdv_theme::Mode;

/// `.md .katex { font-size: 1.1em }`.
const KATEX_EM: f32 = 1.1;
/// Raster memory kept before the least recently used pictures are dropped.
const RASTER_BUDGET: usize = 384 << 20;
/// Pixels in one raster before its scale is reduced.
const MAX_RASTER_PIXELS: f32 = 24_000_000.0;
const WORKERS: usize = 3;
const MAX_QUEUED: usize = 96;
/// Mermaid.js draws at 16 px whatever the body size is.
const MERMAID_PX: f32 = 16.0;

enum Entry {
    Pending,
    Ready { w: f32, h: f32, baseline: f32, image: Arc<RenderImage>, bytes: usize, used: u64 },
    Failed(String),
}

enum Kind {
    Math { src: String, display: bool, opts: MathOptions },
    Mermaid { src: String, opts: DiagramOptions },
}

struct Job {
    key: u64,
    kind: Kind,
    scale: f32,
}

#[derive(Default)]
struct Map {
    entries: HashMap<u64, Entry>,
    bytes: usize,
    tick: u64,
    done: Vec<u64>,
}

#[derive(Default)]
struct Inner {
    map: Mutex<Map>,
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    finished: AtomicU64,
    outstanding: AtomicUsize,
    started: AtomicBool,
}

#[derive(Clone, Default)]
pub struct ContentStore {
    inner: Arc<Inner>,
}

fn hash_of<T: Hash>(v: T) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn theme_of(mode: Mode) -> ContentTheme {
    match mode {
        Mode::Dark => ContentTheme::Dark,
        Mode::Light => ContentTheme::Light,
    }
}

impl ContentStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Count of finished renders, successes and failures alike.
    pub fn finished(&self) -> u64 {
        self.inner.finished.load(Ordering::Acquire)
    }

    /// Requests not yet finished.
    pub fn outstanding(&self) -> usize {
        self.inner.outstanding.load(Ordering::Acquire)
    }

    /// Keys finished since the last call.
    pub fn take_done(&self) -> Vec<u64> {
        std::mem::take(&mut self.inner.map.lock().expect("lock").done)
    }

    pub fn is_pending(&self, key: u64) -> bool {
        matches!(self.inner.map.lock().expect("lock").entries.get(&key), Some(Entry::Pending))
    }

    pub fn by_key(&self, key: u64) -> Option<Arc<RenderImage>> {
        let mut m = self.inner.map.lock().expect("lock");
        m.tick += 1;
        let t = m.tick;
        match m.entries.get_mut(&key) {
            Some(Entry::Ready { image, used, .. }) => {
                *used = t;
                Some(image.clone())
            }
            _ => None,
        }
    }

    /// Drops queued work nobody can see any more (a new document, a tab switch). Work already
    /// running finishes into the cache, which is keyed by source and options and so stays valid.
    pub fn cancel_queued(&self) {
        let dropped: Vec<Job> = self.inner.queue.lock().expect("lock").drain(..).collect();
        let mut m = self.inner.map.lock().expect("lock");
        for j in dropped {
            if matches!(m.entries.get(&j.key), Some(Entry::Pending)) {
                m.entries.remove(&j.key);
            }
            self.inner.outstanding.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn lookup(&self, key: u64, make: impl FnOnce() -> Kind, scale: f32) -> Lookup {
        let mut m = self.inner.map.lock().expect("lock");
        m.tick += 1;
        let t = m.tick;
        match m.entries.get_mut(&key) {
            Some(Entry::Ready { w, h, baseline, used, .. }) => {
                *used = t;
                Lookup::Ready(AssetBox { width: *w, height: *h, baseline: *baseline, key })
            }
            Some(Entry::Failed(e)) => Lookup::Failed(e.clone()),
            Some(Entry::Pending) => Lookup::Pending(key),
            None => {
                m.entries.insert(key, Entry::Pending);
                drop(m);
                self.inner.outstanding.fetch_add(1, Ordering::AcqRel);
                let mut q = self.inner.queue.lock().expect("lock");
                q.push_back(Job { key, kind: make(), scale });
                // Newest first, and only so many: what scrolled past is asked for again if it returns.
                let mut dropped = Vec::new();
                while q.len() > MAX_QUEUED {
                    dropped.extend(q.pop_front().map(|j| j.key));
                }
                drop(q);
                if !dropped.is_empty() {
                    let mut m = self.inner.map.lock().expect("lock");
                    for k in &dropped {
                        if matches!(m.entries.get(k), Some(Entry::Pending)) {
                            m.entries.remove(k);
                        }
                    }
                    // The chunks that waited on them are redone, which asks again if still needed.
                    m.done.extend(dropped.iter().copied());
                    self.inner.outstanding.fetch_sub(dropped.len(), Ordering::AcqRel);
                }
                self.start_workers();
                self.inner.wake.notify_one();
                Lookup::Pending(key)
            }
        }
    }

    fn start_workers(&self) {
        if self.inner.started.swap(true, Ordering::AcqRel) {
            return;
        }
        for n in 0..WORKERS {
            let inner = self.inner.clone();
            let _ = std::thread::Builder::new().name(format!("mdv-content-{n}")).spawn(move || worker(inner));
        }
    }

    pub fn math(&self, src: &str, display: bool, font_size: f32, mode: Mode, scale: f32) -> Lookup {
        let opts = MathOptions { theme: theme_of(mode), font_size: font_size * KATEX_EM, text_color: None };
        let key = math_key(src, display, font_size, mode, scale);
        self.lookup(key, || Kind::Math { src: src.to_string(), display, opts }, scale)
    }

    pub fn mermaid(&self, src: &str, font_size: f32, mode: Mode, scale: f32) -> Lookup {
        let opts = DiagramOptions { theme: theme_of(mode), font_size, text_color: None };
        let key = mermaid_key(src, font_size, mode, scale);
        self.lookup(key, || Kind::Mermaid { src: src.to_string(), opts }, scale)
    }
}

fn scale_bits(s: f32) -> u32 {
    (s * 100.0).round() as u32
}

pub enum Lookup {
    Ready(AssetBox),
    Pending(u64),
    Failed(String),
}

fn worker(inner: Arc<Inner>) {
    loop {
        let job = {
            let mut q = inner.queue.lock().expect("lock");
            loop {
                // Formulas are quick and diagrams slow: take a formula first so a diagram never
                // holds up the text around it.
                if let Some(i) = q.iter().rposition(|j| matches!(j.kind, Kind::Math { .. })).or(q.len().checked_sub(1)) {
                    break q.remove(i).expect("job");
                }
                q = inner.wake.wait(q).expect("wait");
            }
        };
        let result = std::panic::catch_unwind(|| run(&job)).unwrap_or_else(|_| Err("the renderer stopped unexpectedly".to_string()));
        let mut m = inner.map.lock().expect("lock");
        // Only a job still wanted (not cancelled) has a Pending entry to complete.
        let entry = match result {
            Ok((w, h, baseline, image, bytes)) => {
                m.bytes += bytes;
                m.tick += 1;
                Entry::Ready { w, h, baseline, image, bytes, used: m.tick }
            }
            Err(e) => Entry::Failed(e),
        };
        m.entries.insert(job.key, entry);
        m.done.push(job.key);
        evict(&mut m);
        drop(m);
        inner.outstanding.fetch_sub(1, Ordering::AcqRel);
        inner.finished.fetch_add(1, Ordering::AcqRel);
    }
}

fn evict(m: &mut Map) {
    while m.bytes > RASTER_BUDGET {
        let Some((&k, _)) = m.entries.iter().filter(|(_, e)| matches!(e, Entry::Ready { .. })).min_by_key(|(_, e)| match e {
            Entry::Ready { used, .. } => *used,
            _ => u64::MAX,
        }) else {
            break;
        };
        if let Some(Entry::Ready { bytes, .. }) = m.entries.remove(&k) {
            m.bytes -= bytes;
        }
    }
}

type Done = (f32, f32, f32, Arc<RenderImage>, usize);

fn run(job: &Job) -> Result<Done, String> {
    let asset: RenderedAsset = match &job.kind {
        Kind::Math { src, display, opts } => mdv_content::render_math(src, *display, opts),
        Kind::Mermaid { src, opts } => mdv_content::render_mermaid(src, opts),
    }
    .map_err(|e| e.to_string())?;
    let mut scale = job.scale.clamp(0.5, 8.0);
    let px = asset.width_px * asset.height_px * scale * scale;
    if px > MAX_RASTER_PIXELS {
        scale *= (MAX_RASTER_PIXELS / px).sqrt();
    }
    let img = rasterise(&asset, scale).map_err(|e| e.to_string())?;
    let mut px = img.pixels;
    // BGRA with premultiplied alpha, which is what the GPU path expects.
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        let (r, g, b) = (p[0] as u32, p[1] as u32, p[2] as u32);
        p[0] = ((b * a + 127) / 255) as u8;
        p[1] = ((g * a + 127) / 255) as u8;
        p[2] = ((r * a + 127) / 255) as u8;
    }
    let bytes = px.len();
    let buf = image::RgbaImage::from_raw(img.width, img.height, px).ok_or_else(|| "bad raster size".to_string())?;
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
    Ok((asset.width_px, asset.height_px, asset.baseline_px, image, bytes))
}

/// The `Assets` the layout sees for one pass: this store, one colour scheme and one scale.
pub struct ContentAssets<'a> {
    pub store: &'a ContentStore,
    pub mode: Mode,
    pub scale: f32,
    /// Keys that were still pending when asked for, so the caller knows what the layout awaits.
    pub waiting: std::cell::RefCell<Vec<u64>>,
}

impl<'a> ContentAssets<'a> {
    pub fn new(store: &'a ContentStore, mode: Mode, scale: f32) -> Self {
        Self { store, mode, scale, waiting: Default::default() }
    }
    fn answer(&self, l: Lookup) -> Option<AssetBox> {
        match l {
            Lookup::Ready(a) => Some(a),
            Lookup::Pending(k) => {
                self.waiting.borrow_mut().push(k);
                None
            }
            Lookup::Failed(_) => None,
        }
    }
}

impl Assets for ContentAssets<'_> {
    fn math(&self, src: &str, display: bool, font_size: f32) -> Option<AssetBox> {
        let l = self.store.math(src, display, font_size, self.mode, self.scale);
        self.answer(l)
    }
    fn mermaid(&self, src: &str) -> Option<AssetBox> {
        let l = self.store.mermaid(src, MERMAID_PX, self.mode, self.scale);
        self.answer(l)
    }
    fn math_error(&self, src: &str, display: bool, font_size: f32) -> Option<String> {
        match self.store.math(src, display, font_size, self.mode, self.scale) {
            Lookup::Failed(e) => Some(e),
            _ => None,
        }
    }
    fn mermaid_error(&self, src: &str) -> Option<String> {
        match self.store.mermaid(src, MERMAID_PX, self.mode, self.scale) {
            Lookup::Failed(e) => Some(e),
            _ => None,
        }
    }
    fn image(&self, _source: &ImageSource) -> ImageState {
        ImageState::Failed
    }
}

fn math_key(src: &str, display: bool, font_size: f32, mode: Mode, scale: f32) -> u64 {
    hash_of(("m", src, display, theme_of(mode) as u8, (font_size * KATEX_EM).to_bits(), scale_bits(scale)))
}

fn mermaid_key(src: &str, font_size: f32, mode: Mode, scale: f32) -> u64 {
    hash_of(("d", src, theme_of(mode) as u8, font_size.to_bits(), scale_bits(scale)))
}
