//! Shapes text through GPUI's window text system.

use std::collections::HashMap;
use std::rc::Rc;

use gpui::{px, Font, FontFeatures, FontId, FontStyle, FontWeight, Hsla, TextRun, WindowTextSystem};

use crate::layout::{Family, FontSpec, RawGlyph, Shaper};

const SANS: &str = "Geist";
const MONO: &str = "Geist Mono";
/// Geist Serif has one face. Its bold and italic are derived from it (scripts/derive-serif.py).
const SERIF: &str = "Geist Serif";
const SERIF_DERIVED: &str = "MDV Serif";

pub struct GpuiShaper {
    ts: Rc<WindowTextSystem>,
    fonts: HashMap<FontSpec, (Font, FontId)>,
    metrics: HashMap<(FontSpec, u32), (f32, f32)>,
}

impl GpuiShaper {
    pub fn new(ts: Rc<WindowTextSystem>) -> Self {
        Self { ts, fonts: HashMap::new(), metrics: HashMap::new() }
    }

    fn font(&mut self, f: FontSpec) -> &(Font, FontId) {
        let ts = self.ts.clone();
        self.fonts.entry(f).or_insert_with(|| {
            let weight = if f.bold { FontWeight::SEMIBOLD } else { FontWeight::NORMAL };
            let family = match f.family {
                Family::Sans => SANS,
                Family::Mono => MONO,
                Family::Serif if f.bold || f.italic => SERIF_DERIVED,
                Family::Serif => SERIF,
            };
            let weight = if f.family == Family::Serif && f.bold { FontWeight::BOLD } else { weight };
            let style = if f.italic { FontStyle::Italic } else { FontStyle::Normal };
            let font = Font { family: family.into(), features: FontFeatures::default(), fallbacks: None, weight, style };
            let id = ts.resolve_font(&font);
            (font, id)
        })
    }
}

impl Shaper for GpuiShaper {
    fn shape(&mut self, text: &str, f: FontSpec, size: f32, out: &mut Vec<RawGlyph>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let font = self.font(f).0.clone();
        let run = TextRun { len: text.len(), font, color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
        let l = self.ts.layout_line(text, px(size), &[run], None);
        for r in &l.runs {
            for g in &r.glyphs {
                out.push(RawGlyph { x: f32::from(g.position.x), byte: g.index as u32, id: g.id.0, font: r.font_id.0 as u32, emoji: g.is_emoji });
            }
        }
        // Right-to-left text is not reordered by this reader's line breaking: it is a known gap.
        f32::from(l.width)
    }

    fn metrics(&mut self, f: FontSpec, size: f32) -> (f32, f32) {
        let key = (f, size.to_bits());
        if let Some(m) = self.metrics.get(&key) {
            return *m;
        }
        let id = self.font(f).1;
        let a = f32::from(self.ts.ascent(id, px(size))).abs().round();
        let d = f32::from(self.ts.descent(id, px(size))).abs().round();
        self.metrics.insert(key, (a, d));
        (a, d)
    }

}
