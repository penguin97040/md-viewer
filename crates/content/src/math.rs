use std::sync::Arc;

use ratex_layout::{layout, to_display_list, validate_layout, LayoutOptions};
use ratex_parser::parser::Parser;
use ratex_svg::{render_to_svg_with_color_syntax, SvgColorSyntax, SvgOptions};
use ratex_types::color::Color;
use ratex_types::display_item::DisplayItem;
use ratex_types::math_style::MathStyle;

use crate::{guard, limits, svgcheck, ContentError, MathOptions, RenderedAsset};

/// Padding around the ink, in logical pixels, so italic overhangs are not clipped.
const PAD: f64 = 1.0;

pub(crate) fn color(rgb: u32) -> Color {
    Color::rgb(((rgb >> 16) & 0xff) as f32 / 255.0, ((rgb >> 8) & 0xff) as f32 / 255.0, (rgb & 0xff) as f32 / 255.0)
}

pub(crate) fn render(src: &str, display: bool, opts: &MathOptions) -> Result<RenderedAsset, ContentError> {
    if src.len() > limits::MAX_MATH_BYTES {
        return Err(ContentError::TooLarge(format!("maths source is {} bytes (limit {})", src.len(), limits::MAX_MATH_BYTES)));
    }
    if !(opts.font_size.is_finite() && opts.font_size > 0.0 && opts.font_size <= 512.0) {
        return Err(ContentError::Parse("font size out of range".into()));
    }
    let src = crate::strip_xml_illegal(src);
    let fs = opts.font_size as f64;
    let rgb = opts.text_color.unwrap_or_else(|| opts.theme.text_color());
    guard::run(&guard::MATH, limits::MATH_BUDGET, move || {
        // Parser::new, not ratex_parser::parser::parse: the latter strips outer $ delimiters.
        let ast = Parser::new(&src).parse().map_err(|e| ContentError::Parse(e.message))?;
        let lopts = LayoutOptions {
            style: if display { MathStyle::Display } else { MathStyle::Text },
            color: color(rgb),
            ..LayoutOptions::default()
        };
        let lbox = layout(&ast, &lopts);
        validate_layout(&lbox).map_err(ContentError::TooLarge)?;
        let list = to_display_list(&lbox);
        let (w, h, d) = (list.width, list.height, list.depth);
        if !(w.is_finite() && h.is_finite() && d.is_finite()) || w < 0.0 || h < 0.0 || d < 0.0 {
            return Err(ContentError::Parse("invalid geometry".into()));
        }
        if w > limits::MAX_MATH_EM || h + d > limits::MAX_MATH_EM {
            return Err(ContentError::TooLarge(format!("formula is {w:.0} x {:.0} em", h + d)));
        }
        // KaTeX is run with maxSize: 50 em, which clamps \rule, \kern and friends. RaTeX has no such
        // setting, so oversized rules are rejected here instead of clamped.
        for it in &list.items {
            let too_big = match it {
                DisplayItem::Line { width, .. } => *width > limits::MAX_MATH_RULE_EM,
                DisplayItem::Rect { width, height, .. } => *width > limits::MAX_MATH_RULE_EM || *height > limits::MAX_MATH_RULE_EM,
                _ => false,
            };
            if too_big {
                return Err(ContentError::TooLarge(format!("rule larger than {} em", limits::MAX_MATH_RULE_EM)));
            }
        }
        let svg_opts = SvgOptions {
            font_size: fs,
            padding: PAD,
            stroke_width: 1.5 * fs / 40.0,
            embed_glyphs: true,
            font_dir: String::new(),
        };
        let svg = px_units(render_to_svg_with_color_syntax(&list, &svg_opts, SvgColorSyntax::Rgb));
        if svg.len() > limits::MAX_SVG_BYTES {
            return Err(ContentError::TooLarge(format!("SVG is {} bytes (limit {})", svg.len(), limits::MAX_SVG_BYTES)));
        }
        svgcheck::check(&svg)?;
        Ok(RenderedAsset {
            width_px: (w * fs + 2.0 * PAD) as f32,
            height_px: ((h + d) * fs + 2.0 * PAD) as f32,
            baseline_px: (h * fs + PAD) as f32,
            svg: Arc::from(svg),
        })
    })
}

/// ratex-svg writes `width="85.4pt"`, which resvg reads as 1.333 CSS px per pt and so draws 33 %
/// too large. The numbers are already logical pixels, so the unit is dropped.
fn px_units(svg: String) -> String {
    match svg.find('>') {
        Some(end) if svg.starts_with("<svg") => {
            let head = svg[..end].replace("pt\"", "\"");
            format!("{head}{}", &svg[end..])
        }
        _ => svg,
    }
}
