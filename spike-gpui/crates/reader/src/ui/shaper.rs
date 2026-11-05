//! Shapes text through GPUI's window text system.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;

use gpui::{px, Font, FontFeatures, FontId, FontStyle, FontWeight, Hsla, TextRun, WindowTextSystem};

use crate::layout::{Family, FontSpec, RawGlyph, Shaper};

const SANS: &str = "Chakra Petch";
const MONO: &str = "Share Tech Mono";
const MONO_OBLIQUE: &str = "MDV Mono Oblique";
/// `--serif` in style.css, followed by common Linux serif faces. The first one installed is used.
const SERIFS: [&str; 12] = ["Charter", "Bitstream Charter", "Iowan Old Style", "Georgia", "Cambria", "Noto Serif", "Times New Roman", "DejaVu Serif", "Liberation Serif", "FreeSerif", "Tinos", "Gelasio"];

fn serif_family(ts: &WindowTextSystem) -> &'static str {
    static NAME: OnceLock<&'static str> = OnceLock::new();
    NAME.get_or_init(|| {
        let names = ts.all_font_names();
        SERIFS.iter().find(|s| names.iter().any(|n| n == *s)).copied().unwrap_or(SANS)
    })
}

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
            // Share Tech Mono has one face: bold is painted, and italic has its own sheared file.
            let (family, weight) = match f.family {
                Family::Sans => (SANS, if f.bold { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }),
                Family::Serif => (serif_family(&ts), if f.bold { FontWeight::SEMIBOLD } else { FontWeight::NORMAL }),
                Family::Mono => (if f.italic { MONO_OBLIQUE } else { MONO }, FontWeight::NORMAL),
            };
            let style = if f.italic && f.family != Family::Mono { FontStyle::Italic } else { FontStyle::Normal };
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
        // Fallback runs can arrive out of byte order only for right-to-left text, which this
        // reader does not lay out. The sort keeps line breaking sound if it happens.
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

    fn fake_bold(&self, f: FontSpec) -> bool {
        f.family == Family::Mono && f.bold
    }
}
