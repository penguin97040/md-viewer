use std::sync::{Arc, OnceLock};

use crate::{limits, ContentError, RenderedAsset};

const CHAKRA_REGULAR: &[u8] = include_bytes!("../../../../frontend/fonts/ChakraPetch-Regular.ttf");
const CHAKRA_SEMIBOLD: &[u8] = include_bytes!("../../../../frontend/fonts/ChakraPetch-SemiBold.ttf");
const SHARE_TECH_MONO: &[u8] = include_bytes!("../../../../frontend/fonts/ShareTechMono-Regular.ttf");

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
        db.load_font_data(CHAKRA_REGULAR.to_vec());
        db.load_font_data(CHAKRA_SEMIBOLD.to_vec());
        db.load_font_data(SHARE_TECH_MONO.to_vec());
        db.set_sans_serif_family("Chakra Petch");
        db.set_serif_family("Chakra Petch");
        db.set_monospace_family("Share Tech Mono");
        db.set_cursive_family("Chakra Petch");
        db.set_fantasy_family("Chakra Petch");
        Arc::new(db)
    })
    .clone()
}

/// Rasterise an asset at `scale` (device pixels per logical pixel).
pub fn rasterise(asset: &RenderedAsset, scale: f32) -> Result<RgbaImage, ContentError> {
    let err = |m: String| ContentError::Raster(m);
    if !(scale.is_finite() && scale > 0.0 && scale <= 16.0) {
        return Err(err(format!("scale {scale} out of range")));
    }
    let opt = usvg::Options {
        font_family: "Chakra Petch".into(),
        fontdb: fontdb(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(&asset.svg, &opt).map_err(|e| err(e.to_string()))?;
    let w = (tree.size().width() * scale).ceil();
    let h = (tree.size().height() * scale).ceil();
    if !(w >= 1.0 && h >= 1.0) || w > limits::MAX_RASTER_PX as f32 || h > limits::MAX_RASTER_PX as f32 {
        return Err(ContentError::TooLarge(format!("raster would be {w} x {h} px")));
    }
    let (w, h) = (w as u32, h as u32);
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h).ok_or_else(|| err("no pixmap".into()))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
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
