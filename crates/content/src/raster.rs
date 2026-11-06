use std::sync::{Arc, OnceLock};

use crate::{limits, ContentError, RenderedAsset};

pub(crate) const GEIST_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Geist-Regular.ttf");
pub(crate) const GEIST_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Geist-SemiBold.ttf");
pub(crate) const GEIST_MONO: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Regular.ttf");

/// Straight-alpha RGBA8 pixels, row-major, `width * height * 4` bytes.
#[derive(Clone, Debug)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// The only fonts resvg may use. System fonts are never loaded, so output does not depend on
/// what is installed.
pub(crate) fn fontdb() -> Arc<usvg::fontdb::Database> {
    static DB: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_font_data(GEIST_REGULAR.to_vec());
        db.load_font_data(GEIST_SEMIBOLD.to_vec());
        db.load_font_data(GEIST_MONO.to_vec());
        db.set_sans_serif_family("Geist");
        db.set_serif_family("Geist");
        db.set_monospace_family("Geist Mono");
        db.set_cursive_family("Geist");
        db.set_fantasy_family("Geist");
        Arc::new(db)
    })
    .clone()
}

/// Options for parsing an SVG. Nothing it names can leave the string itself: a linked file would
/// be read from disk by usvg's default resolver, so links resolve to nothing. Embedded images go the
/// same way: this build of resvg has no decoders for them, and one can be another whole SVG.
fn usvg_options() -> usvg::Options<'static> {
    usvg::Options {
        font_family: "Geist".into(),
        fontdb: fontdb(),
        image_href_resolver: usvg::ImageHrefResolver { resolve_data: Box::new(|_, _, _| None), resolve_string: Box::new(|_, _| None) },
        ..usvg::Options::default()
    }
}

/// An SVG image from a document or the web, drawn to pixels at twice its size for sharpness.
#[derive(Clone, Debug)]
pub struct SvgImage {
    /// Its own size in logical pixels.
    pub width_px: f32,
    pub height_px: f32,
    pub image: RgbaImage,
}

/// Elements an SVG can expand to once its `<use>` references are followed. Without this, nine levels
/// of ten references each are a few hundred bytes that unfold into a billion elements.
const MAX_SVG_ELEMENTS: u64 = 1_000_000;
/// Levels of elements and references, the XML parser's own limit.
const MAX_SVG_DEPTH: u32 = 1024;
const MAX_SVG_STYLE_REFS: usize = 512;
/// Working memory one SVG image may need at worst while drawing. Each layer resvg opens is a new
/// canvas, up to 25 times the visible one, all held at once while what's inside them draws. Failing
/// to allocate ends the process, so what the file asks for has to be worked out first.
const MAX_SVG_WORKING_BYTES: f64 = 512.0 * 1024.0 * 1024.0;
/// Below this scale the picture is no use, and it's asking for far too much anyway.
const MIN_SVG_SCALE: f32 = 0.05;

/// The ids an attribute value points at: `url(#id)` in any property, and `#id` in a link.
fn references<'a>(a: &roxmltree::Attribute<'a, '_>, out: &mut Vec<&'a str>) {
    let v = a.value();
    if a.name() == "href" {
        out.extend(v.strip_prefix('#'));
    }
    let mut rest = v;
    while let Some(i) = rest.find("url(") {
        rest = &rest[i + 4..];
        let id = rest.trim_start().trim_start_matches(['"', '\'']);
        if let Some(id) = id.strip_prefix('#') {
            out.push(&id[..id.find(['"', '\'', ')', ' ']).unwrap_or(id.len())]);
        }
    }
}

/// Refuses an SVG whose references unfold into too many elements or run too deep, before usvg walks
/// them. That walk recurses, through `<use>`, masks, clip paths, patterns, gradients and more, and a
/// stack overflow ends the process. A cycle counts for nothing here; usvg refuses those itself.
fn check_references(text: &str) -> Result<(), ContentError> {
    use std::collections::HashMap;
    type Ids<'a> = HashMap<&'a str, roxmltree::NodeId>;
    struct Walk<'a, 'i> {
        doc: &'a roxmltree::Document<'i>,
        ids: Ids<'a>,
        /// Per node, the elements it unfolds into and the levels it runs to.
        memo: HashMap<roxmltree::NodeId, (u64, u32)>,
        too_deep: bool,
    }
    impl Walk<'_, '_> {
        fn visit(&mut self, n: roxmltree::Node, depth: u32) -> (u64, u32) {
            if depth > MAX_SVG_DEPTH {
                self.too_deep = true;
                return (0, 0);
            }
            if let Some(known) = self.memo.get(&n.id()) {
                return *known;
            }
            self.memo.insert(n.id(), (0, 0));
            let (mut count, mut levels) = (1u64, 0u32);
            let mut targets = Vec::new();
            for a in n.attributes() {
                references(&a, &mut targets);
            }
            let refs: Vec<roxmltree::NodeId> = targets.iter().filter_map(|t| self.ids.get(t).copied()).collect();
            let next = n.children().filter(|c| c.is_element()).chain(refs.iter().filter_map(|id| self.doc.get_node(*id)));
            for c in next {
                let (cc, cl) = self.visit(c, depth + 1);
                count = count.saturating_add(cc);
                levels = levels.max(cl);
            }
            let result = (count, levels.saturating_add(1));
            self.memo.insert(n.id(), result);
            result
        }
    }
    let opt = roxmltree::ParsingOptions { allow_dtd: false, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(text, opt).map_err(|e| ContentError::Raster(e.to_string()))?;
    // A style sheet's references aren't followed here. Keeping them few keeps their depth small.
    let style_refs = doc.descendants().filter(|n| n.has_tag_name("style")).filter_map(|n| n.text()).map(|t| t.matches("url(").count()).sum::<usize>();
    if style_refs > MAX_SVG_STYLE_REFS {
        return Err(ContentError::TooLarge(format!("SVG style sheet has {style_refs} references")));
    }
    let ids: Ids = doc.descendants().filter(|n| n.is_element()).filter_map(|n| Some((n.attribute("id")?, n.id()))).collect();
    let mut walk = Walk { doc: &doc, ids, memo: HashMap::new(), too_deep: false };
    let (count, levels) = walk.visit(doc.root_element(), 0);
    if walk.too_deep || levels > MAX_SVG_DEPTH {
        return Err(ContentError::TooLarge("SVG is nested too deeply".into()));
    }
    if count > MAX_SVG_ELEMENTS {
        return Err(ContentError::TooLarge(format!("SVG unfolds into {count} elements")));
    }
    Ok(())
}

/// What a parsed SVG will ask of the renderer. Definitions are shared, so the walk is bounded by the
/// visit count, not by the shape of the file.
#[derive(Default)]
struct Demand {
    visits: u64,
    /// The deepest stack of canvases held at once, in canvases.
    canvases: u64,
    /// Pixels in pattern tiles, which aren't limited to the canvas, at scale 1.
    tiles: f64,
}

impl Demand {
    fn group(&mut self, g: &usvg::Group, depth: u64, ts: usvg::Transform) -> Result<(), ()> {
        self.visits += 1;
        if self.visits > MAX_SVG_ELEMENTS {
            return Err(());
        }
        let layered = g.opacity() != usvg::Opacity::ONE
            || g.clip_path().is_some()
            || g.mask().is_some()
            || !g.filters().is_empty()
            || g.blend_mode() != usvg::BlendMode::Normal
            || g.isolate();
        let primitives: u64 = g.filters().iter().map(|f| f.primitives().len() as u64 + 1).sum();
        let depth = depth + layered as u64 + primitives;
        self.canvases = self.canvases.max(depth);
        let mut clip = g.clip_path();
        let mut inner = depth;
        while let Some(c) = clip {
            inner += 1;
            self.group(c.root(), inner, ts)?;
            clip = c.clip_path();
        }
        let mut mask = g.mask();
        while let Some(m) = mask {
            inner += 1;
            self.group(m.root(), inner, ts)?;
            mask = m.mask();
        }
        self.canvases = self.canvases.max(inner);
        for n in g.children() {
            match n {
                usvg::Node::Group(c) => self.group(c, depth, ts)?,
                usvg::Node::Path(p) => {
                    let (fill, stroke) = (p.fill().map(|f| f.paint()), p.stroke().map(|s| s.paint()));
                    for paint in [fill, stroke].into_iter().flatten() {
                        if let usvg::Paint::Pattern(pattern) = paint {
                            self.pattern(pattern, depth, ts.pre_concat(p.abs_transform()))?;
                        }
                    }
                }
                usvg::Node::Text(t) => self.group(t.flattened(), depth, ts)?,
                usvg::Node::Image(_) => {}
            }
        }
        Ok(())
    }

    fn pattern(&mut self, p: &usvg::Pattern, depth: u64, ts: usvg::Transform) -> Result<(), ()> {
        let ts = ts.pre_concat(p.transform());
        let (sx, sy) = ts.get_scale();
        let px = (p.rect().width() * sx) as f64 * (p.rect().height() * sy) as f64;
        self.tiles += px;
        if !px.is_finite() || !self.tiles.is_finite() {
            return Err(());
        }
        // A tile is drawn in its own space at its own scale.
        self.group(p.root(), depth + 1, usvg::Transform::from_scale(sx, sy))
    }

    /// The scale to draw at, at most `wanted`, so that the worst case fits; `None` if nothing does.
    fn scale(&self, wanted: f32, w: f32, h: f32) -> Option<f32> {
        let scale = wanted.min((limits::MAX_SVG_IMAGE_PIXELS / (w * h)).sqrt()).min((limits::MAX_RASTER_PX as f32 - 1.0) / w.max(h));
        let canvas_px = (w * h) as f64;
        let per_px = 4.0 * (1.0 + 25.0 * self.canvases as f64);
        let fit = (MAX_SVG_WORKING_BYTES / (canvas_px * per_px)).sqrt() as f32;
        let scale = scale.min(fit);
        // Tiles grow with the square of the scale too.
        let tiles = 4.0 * self.tiles * (scale * scale) as f64;
        (tiles < MAX_SVG_WORKING_BYTES && scale >= MIN_SVG_SCALE).then_some(scale)
    }
}

/// Draws an SVG image the way a browser's `<img>` does: no script, no links, nothing outside the
/// file. Unknown elements are ignored, so files from drawing tools still work, which is why this
/// doesn't use the allow-list in `svgcheck`; that guards what this crate hands to the PDF writer.
/// Runs on a worker thread with a time budget, because drawing time is the document's choice.
pub fn render_svg_image(svg: &[u8]) -> Result<SvgImage, ContentError> {
    const SVG_SCALE: f32 = 2.0;
    let bad = |m: &str| Err(ContentError::Unsafe(m.to_string()));
    if svg.len() > limits::MAX_SVG_IMAGE_BYTES {
        return Err(ContentError::TooLarge(format!("SVG is {} bytes", svg.len())));
    }
    let Ok(text) = std::str::from_utf8(svg) else { return bad("not UTF-8") };
    let lower = text.to_ascii_lowercase();
    if lower.contains("<!doctype") || lower.contains("<!entity") {
        return bad("DTD or entity declaration");
    }
    let text = text.to_string();
    crate::guard::run(&crate::guard::MERMAID, limits::MERMAID_BUDGET, move || {
        check_references(&text)?;
        let tree = usvg::Tree::from_str(&text, &usvg_options()).map_err(|e| ContentError::Raster(e.to_string()))?;
        let (w, h) = (tree.size().width(), tree.size().height());
        if w > limits::MAX_RASTER_PX as f32 || h > limits::MAX_RASTER_PX as f32 {
            return Err(ContentError::TooLarge(format!("SVG is {w:.0} x {h:.0} px")));
        }
        let mut demand = Demand::default();
        let too_big = || ContentError::TooLarge("SVG asks for too much memory to draw".into());
        demand.group(tree.root(), 0, usvg::Transform::default()).map_err(|()| too_big())?;
        let scale = demand.scale(SVG_SCALE, w, h).ok_or_else(too_big)?;
        let image = draw(&tree, scale)?;
        Ok(SvgImage { width_px: w, height_px: h, image })
    })
}

/// Rasterise an asset at `scale` (device pixels per logical pixel).
pub fn rasterise(asset: &RenderedAsset, scale: f32) -> Result<RgbaImage, ContentError> {
    let err = |m: String| ContentError::Raster(m);
    if !(scale.is_finite() && scale > 0.0 && scale <= 16.0) {
        return Err(err(format!("scale {scale} out of range")));
    }
    let tree = usvg::Tree::from_str(&asset.svg, &usvg_options()).map_err(|e| err(e.to_string()))?;
    draw(&tree, scale)
}

fn draw(tree: &usvg::Tree, scale: f32) -> Result<RgbaImage, ContentError> {
    let err = |m: String| ContentError::Raster(m);
    let w = (tree.size().width() * scale).ceil();
    let h = (tree.size().height() * scale).ceil();
    if !(w >= 1.0 && h >= 1.0) || w > limits::MAX_RASTER_PX as f32 || h > limits::MAX_RASTER_PX as f32 {
        return Err(ContentError::TooLarge(format!("raster would be {w} x {h} px")));
    }
    if w as f64 * h as f64 > limits::MAX_RASTER_PIXELS as f64 {
        return Err(ContentError::TooLarge(format!("raster would be {w} x {h} px")));
    }
    let (w, h) = (w as u32, h as u32);
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h).ok_or_else(|| err("no pixmap".into()))?;
    resvg::render(tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    let mut pixels = pm.take();
    for px in pixels.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Ok(RgbaImage { width: w, height: h, pixels })
}

/// Strength of the light-on-dark boost: the exponent applied to partial coverage drops by this
/// much for a pure white pixel (see `boost_light_on_dark`).
pub const LIGHT_ON_DARK_BOOST: f32 = 0.2;

/// Thickens light strokes that will be blended over a dark background.
///
/// The GPU blends rasters in gamma-encoded sRGB, so a half-covered pixel of a light stroke on black
/// carries less than half of the light a viewer expects, while the same stroke dark on white
/// carries more. Light-on-dark text therefore looks thinner and dimmer than the same glyphs in the
/// light scheme. Browsers and GPUI's own text correct for this with a contrast boost; this does the
/// same for rasters, raising partial coverage by `alpha^(1 - k * luma)`. Opaque and transparent
/// pixels are unchanged, and dark pixels (a diagram's node fills) hardly move. `rgba` is straight
/// alpha, as `rasterise` returns it.
pub fn boost_light_on_dark(img: &mut RgbaImage) {
    // 256-entry table per luma step would be exact; the exponent varies slowly, so 16 steps do.
    let mut table = [[0u8; 256]; 17];
    for (step, row) in table.iter_mut().enumerate() {
        let e = 1.0 - LIGHT_ON_DARK_BOOST * (step as f32 / 16.0);
        for (a, out) in row.iter_mut().enumerate() {
            *out = ((a as f32 / 255.0).powf(e) * 255.0).round().min(255.0) as u8;
        }
    }
    for px in img.pixels.chunks_exact_mut(4) {
        let a = px[3];
        if a == 0 || a == 255 {
            continue;
        }
        let luma = (px[0] as u32 * 2126 + px[1] as u32 * 7152 + px[2] as u32 * 722) / 10_000;
        px[3] = table[((luma * 16 + 127) / 255) as usize][a as usize];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boost_raises_light_partial_coverage_only() {
        let mut img = RgbaImage { width: 4, height: 1, pixels: vec![255, 255, 255, 128, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 0, 128] };
        boost_light_on_dark(&mut img);
        assert!(img.pixels[3] > 128 && img.pixels[3] < 255, "white partial rises: {}", img.pixels[3]);
        assert_eq!(img.pixels[7], 255);
        assert_eq!(img.pixels[11], 0);
        assert_eq!(img.pixels[15], 128, "black is untouched");
        assert_eq!(&img.pixels[0..3], &[255, 255, 255], "colour is never changed");
    }
}
