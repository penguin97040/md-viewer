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
use std::time::{Duration, Instant};

use gpui::RenderImage;
use mdv_content::{boost_light_on_dark, rasterise, ContentTheme, DiagramOptions, MathOptions, RenderedAsset};

use crate::layout::{AssetBox, Assets, ImageState};
use crate::locked;
use crate::model::ImageSource;
use mdv_theme::Mode;

/// `.md .katex { font-size: 1.1em }`.
const KATEX_EM: f32 = 1.1;
/// Raster memory kept before the least recently used pictures are dropped.
const RASTER_BUDGET: usize = 384 << 20;
/// Pixels in one raster before its scale is reduced.
const MAX_RASTER_PIXELS: f32 = 24_000_000.0;
/// Worker threads for one document, which exist only while it has work.
const WORKERS: usize = 3;
/// How long an idle worker waits for more before it ends.
const WORKER_IDLE: Duration = Duration::from_secs(10);
/// A render refused because too many earlier ones are still running is tried again this often,
/// and after this many tries shows its error.
const BUSY_RETRY: Duration = Duration::from_secs(1);
const BUSY_TRIES: u32 = 30;
const MAX_QUEUED: usize = 96;
/// Mermaid.js draws at 16 px whatever the body size is.
const MERMAID_PX: f32 = 16.0;

enum Entry {
    Pending,
    Ready { w: f32, h: f32, baseline: f32, image: Arc<RenderImage>, bytes: usize, used: u64, redraw: Option<Redraw> },
    Failed(String),
}

/// What a finished diagram needs to be rasterised again at another size without laying it out again.
#[derive(Clone)]
struct Redraw {
    asset: Arc<RenderedAsset>,
    dark: bool,
}

enum Kind {
    Math { src: String, display: bool, opts: MathOptions },
    Mermaid { src: String, opts: DiagramOptions },
    /// A finished diagram drawn again so that it is `width` physical pixels wide.
    Sharp { base: u64, redraw: Redraw, width: u32 },
}

struct Job {
    key: u64,
    kind: Kind,
    scale: f32,
    tries: u32,
    /// Not to be tried before this.
    not_before: Option<Instant>,
}

impl Job {
    fn new(key: u64, kind: Kind, scale: f32) -> Self {
        Self { key, kind, scale, tries: 0, not_before: None }
    }

    fn ready(&self, now: Instant) -> bool {
        self.not_before.is_none_or(|t| t <= now)
    }
}

#[derive(Default)]
struct Map {
    entries: HashMap<u64, Entry>,
    bytes: usize,
    tick: u64,
    done: Vec<u64>,
    /// Per diagram, the latest size-matched raster asked for: (its key, its width in pixels).
    sharp: HashMap<u64, (u64, u32)>,
    /// Per diagram, the key of the newest size-matched raster that finished.
    last_ready: HashMap<u64, u64>,
    /// Pictures let go of, whose GPU copies the view is yet to release.
    retired: Vec<Arc<RenderImage>>,
}

struct Inner {
    map: Mutex<Map>,
    queue: Mutex<VecDeque<Job>>,
    wake: Condvar,
    finished: AtomicU64,
    outstanding: AtomicUsize,
    workers: AtomicUsize,
    /// Every handle has gone: the workers end, which frees this.
    closed: AtomicBool,
    /// Who this document's renders are counted against.
    owner: u64,
}

/// Dropped with the last `ContentStore` handle. The workers hold `Inner` themselves, so without
/// this a closed document would keep its threads and its pictures for good.
struct Handle(Arc<Inner>);

impl Drop for Handle {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::SeqCst);
        let _held = locked(&self.0.queue);
        self.0.wake.notify_all();
    }
}

#[derive(Clone)]
pub struct ContentStore {
    inner: Arc<Inner>,
    _handle: Arc<Handle>,
}

impl Default for ContentStore {
    fn default() -> Self {
        let inner = Arc::new(Inner {
            map: Mutex::default(),
            queue: Mutex::default(),
            wake: Condvar::new(),
            finished: AtomicU64::new(0),
            outstanding: AtomicUsize::new(0),
            workers: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            owner: mdv_content::new_render_owner(),
        });
        Self { _handle: Arc::new(Handle(inner.clone())), inner }
    }
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

    /// Pictures this store has let go of since the last call.
    pub fn take_retired(&self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut locked(&self.inner.map).retired)
    }

    /// Every picture held, for a document that is closing.
    pub fn take_images(&self) -> Vec<Arc<RenderImage>> {
        let mut m = locked(&self.inner.map);
        let mut out = std::mem::take(&mut m.retired);
        out.extend(m.entries.drain().filter_map(|(_, e)| match e {
            Entry::Ready { image, .. } => Some(image),
            _ => None,
        }));
        m.bytes = 0;
        out
    }

    /// Keys finished since the last call.
    pub fn take_done(&self) -> Vec<u64> {
        std::mem::take(&mut locked(&self.inner.map).done)
    }

    pub fn is_pending(&self, key: u64) -> bool {
        matches!(locked(&self.inner.map).entries.get(&key), Some(Entry::Pending))
    }

    pub fn by_key(&self, key: u64) -> Option<Arc<RenderImage>> {
        let mut m = locked(&self.inner.map);
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
        let dropped: Vec<Job> = locked(&self.inner.queue).drain(..).collect();
        let mut m = locked(&self.inner.map);
        for j in dropped {
            if matches!(m.entries.get(&j.key), Some(Entry::Pending)) {
                m.entries.remove(&j.key);
            }
            self.inner.outstanding.fetch_sub(1, Ordering::AcqRel);
        }
    }

    fn lookup(&self, key: u64, make: impl FnOnce() -> Kind, scale: f32) -> Lookup {
        let mut m = locked(&self.inner.map);
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
                let mut q = locked(&self.inner.queue);
                q.push_back(Job::new(key, make(), scale));
                // Newest first, and only so many: what scrolled past is asked for again if it returns.
                let mut dropped = Vec::new();
                while q.len() > MAX_QUEUED {
                    dropped.extend(q.pop_front().map(|j| j.key));
                }
                drop(q);
                if !dropped.is_empty() {
                    let mut m = locked(&self.inner.map);
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

    /// The raster to paint a diagram that is shown `w` logical pixels wide, narrower than its own
    /// size. Asks for the diagram to be drawn again at exactly that many device pixels, so that it
    /// paints one raster pixel to one device pixel as other rasters do, and meanwhile answers with
    /// the previous size-matched raster, or the one it has, which the GPU scales. The second value is
    /// true when a new render was queued, so the caller can ask for another frame.
    pub fn for_display(&self, key: u64, w: f32, scale: f32) -> (Option<Arc<RenderImage>>, bool) {
        let width = (w * scale).ceil() as u32;
        let mut m = locked(&self.inner.map);
        m.tick += 1;
        let t = m.tick;
        let redraw = match m.entries.get_mut(&key) {
            Some(Entry::Ready { image, used, redraw, .. }) => {
                *used = t;
                let natural = image.size(0).width.0 as u32;
                match redraw {
                    Some(r) if width >= 1 && width + 1 < natural => (image.clone(), r.clone()),
                    _ => return (Some(image.clone()), false),
                }
            }
            _ => return (None, false),
        };
        let (base_image, redraw) = redraw;
        let vkey = hash_of(("s", key, width));
        if let Some(Entry::Ready { image, used, .. }) = m.entries.get_mut(&vkey) {
            *used = t;
            return (Some(image.clone()), false);
        }
        let mut queued = false;
        if !matches!(m.entries.get(&vkey), Some(Entry::Pending)) {
            // One request per diagram is wanted at a time: a newer size replaces a queued older one.
            if let Some((old, _)) = m.sharp.insert(key, (vkey, width)) {
                let mut q = locked(&self.inner.queue);
                if let Some(i) = q.iter().position(|j| j.key == old) {
                    q.remove(i);
                    self.inner.outstanding.fetch_sub(1, Ordering::AcqRel);
                    if matches!(m.entries.get(&old), Some(Entry::Pending)) {
                        m.entries.remove(&old);
                    }
                }
            }
            m.entries.insert(vkey, Entry::Pending);
            self.inner.outstanding.fetch_add(1, Ordering::AcqRel);
            locked(&self.inner.queue).push_back(Job::new(vkey, Kind::Sharp { base: key, redraw, width }, scale));
            queued = true;
        }
        // The newest finished size-matched raster of this diagram, if any, else the base one.
        let shown = match m.last_ready.get(&key).and_then(|k| m.entries.get(k)) {
            Some(Entry::Ready { image, .. }) => image.clone(),
            _ => base_image,
        };
        drop(m);
        if queued {
            self.start_workers();
            self.inner.wake.notify_one();
        }
        (Some(shown), queued)
    }

    fn start_workers(&self) {
        while self.inner.workers.try_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < WORKERS).then_some(n + 1)).is_ok() {
            let inner = self.inner.clone();
            // The same stack as the renders: drawing a diagram walks as deep as the diagram goes.
            let spawned = std::thread::Builder::new().name("mdv-content".into()).stack_size(mdv_content::limits::WORKER_STACK).spawn(move || worker(inner));
            if spawned.is_err() {
                self.inner.workers.fetch_sub(1, Ordering::AcqRel);
                break;
            }
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

/// The next job to run: newest first, and formulas before everything else, since formulas are quick
/// and diagrams slow, and a diagram should never hold up the text around it.
fn next_job(q: &VecDeque<Job>, now: Instant) -> Option<usize> {
    q.iter().rposition(|j| j.ready(now) && matches!(j.kind, Kind::Math { .. })).or_else(|| q.iter().rposition(|j| j.ready(now)))
}

fn worker(inner: Arc<Inner>) {
    mdv_content::set_render_owner(inner.owner);
    loop {
        let mut job = {
            let mut q = locked(&inner.queue);
            let mut idle_since = Instant::now();
            loop {
                let now = Instant::now();
                if inner.closed.load(Ordering::SeqCst) {
                    inner.workers.fetch_sub(1, Ordering::AcqRel);
                    return;
                }
                if let Some(job) = next_job(&q, now).and_then(|i| q.remove(i)) {
                    break job;
                }
                // Nothing to run. Waiting on retries counts as busy; with an empty queue, end after
                // a while. The count drops under the queue lock, so a request made just after this
                // sees it and starts a new worker.
                let wake_at = q.iter().filter_map(|j| j.not_before).min();
                if !q.is_empty() {
                    idle_since = now;
                } else if now.duration_since(idle_since) >= WORKER_IDLE {
                    inner.workers.fetch_sub(1, Ordering::AcqRel);
                    return;
                }
                let wait = wake_at.map_or(WORKER_IDLE, |t| t.saturating_duration_since(now));
                q = inner.wake.wait_timeout(q, wait).unwrap_or_else(|e| e.into_inner()).0;
            }
        };
        let result = std::panic::catch_unwind(|| run(&job)).unwrap_or_else(|_| Err(Fail::Other("the renderer stopped unexpectedly".to_string())));
        if matches!(result, Err(Fail::Busy)) && job.tries < BUSY_TRIES {
            job.tries += 1;
            job.not_before = Some(Instant::now() + BUSY_RETRY);
            locked(&inner.queue).push_front(job);
            inner.wake.notify_one();
            continue;
        }
        let result = result.map_err(|e| match e {
            Fail::Busy => mdv_content::ContentError::Busy.to_string(),
            Fail::Other(m) => m,
        });
        let mut m = locked(&inner.map);
        // Only a job still wanted (not cancelled) has a Pending entry to complete.
        let entry = match result {
            Ok((w, h, baseline, image, bytes, redraw)) => {
                m.bytes += bytes;
                m.tick += 1;
                if let Kind::Sharp { base, .. } = &job.kind {
                    // The older size-matched raster has been replaced; its memory goes.
                    if let Some(old) = m.last_ready.insert(*base, job.key).filter(|o| *o != job.key) {
                        if let Some(Entry::Ready { bytes, image, .. }) = m.entries.remove(&old) {
                            m.bytes -= bytes;
                            m.retired.push(image);
                        }
                    }
                }
                Entry::Ready { w, h, baseline, image, bytes, used: m.tick, redraw }
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
        if let Some(Entry::Ready { bytes, image, .. }) = m.entries.remove(&k) {
            m.bytes -= bytes;
            m.retired.push(image);
        }
    }
}

type Done = (f32, f32, f32, Arc<RenderImage>, usize, Option<Redraw>);

enum Fail {
    Busy,
    Other(String),
}

impl From<mdv_content::ContentError> for Fail {
    fn from(e: mdv_content::ContentError) -> Self {
        match e {
            mdv_content::ContentError::Busy => Fail::Busy,
            e => Fail::Other(e.to_string()),
        }
    }
}

fn run(job: &Job) -> Result<Done, Fail> {
    let (asset, mut img, dark, redraw) = match &job.kind {
        Kind::Sharp { redraw, width, .. } => {
            let a = &redraw.asset;
            // `rasterise` rounds the width up, so aim a hair under the whole number.
            let scale = (*width as f32 - 0.001) / a.width_px;
            let img = rasterise(a, scale)?;
            (a.clone(), img, redraw.dark, None)
        }
        kind => {
            let asset: RenderedAsset = match kind {
                Kind::Math { src, display, opts } => mdv_content::render_math(src, *display, opts),
                Kind::Mermaid { src, opts } => mdv_content::render_mermaid(src, opts),
                Kind::Sharp { .. } => unreachable!(),
            }?;
            let mut scale = job.scale.clamp(0.5, 8.0);
            let px = asset.width_px * asset.height_px * scale * scale;
            if px > MAX_RASTER_PIXELS {
                scale *= (MAX_RASTER_PIXELS / px).sqrt();
            }
            let img = rasterise(&asset, scale)?;
            let dark = match kind {
                Kind::Math { opts, .. } => opts.theme == ContentTheme::Dark,
                Kind::Mermaid { opts, .. } => opts.theme == ContentTheme::Dark,
                Kind::Sharp { .. } => false,
            };
            let redraw = matches!(kind, Kind::Mermaid { .. }).then(|| Redraw { asset: Arc::new(asset.clone()), dark });
            (Arc::new(asset), img, dark, redraw)
        }
    };
    if dark {
        boost_light_on_dark(&mut img);
    }
    let mut px = img.pixels;
    // BGRA with straight (not premultiplied) alpha. GPUI's image shader outputs the sampled colour
    // unmodified and relies on the blend state (`SRC_ALPHA`, `INV_SRC_ALPHA` on Direct3D, the
    // non-premultiplied `ALPHA_BLENDING` on wgpu) to apply alpha; premultiplying here as well
    // multiplied partial pixels by alpha twice, which dimmed light strokes on dark far more than
    // dark strokes on light.
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let bytes = px.len();
    let buf = image::RgbaImage::from_raw(img.width, img.height, px).ok_or_else(|| Fail::Other("bad raster size".to_string()))?;
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
    Ok((asset.width_px, asset.height_px, asset.baseline_px, image, bytes, redraw))
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

#[cfg(test)]
mod worker_tests {
    use super::*;

    #[test]
    fn a_closed_document_frees_its_threads_and_pictures() {
        let store = ContentStore::new();
        let inner = Arc::downgrade(&store.inner);
        assert!(matches!(store.math("x^2", false, 16.0, Mode::Dark, 1.0), Lookup::Pending(_)));
        let copy = store.clone();
        drop(store);
        assert!(inner.upgrade().is_some(), "a clone still holds it");
        let t = Instant::now();
        while copy.outstanding() > 0 {
            assert!(t.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(5));
        }
        drop(copy);
        while inner.upgrade().is_some() {
            assert!(t.elapsed() < Duration::from_secs(20), "the workers kept the store alive");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn jobs_waiting_to_retry_do_not_hold_up_others() {
        let now = Instant::now();
        let math = || Kind::Math { src: String::new(), display: false, opts: MathOptions::default() };
        let diagram = || Kind::Mermaid { src: String::new(), opts: DiagramOptions::default() };
        let later = |mut j: Job| {
            j.not_before = Some(now + Duration::from_secs(1));
            j
        };
        let q: VecDeque<Job> = [Job::new(1, diagram(), 1.0), Job::new(2, diagram(), 1.0), later(Job::new(3, diagram(), 1.0))].into();
        assert_eq!(next_job(&q, now), Some(1), "newest that is ready");
        let q: VecDeque<Job> = [Job::new(1, math(), 1.0), Job::new(2, diagram(), 1.0)].into();
        assert_eq!(next_job(&q, now), Some(0), "formulas go first");
        let q: VecDeque<Job> = [later(Job::new(1, math(), 1.0)), Job::new(2, diagram(), 1.0)].into();
        assert_eq!(next_job(&q, now), Some(1), "unless they're waiting");
        let q: VecDeque<Job> = [later(Job::new(1, diagram(), 1.0))].into();
        assert_eq!(next_job(&q, now), None);
        assert_eq!(next_job(&q, now + Duration::from_secs(1)), Some(0));
    }
}
