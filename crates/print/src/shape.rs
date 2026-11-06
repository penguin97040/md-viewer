//! Text shaping for the layout, with no GPUI: HarfBuzz through `rustybuzz`, over the fonts in
//! [`FontSet`]. Ligatures and kerning are on; right-to-left text is not reordered (the same gap
//! the on-screen reader has).

use mdv_reader::layout::{FontSpec, RawGlyph, Shaper};
use unicode_segmentation::UnicodeSegmentation;

use crate::fonts::FontSet;

/// How many different undrawable characters are remembered to name to the person.
const MISSING_SAMPLE: usize = 12;

pub struct PdfShaper<'a> {
    pub fonts: &'a FontSet,
    /// Characters no embedded font has a glyph for, counted so the caller can say so.
    pub missing: usize,
    /// The first few of them, each once.
    pub missing_sample: Vec<char>,
}

impl<'a> PdfShaper<'a> {
    pub fn new(fonts: &'a FontSet) -> Self {
        Self { fonts, missing: 0, missing_sample: Vec::new() }
    }

    fn note_missing(&mut self, cluster: &str) {
        self.missing += 1;
        if let Some(c) = cluster.chars().find(|c| !ignorable(*c)) {
            if self.missing_sample.len() < MISSING_SAMPLE && !self.missing_sample.contains(&c) {
                self.missing_sample.push(c);
            }
        }
    }

    /// Shapes `text` in one font and appends the glyphs, `base` bytes and `x` pixels along.
    fn shape_in(&mut self, ix: usize, text: &str, base: u32, x: f32, size: f32, out: &mut Vec<RawGlyph>) -> f32 {
        let font = self.fonts.fonts[ix].clone();
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        buf.guess_segment_properties();
        let shaped = rustybuzz::shape(&font.face, &[], buf);
        let scale = size / font.upem;
        let mut cursor = x;
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            out.push(RawGlyph { x: cursor + pos.x_offset as f32 * scale, byte: base + info.cluster, id: info.glyph_id, font: ix as u32, emoji: false });
            cursor += pos.x_advance as f32 * scale;
        }
        cursor
    }
}

/// Characters that draw nothing, so no font needs a glyph for them: variation selectors, joiners,
/// bidirectional marks and controls. Shaping drops them or gives them no width.
fn ignorable(c: char) -> bool {
    c.is_control() || matches!(c, '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{e0100}'..='\u{e01ef}')
}

impl Shaper for PdfShaper<'_> {
    fn shape(&mut self, text: &str, spec: FontSpec, size: f32, out: &mut Vec<RawGlyph>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let chain = self.fonts.chain(spec);
        let fonts = self.fonts;
        let has = |ix: usize, c: char| ignorable(c) || fonts.fonts[ix].face.glyph_index(c).is_some();
        // The usual case, and the only one where shaping across the whole text is certain to work.
        if text.chars().all(|c| has(chain[0], c)) {
            return self.shape_in(chain[0], text, 0, 0.0, size, out);
        }
        // Stretches of one font. A character and its combining marks stay in one font.
        let mut pieces: Vec<(usize, usize, usize)> = Vec::new();
        let mut lost: Vec<&str> = Vec::new();
        for (at, cluster) in text.grapheme_indices(true) {
            let ix = chain.iter().copied().find(|&f| cluster.chars().all(|c| has(f, c)));
            if ix.is_none() {
                lost.push(cluster);
            }
            let ix = ix.unwrap_or(chain[0]);
            match pieces.last_mut() {
                Some(p) if p.0 == ix => p.2 = at + cluster.len(),
                _ => pieces.push((ix, at, at + cluster.len())),
            }
        }
        for cluster in lost {
            self.note_missing(cluster);
        }
        let mut x = 0.0;
        for (ix, a, b) in pieces {
            x = self.shape_in(ix, &text[a..b], a as u32, x, size, out);
        }
        x
    }

    fn metrics(&mut self, spec: FontSpec, size: f32) -> (f32, f32) {
        let font = &self.fonts.fonts[self.fonts.index(spec)];
        let scale = size / font.upem;
        ((font.face.ascender() as f32 * scale).abs().round(), (font.face.descender() as f32 * scale).abs().round())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdv_reader::layout::Family;

    const SERIF: FontSpec = FontSpec { family: Family::Serif, bold: false, italic: false };
    const SANS: FontSpec = FontSpec { family: Family::Sans, bold: false, italic: false };

    fn shape(text: &str, spec: FontSpec) -> (Vec<RawGlyph>, usize, Vec<char>) {
        let fonts = FontSet::new();
        let mut s = PdfShaper::new(&fonts);
        let mut out = Vec::new();
        s.shape(text, spec, 16.0, &mut out);
        (out, s.missing, s.missing_sample)
    }

    #[test]
    fn serif_falls_back_for_cyrillic_and_the_footnote_arrow() {
        let fonts = FontSet::new();
        for text in ["Да", "\u{21a9}\u{fe0e}"] {
            let (g, missing, _) = shape(text, SERIF);
            assert_eq!(missing, 0, "{text}");
            assert!(g.iter().all(|g| g.font as usize == fonts.index(SANS)), "{text}");
            assert!(g.iter().any(|g| g.id != 0), "{text}");
        }
    }

    #[test]
    fn fallback_keeps_positions_and_offsets_in_order() {
        let (g, missing, _) = shape("ab Да cd", SERIF);
        assert_eq!(missing, 0);
        assert!(g.windows(2).all(|w| w[0].x <= w[1].x && w[0].byte <= w[1].byte));
        let fonts: std::collections::HashSet<u32> = g.iter().map(|g| g.font).collect();
        assert_eq!(fonts.len(), 2);
        assert_eq!(g.last().unwrap().byte as usize, "ab Да c".len());
    }

    #[test]
    fn what_no_font_has_is_counted_and_named() {
        let (_, missing, sample) = shape("a α 漢 😀 ✓ α b", SANS);
        assert_eq!(missing, 5);
        assert_eq!(sample, vec!['α', '漢', '😀', '✓']);
    }

    #[test]
    fn plain_text_stays_in_its_font_as_one_shaped_run() {
        let (g, missing, _) = shape("fi office", SANS);
        assert_eq!(missing, 0);
        assert!(g.iter().all(|g| g.font == 0));
    }
}
