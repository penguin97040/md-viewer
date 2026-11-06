//! Text shaping for the layout, with no GPUI: HarfBuzz through `rustybuzz`, over the fonts in
//! [`FontSet`]. Ligatures and kerning are on; right-to-left text is not reordered (the same gap
//! the on-screen reader has).

use mdv_reader::layout::{FontSpec, RawGlyph, Shaper};

use crate::fonts::FontSet;

pub struct PdfShaper<'a> {
    pub fonts: &'a FontSet,
    /// Characters no font in use has a glyph for, counted so the caller can say so.
    pub missing: usize,
}

impl<'a> PdfShaper<'a> {
    pub fn new(fonts: &'a FontSet) -> Self {
        Self { fonts, missing: 0 }
    }
}

impl Shaper for PdfShaper<'_> {
    fn shape(&mut self, text: &str, spec: FontSpec, size: f32, out: &mut Vec<RawGlyph>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let ix = self.fonts.index(spec);
        let font = &self.fonts.fonts[ix];
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(text);
        buf.guess_segment_properties();
        let shaped = rustybuzz::shape(&font.face, &[], buf);
        let scale = size / font.upem;
        let mut cursor = 0.0f32;
        for (info, pos) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
            if info.glyph_id == 0 {
                self.missing += 1;
            }
            out.push(RawGlyph { x: cursor + pos.x_offset as f32 * scale, byte: info.cluster, id: info.glyph_id, font: ix as u32, emoji: false });
            cursor += pos.x_advance as f32 * scale;
        }
        cursor
    }

    fn metrics(&mut self, spec: FontSpec, size: f32) -> (f32, f32) {
        let font = &self.fonts.fonts[self.fonts.index(spec)];
        let scale = size / font.upem;
        ((font.face.ascender() as f32 * scale).abs().round(), (font.face.descender() as f32 * scale).abs().round())
    }
}
