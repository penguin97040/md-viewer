//! What the layout asks for that is not text: maths, diagrams and images. Everything here runs on
//! the print thread. Nothing is fetched from the network: web images stay held, as on screen.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use mdv_content::{render_math, render_mermaid, ContentTheme, DiagramOptions, MathOptions, RenderedAsset};
use mdv_reader::layout::{AssetBox, Assets, ImageState};
use mdv_reader::model::ImageSource;
use mdv_reader::paths;

const MAX_IMAGE_BYTES: u64 = 25 << 20;
const MAX_IMAGE_SIDE: u32 = 16_384;
/// Rendered SVG text kept before older entries are dropped and redrawn on demand.
const SVG_BUDGET: usize = 192 << 20;
/// Body text colour in print, `--text` in the print block of style.css.
pub const PRINT_TEXT: u32 = 0x1f2328;

#[derive(Clone, Copy)]
pub enum RasterKind {
    Png,
    Jpeg,
    Gif,
    Webp,
}

pub enum Source {
    Math { src: String, display: bool, size: f32 },
    Mermaid { src: String },
}

pub enum Item {
    Svg { source: Source, svg: Option<Arc<RenderedAsset>>, w: f32, h: f32, baseline: f32 },
    Raster { bytes: Arc<Vec<u8>>, kind: RasterKind, w: f32, h: f32 },
    Failed(String),
}

#[derive(Default)]
pub struct Store {
    pub items: HashMap<u64, Item>,
    svg_bytes: usize,
    pub failed_math: usize,
    pub failed_diagrams: usize,
    pub failed_images: usize,
    pub held_web_images: usize,
}

fn key_of<T: Hash>(v: T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

impl Store {
    /// The picture for a key, redrawing it if it was dropped to save memory.
    pub fn svg(&mut self, key: u64) -> Option<Arc<RenderedAsset>> {
        let (src_math, src_diagram) = match self.items.get(&key)? {
            Item::Svg { svg: Some(s), .. } => return Some(s.clone()),
            Item::Svg { source: Source::Math { src, display, size }, .. } => (Some((src.clone(), *display, *size)), None),
            Item::Svg { source: Source::Mermaid { src }, .. } => (None, Some(src.clone())),
            _ => return None,
        };
        let asset = match (src_math, src_diagram) {
            (Some((s, d, z)), _) => render_math(&s, d, &math_opts(z)).ok()?,
            (_, Some(s)) => render_mermaid(&s, &diagram_opts()).ok()?,
            _ => return None,
        };
        let arc = Arc::new(asset);
        self.hold(key, arc.clone());
        Some(arc)
    }

    fn hold(&mut self, key: u64, asset: Arc<RenderedAsset>) {
        if self.svg_bytes > SVG_BUDGET {
            for it in self.items.values_mut() {
                if let Item::Svg { svg, .. } = it {
                    *svg = None;
                }
            }
            self.svg_bytes = 0;
        }
        self.svg_bytes += asset.svg.len();
        if let Some(Item::Svg { svg, .. }) = self.items.get_mut(&key) {
            *svg = Some(asset);
        }
    }
}

fn math_opts(size: f32) -> MathOptions {
    MathOptions { theme: ContentTheme::Print, font_size: size * 1.1, text_color: Some(PRINT_TEXT) }
}

fn diagram_opts() -> DiagramOptions {
    DiagramOptions { theme: ContentTheme::Light, font_size: 16.0, text_color: None }
}

pub struct PrintAssets<'a> {
    pub store: &'a RefCell<Store>,
    pub document: Option<&'a Path>,
}

impl PrintAssets<'_> {
    fn svg_box(&self, key: u64, make: impl FnOnce() -> (Source, Result<RenderedAsset, String>)) -> Option<AssetBox> {
        if let Some(it) = self.store.borrow().items.get(&key) {
            return match it {
                Item::Svg { w, h, baseline, .. } => Some(AssetBox { width: *w, height: *h, baseline: *baseline, key }),
                _ => None,
            };
        }
        let (source, result) = make();
        let mut s = self.store.borrow_mut();
        match result {
            Ok(a) => {
                let (w, h, baseline) = (a.width_px, a.height_px, a.baseline_px);
                s.items.insert(key, Item::Svg { source, svg: None, w, h, baseline });
                s.hold(key, Arc::new(a));
                Some(AssetBox { width: w, height: h, baseline, key })
            }
            Err(e) => {
                match source {
                    Source::Math { .. } => s.failed_math += 1,
                    Source::Mermaid { .. } => s.failed_diagrams += 1,
                }
                s.items.insert(key, Item::Failed(e));
                None
            }
        }
    }

    fn failure(&self, key: u64) -> Option<String> {
        match self.store.borrow().items.get(&key) {
            Some(Item::Failed(e)) => Some(e.clone()),
            _ => None,
        }
    }
}

impl Assets for PrintAssets<'_> {
    fn math(&self, src: &str, display: bool, font_size: f32) -> Option<AssetBox> {
        let key = key_of(("m", src, display, font_size.to_bits()));
        self.svg_box(key, || (Source::Math { src: src.to_string(), display, size: font_size }, render_math(src, display, &math_opts(font_size)).map_err(|e| e.to_string())))
    }

    fn mermaid(&self, src: &str) -> Option<AssetBox> {
        let key = key_of(("d", src));
        self.svg_box(key, || (Source::Mermaid { src: src.to_string() }, render_mermaid(src, &diagram_opts()).map_err(|e| e.to_string())))
    }

    fn math_error(&self, src: &str, display: bool, font_size: f32) -> Option<String> {
        self.failure(key_of(("m", src, display, font_size.to_bits())))
    }

    fn mermaid_error(&self, src: &str) -> Option<String> {
        self.failure(key_of(("d", src)))
    }

    fn image(&self, source: &ImageSource) -> ImageState {
        let id = match source {
            ImageSource::Web(_) => {
                // Held, never fetched. Counted once per distinct address by the caller's totals.
                self.store.borrow_mut().held_web_images += 1;
                return ImageState::Failed;
            }
            ImageSource::Local(p) => format!("L{p}"),
            ImageSource::Data(d) => format!("D{d}"),
        };
        let key = key_of(&id);
        if let Some(it) = self.store.borrow().items.get(&key) {
            return match it {
                Item::Raster { w, h, .. } => ImageState::Ready { width: *w, height: *h, key },
                _ => ImageState::Failed,
            };
        }
        let loaded = load_image(&id, self.document);
        let mut s = self.store.borrow_mut();
        match loaded {
            Some((bytes, kind, w, h)) => {
                s.items.insert(key, Item::Raster { bytes: Arc::new(bytes), kind, w, h });
                ImageState::Ready { width: w, height: h, key }
            }
            None => {
                s.failed_images += 1;
                s.items.insert(key, Item::Failed("image could not be read".into()));
                ImageState::Failed
            }
        }
    }
}

fn load_image(id: &str, doc: Option<&Path>) -> Option<(Vec<u8>, RasterKind, f32, f32)> {
    let bytes = if let Some(href) = id.strip_prefix('L') {
        // The same check as on screen comes before anything touches the file system.
        let path = paths::resolve(doc?, href).ok()?;
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.len() > MAX_IMAGE_BYTES {
            return None;
        }
        std::fs::read(&path).ok()?
    } else {
        base64(id.strip_prefix('D')?.split_once(',')?.1)?
    };
    let reader = image::ImageReader::new(std::io::Cursor::new(&bytes)).with_guessed_format().ok()?;
    let format = reader.format()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w == 0 || h == 0 || w > MAX_IMAGE_SIDE || h > MAX_IMAGE_SIDE {
        return None;
    }
    let kind = match format {
        image::ImageFormat::Png => Some(RasterKind::Png),
        image::ImageFormat::Jpeg => Some(RasterKind::Jpeg),
        image::ImageFormat::Gif => Some(RasterKind::Gif),
        image::ImageFormat::WebP => Some(RasterKind::Webp),
        _ => None,
    };
    match kind {
        Some(k) => Some((bytes, k, w as f32, h as f32)),
        None => {
            // Anything else (BMP) is decoded once and kept as PNG.
            let img = image::load_from_memory(&bytes).ok()?;
            let mut out = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
            Some((out, RasterKind::Png, w as f32, h as f32))
        }
    }
}

fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}
