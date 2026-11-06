//! Images: local files, data URIs and, once the reader has agreed for this document, the web.
//! Paths go through `paths::resolve`, which refuses network and device paths, and web addresses
//! through `mdv_webimage`, which refuses anything but public addresses. Until the reader agrees, a
//! web image is only noted here, so that the bar can ask; nothing is contacted.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use gpui::RenderImage;

use crate::layout::{Assets, ImageState};
use crate::locked;
use crate::model::ImageSource;
use crate::paths;

const MAX_IMAGE_BYTES: u64 = 25 << 20;
const MAX_IMAGE_SIDE: u32 = 16_384;
/// Decoded size of one image. A small PNG can claim 16,384 x 16,384 pixels, which is 1 GB.
const MAX_DECODED_BYTES: u64 = 400 << 20;
/// Decoded images held across all tabs. Running out of memory aborts the process and so closes every
/// window, which `catch_unwind` cannot contain; past this, images show as failed instead.
const DECODED_BUDGET: u64 = 1 << 30;
/// Threads reading files and decoding. A chunk can name thousands of images.
const LOCAL_THREADS: usize = 4;
/// Images fetched at once across all tabs, as many as a browser opens to one site.
const WEB_THREADS: usize = 6;

static DECODED: AtomicU64 = AtomicU64::new(0);

/// Takes `need` bytes from `total`, unless that would pass the budget or the single-image limit.
fn reserve(total: &AtomicU64, need: u64, budget: u64) -> bool {
    need <= MAX_DECODED_BYTES && total.try_update(Ordering::SeqCst, Ordering::SeqCst, |t| (t + need <= budget).then_some(t + need)).is_ok()
}

type Job = Box<dyn FnOnce() + Send>;

/// Runs jobs on at most `max` threads, which exist only while there is work. A job that panics
/// takes only itself: see `Finish`, which still marks its image as failed.
struct Pool {
    max: usize,
    state: Mutex<PoolState>,
}

#[derive(Default)]
struct PoolState {
    queue: VecDeque<Job>,
    running: usize,
}

impl Pool {
    fn new(max: usize) -> Self {
        Self { max, state: Mutex::default() }
    }

    fn submit(self: &Arc<Self>, job: Job) {
        let mut s = locked(&self.state);
        s.queue.push_back(job);
        if s.running >= self.max {
            return;
        }
        s.running += 1;
        drop(s);
        let pool = self.clone();
        if std::thread::Builder::new().name("mdv-image".into()).spawn(move || pool.work()).is_err() {
            // No thread to be had: dropping the jobs marks their images as failed.
            let mut s = locked(&self.state);
            s.running -= 1;
            if s.running == 0 {
                let dropped = std::mem::take(&mut s.queue);
                drop(s);
                drop(dropped);
            }
        }
    }

    fn work(&self) {
        loop {
            let mut s = locked(&self.state);
            let Some(job) = s.queue.pop_front() else {
                s.running -= 1;
                return;
            };
            drop(s);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
        }
    }
}

static LOCAL_POOL: LazyLock<Arc<Pool>> = LazyLock::new(|| Arc::new(Pool::new(LOCAL_THREADS)));
static WEB_POOL: LazyLock<Arc<Pool>> = LazyLock::new(|| Arc::new(Pool::new(WEB_THREADS)));

#[derive(Clone)]
enum Entry {
    Pending,
    Ready { w: f32, h: f32, key: u64, image: Arc<RenderImage> },
    Failed,
}

/// How web images are got. The tests stand in for the network here.
pub type Fetch = Arc<dyn Fn(&str, &Arc<AtomicBool>) -> Option<Arc<mdv_webimage::Image>> + Send + Sync>;

/// Per-document image cache. A request starts a load; `finished` counts what is done.
pub struct ImageStore {
    document: Option<PathBuf>,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
    finished: Arc<AtomicU64>,
    started: Mutex<Vec<String>>,
    /// This store's share of `DECODED`, given back when the document closes.
    decoded: Arc<AtomicU64>,
    loads_begun: AtomicU64,
    /// Whether the reader has agreed to load web images. Owned by the tab, not the document, so
    /// that it survives a reload.
    consent: Arc<AtomicBool>,
    /// A web image has been held back since this store began.
    held: AtomicBool,
    /// Set when the document closes, which stops its fetches.
    closed: Arc<AtomicBool>,
    fetch: Fetch,
}

impl Drop for ImageStore {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
        DECODED.fetch_sub(self.decoded.load(Ordering::SeqCst), Ordering::SeqCst);
    }
}

/// Records how a load ended, however it ends: finished, panicked or dropped from a queue.
struct Finish {
    id: Option<String>,
    entry: Entry,
    entries: Arc<Mutex<HashMap<String, Entry>>>,
    finished: Arc<AtomicU64>,
}

impl Drop for Finish {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            locked(&self.entries).insert(id, std::mem::replace(&mut self.entry, Entry::Failed));
        }
        self.finished.fetch_add(1, Ordering::SeqCst);
    }
}

impl ImageStore {
    pub fn new(document: Option<PathBuf>, consent: Arc<AtomicBool>) -> Self {
        let fetch: Fetch = Arc::new(|url, cancel| mdv_webimage::fetch(url, cancel).ok());
        Self::with_fetch(document, consent, fetch)
    }

    pub fn with_fetch(document: Option<PathBuf>, consent: Arc<AtomicBool>, fetch: Fetch) -> Self {
        Self {
            document,
            entries: Arc::default(),
            finished: Arc::default(),
            started: Mutex::new(Vec::new()),
            decoded: Arc::default(),
            loads_begun: AtomicU64::new(0),
            consent,
            held: AtomicBool::new(false),
            closed: Arc::default(),
            fetch,
        }
    }

    pub fn finished(&self) -> u64 {
        self.finished.load(Ordering::SeqCst)
    }

    /// Loads started and not yet finished, including any requested by the layout just done.
    pub fn pending(&self) -> u64 {
        self.loads_begun.load(Ordering::SeqCst).saturating_sub(self.finished())
    }

    /// Whether this document has shown a web image that is being held back.
    pub fn holding_web_images(&self) -> bool {
        self.held.load(Ordering::SeqCst) && !self.consent.load(Ordering::SeqCst)
    }

    pub fn by_key(&self, key: u64) -> Option<Arc<RenderImage>> {
        locked(&self.entries).values().find_map(|e| match e {
            Entry::Ready { key: k, image, .. } if *k == key => Some(image.clone()),
            _ => None,
        })
    }

    /// Requests made while laying out. The view starts them.
    pub fn take_started(&self) -> Vec<String> {
        std::mem::take(&mut *locked(&self.started))
    }

    /// Loads one image in the background.
    pub fn load(&self, id: String) {
        self.loads_begun.fetch_add(1, Ordering::SeqCst);
        let doc = self.document.clone();
        let decoded = self.decoded.clone();
        let fetch = self.fetch.clone();
        let closed = self.closed.clone();
        let web = id.starts_with('W');
        let mut finish = Finish { id: Some(id), entry: Entry::Failed, entries: self.entries.clone(), finished: self.finished.clone() };
        let job = Box::new(move || {
            if closed.load(Ordering::Relaxed) {
                return;
            }
            let id = finish.id.as_deref().unwrap_or_default();
            if let Some(entry) = read_image(id, doc.as_deref(), &decoded, &fetch, &closed) {
                finish.entry = entry;
            }
        });
        if web { &*WEB_POOL } else { &*LOCAL_POOL }.submit(job);
    }
}

fn source_id(s: &ImageSource) -> String {
    match s {
        ImageSource::Local(p) => format!("L{p}"),
        ImageSource::Data(d) => format!("D{d}"),
        ImageSource::Web(u) => format!("W{u}"),
    }
}

impl Assets for ImageStore {
    fn image(&self, source: &ImageSource) -> ImageState {
        if matches!(source, ImageSource::Web(_)) && !self.consent.load(Ordering::SeqCst) {
            self.held.store(true, Ordering::SeqCst);
            return ImageState::Failed;
        }
        let id = source_id(source);
        let mut e = locked(&self.entries);
        match e.get(&id) {
            Some(Entry::Ready { w, h, key, .. }) => ImageState::Ready { width: *w, height: *h, key: *key },
            Some(Entry::Failed) => ImageState::Failed,
            Some(Entry::Pending) => ImageState::Pending,
            None => {
                e.insert(id.clone(), Entry::Pending);
                locked(&self.started).push(id);
                ImageState::Pending
            }
        }
    }
}

fn read_image(id: &str, doc: Option<&Path>, decoded: &AtomicU64, fetch: &Fetch, closed: &Arc<AtomicBool>) -> Option<Entry> {
    if let Some(url) = id.strip_prefix('W') {
        let image = fetch(url, closed)?;
        return decode(&image.data, decoded);
    }
    let bytes = if let Some(href) = id.strip_prefix('L') {
        // The security check comes before anything can touch the file system.
        let path = paths::resolve(doc?, href).ok()?;
        if !paths::is_image_path(&path) {
            return None;
        }
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > MAX_IMAGE_BYTES {
            return None;
        }
        std::fs::read(&path).ok()?
    } else {
        let d = id.strip_prefix('D')?;
        base64(d.split_once(',')?.1)?
    };
    decode(&bytes, decoded)
}

/// Whether bytes are an SVG file: markup with an `<svg` in it, near the start.
fn looks_like_svg(bytes: &[u8]) -> bool {
    let start = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let start = &start[start.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0)..];
    start.first() == Some(&b'<') && start[..start.len().min(4096)].windows(4).any(|w| w == b"<svg")
}

fn decode(bytes: &[u8], decoded: &AtomicU64) -> Option<Entry> {
    if looks_like_svg(bytes) {
        return decode_svg(bytes, decoded);
    }
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    // GPUI turns on every decoder the `image` crate has (TIFF, EXR, DDS and more). A document only
    // gets the formats on the Go reader's list, so the others are not reachable from one.
    use image::ImageFormat as F;
    if !matches!(reader.format()?, F::Png | F::Jpeg | F::Gif | F::WebP | F::Bmp | F::Ico) {
        return None;
    }
    let (claimed_w, claimed_h) = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()?;
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

fn decode_svg(bytes: &[u8], decoded: &AtomicU64) -> Option<Entry> {
    let svg = mdv_content::render_svg_image(bytes).ok()?;
    let need = svg.image.pixels.len() as u64;
    if !reserve(&DECODED, need, DECODED_BUDGET) {
        return None;
    }
    decoded.fetch_add(need, Ordering::SeqCst);
    let (width, height) = (svg.image.width, svg.image.height);
    let mut px = svg.image.pixels;
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let buf = image::RgbaImage::from_raw(width, height, px)?;
    let image = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
    let key = image.id.0 as u64;
    Some(Entry::Ready { w: svg.width_px, h: svg.height_px, key, image })
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
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    fn no_fetch() -> Fetch {
        Arc::new(|_, _| panic!("nothing may be fetched"))
    }

    fn read(id: &str, doc: Option<&Path>) -> Option<Entry> {
        read_image(id, doc, &AtomicU64::new(0), &no_fetch(), &Arc::default())
    }

    /// A one-pixel PNG.
    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        image::RgbaImage::from_pixel(1, 1, image::Rgba([1, 2, 3, 255])).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
        out
    }

    fn wait_for(store: &ImageStore, want: ImageState, source: &ImageSource) {
        let t = Instant::now();
        loop {
            let got = store.image(source);
            if matches!((got, want), (ImageState::Ready { .. }, ImageState::Ready { .. })) || got == want {
                return;
            }
            assert!(t.elapsed() < Duration::from_secs(10), "stuck at {got:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn drain(store: &ImageStore) {
        for id in store.take_started() {
            store.load(id);
        }
    }

    #[test]
    fn base64_decodes() {
        assert_eq!(base64("aGk=").unwrap(), b"hi");
        assert!(base64("a*").is_none());
    }

    #[test]
    fn network_paths_never_reach_the_file_system() {
        let doc = Path::new("/home/me/a.md");
        for p in [r"L\\attacker\share\a.png", "L//attacker/share/a.png", r"L\\?\C:\a.png", "L%5C%5Cattacker%5Cshare%5Ca.png"] {
            assert!(read(p, Some(doc)).is_none(), "{p}");
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
        assert!(read(&format!("L{}", img.display()), Some(&doc)).is_none());
        std::fs::remove_file(&img).unwrap();
    }

    #[test]
    fn nothing_is_fetched_until_the_reader_agrees() {
        let fetched = Arc::new(AtomicUsize::new(0));
        let count = fetched.clone();
        let fetch: Fetch = Arc::new(move |_, _| {
            count.fetch_add(1, Ordering::SeqCst);
            Some(Arc::new(mdv_webimage::Image { mime: "image/png".into(), data: png() }))
        });
        let consent = Arc::new(AtomicBool::new(false));
        let store = ImageStore::with_fetch(None, consent.clone(), fetch);
        let web = ImageSource::Web("https://example.com/a.png".into());

        assert!(!store.holding_web_images());
        assert_eq!(store.image(&web), ImageState::Failed);
        assert!(store.holding_web_images(), "the bar asks");
        assert!(store.take_started().is_empty());
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(fetched.load(Ordering::SeqCst), 0);

        consent.store(true, Ordering::SeqCst);
        assert!(!store.holding_web_images(), "the bar goes");
        assert_eq!(store.image(&web), ImageState::Pending);
        drain(&store);
        wait_for(&store, ImageState::Ready { width: 0.0, height: 0.0, key: 0 }, &web);
        assert_eq!(fetched.load(Ordering::SeqCst), 1);
        assert_eq!(store.pending(), 0);
    }

    #[test]
    fn consent_belongs_to_the_tab_not_the_document() {
        // A reload makes a new store. The tab's flag passes on, and a new tab starts without it.
        let consent = Arc::new(AtomicBool::new(true));
        let web = ImageSource::Web("https://example.com/a.png".into());
        let reloaded = ImageStore::with_fetch(None, consent.clone(), Arc::new(|_, _| None));
        assert_eq!(reloaded.image(&web), ImageState::Pending);
        let other_tab = ImageStore::with_fetch(None, Arc::default(), no_fetch());
        assert_eq!(other_tab.image(&web), ImageState::Failed);
    }

    #[test]
    fn a_web_image_that_fails_or_panics_shows_as_failed() {
        let consent = Arc::new(AtomicBool::new(true));
        for fetch in [Arc::new(|_: &str, _: &Arc<AtomicBool>| None) as Fetch, Arc::new(|_: &str, _: &Arc<AtomicBool>| panic!("contained"))] {
            let store = ImageStore::with_fetch(None, consent.clone(), fetch);
            let web = ImageSource::Web("https://example.com/a.png".into());
            assert_eq!(store.image(&web), ImageState::Pending);
            drain(&store);
            wait_for(&store, ImageState::Failed, &web);
            assert_eq!(store.pending(), 0, "the repaint watcher stops");
        }
    }

    #[test]
    fn something_that_is_not_an_image_fails_whatever_it_claims() {
        let fetch: Fetch = Arc::new(|_, _| Some(Arc::new(mdv_webimage::Image { mime: "image/png".into(), data: b"<html><script>alert(1)</script></html>".to_vec() })));
        let store = ImageStore::with_fetch(None, Arc::new(AtomicBool::new(true)), fetch);
        let web = ImageSource::Web("https://example.com/a.png".into());
        store.image(&web);
        drain(&store);
        wait_for(&store, ImageState::Failed, &web);
    }

    #[test]
    fn closing_the_document_cancels_its_fetches() {
        let (tx, rx) = std::sync::mpsc::channel();
        let (seen_tx, seen_rx) = std::sync::mpsc::channel::<Arc<AtomicBool>>();
        let fetch: Fetch = Arc::new(move |_, cancel| {
            seen_tx.send(cancel.clone()).unwrap();
            while !cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            tx.send(()).unwrap();
            None
        });
        let store = ImageStore::with_fetch(None, Arc::new(AtomicBool::new(true)), fetch);
        store.image(&ImageSource::Web("https://example.com/slow.png".into()));
        drain(&store);
        let _cancel = seen_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(store);
        rx.recv_timeout(Duration::from_secs(5)).expect("the fetch stopped");
    }

    #[test]
    fn svg_images_draw_at_their_own_size_and_links_go_nowhere() {
        let dir = std::env::temp_dir();
        let secret = dir.join(format!("mdv-svg-{}.png", std::process::id()));
        std::fs::write(&secret, png()).unwrap();
        let svg = format!(r#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" xmlns:x="http://example.com/x" width="10" height="5"><x:y/><rect width="10" height="5" fill="red"/><image href="{}" width="10" height="5"/></svg>"#, secret.display());
        let Entry::Ready { w, h, .. } = decode(svg.as_bytes(), &AtomicU64::new(0)).expect("drawn") else { panic!() };
        assert_eq!((w, h), (10.0, 5.0));
        std::fs::remove_file(&secret).unwrap();
        assert!(decode(b"<!DOCTYPE svg [<!ENTITY a 'b'>]><svg/>", &AtomicU64::new(0)).is_none(), "no DTD");
        assert!(decode(br#"<svg xmlns="http://www.w3.org/2000/svg" width="99999" height="99999"/>"#, &AtomicU64::new(0)).is_none());
    }

    #[test]
    fn the_pool_stays_within_its_limit_and_survives_a_panic() {
        let pool = Arc::new(Pool::new(3));
        let (running, peak, done) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        for i in 0..40 {
            let (running, peak, done) = (running.clone(), peak.clone(), done.clone());
            pool.submit(Box::new(move || {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(2));
                running.fetch_sub(1, Ordering::SeqCst);
                done.fetch_add(1, Ordering::SeqCst);
                assert!(i % 10 != 5, "contained");
            }));
        }
        let t = Instant::now();
        while done.load(Ordering::SeqCst) < 40 {
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(peak.load(Ordering::SeqCst) <= 3, "{}", peak.load(Ordering::SeqCst));
        // Every thread ends once the queue is empty.
        while locked(&pool.state).running > 0 {
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
