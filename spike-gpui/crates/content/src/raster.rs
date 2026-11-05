use std::sync::{Arc, OnceLock};

use crate::{limits, ContentError, RenderedAsset};

pub(crate) const CHAKRA_REGULAR: &[u8] = include_bytes!("../../../../frontend/fonts/ChakraPetch-Regular.ttf");
pub(crate) const CHAKRA_SEMIBOLD: &[u8] = include_bytes!("../../../../frontend/fonts/ChakraPetch-SemiBold.ttf");
pub(crate) const SHARE_TECH_MONO: &[u8] = include_bytes!("../../../../frontend/fonts/ShareTechMono-Regular.ttf");

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
