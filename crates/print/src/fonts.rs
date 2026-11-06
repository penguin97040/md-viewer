//! The fonts a PDF may use, all embedded in the binary, so output never depends on what is
//! installed: Geist, Geist Mono and Geist Serif with its derived bold and italic.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use mdv_reader::layout::{Family, FontSpec};
use rustybuzz::Face;

macro_rules! font {
    ($file:literal) => {
        include_bytes!(concat!("../../../assets/fonts/", $file)).as_slice()
    };
}

/// A font file in memory, with its shaping face and a glyph-to-character table.
pub struct Loaded {
    pub data: &'static [u8],
    pub face: Face<'static>,
    pub upem: f32,
    pub name: &'static str,
    unicode: OnceLock<HashMap<u16, char>>,
}

impl Loaded {
    fn new(data: &'static [u8], name: &'static str) -> Option<Self> {
        let face = Face::from_slice(data, 0)?;
        let upem = face.units_per_em() as f32;
        Some(Self { data, face, upem, name, unicode: OnceLock::new() })
    }

    /// The character a glyph was reached from, for text a layout produced without a source offset
    /// (list markers and similar labels).
    pub fn char_of(&self, glyph: u16) -> Option<char> {
        self.unicode
            .get_or_init(|| {
                let mut m = HashMap::new();
                if let Some(cmap) = self.face.tables().cmap {
                    for st in cmap.subtables {
                        if !st.is_unicode() {
                            continue;
                        }
                        st.codepoints(|cp| {
                            if let (Some(c), Some(g)) = (char::from_u32(cp), st.glyph_index(cp)) {
                                m.entry(g.0).or_insert(c);
                            }
                        });
                    }
                }
                m
            })
            .get(&glyph)
            .copied()
    }
}

pub struct FontSet {
    pub fonts: Vec<Arc<Loaded>>,
    /// Indexes into `fonts`, each regular, bold, italic, bold italic.
    sans: [usize; 4],
    mono: [usize; 4],
    serif: [usize; 4],
}

fn embedded() -> &'static [Arc<Loaded>] {
    static E: OnceLock<Vec<Arc<Loaded>>> = OnceLock::new();
    E.get_or_init(|| {
        [
            (font!("Geist-Regular.ttf"), "Geist"),
            (font!("Geist-SemiBold.ttf"), "Geist SemiBold"),
            (font!("Geist-Italic.ttf"), "Geist Italic"),
            (font!("Geist-SemiBoldItalic.ttf"), "Geist SemiBold Italic"),
            (font!("GeistMono-Regular.ttf"), "Geist Mono"),
            (font!("GeistMono-SemiBold.ttf"), "Geist Mono SemiBold"),
            (font!("GeistMono-Italic.ttf"), "Geist Mono Italic"),
            (font!("GeistMono-SemiBoldItalic.ttf"), "Geist Mono SemiBold Italic"),
            (font!("GeistSerif-Regular.ttf"), "Geist Serif"),
            (font!("MDVSerif-Bold.ttf"), "MDV Serif Bold"),
            (font!("MDVSerif-Italic.ttf"), "MDV Serif Italic"),
            (font!("MDVSerif-BoldItalic.ttf"), "MDV Serif Bold Italic"),
        ]
        .into_iter()
        .filter_map(|(d, n)| Loaded::new(d, n).map(Arc::new))
        .collect()
    })
}

impl FontSet {
    pub fn new() -> Self {
        Self { fonts: embedded().to_vec(), sans: [0, 1, 2, 3], mono: [4, 5, 6, 7], serif: [8, 9, 10, 11] }
    }

    pub fn index(&self, f: FontSpec) -> usize {
        self.chain(f)[0]
    }

    /// The fonts to try for a character, in order. Geist Serif has the fewest characters, so it
    /// falls back to Geist, as it does on screen; Geist Mono has the most.
    pub fn chain(&self, f: FontSpec) -> [usize; 3] {
        let style = usize::from(f.bold) + 2 * usize::from(f.italic);
        match f.family {
            Family::Serif => [self.serif[style], self.sans[style], self.mono[style]],
            Family::Sans => [self.sans[style], self.mono[style], self.serif[style]],
            Family::Mono => [self.mono[style], self.sans[style], self.serif[style]],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Contours(usize);
    impl rustybuzz::ttf_parser::OutlineBuilder for Contours {
        fn move_to(&mut self, _x: f32, _y: f32) {
            self.0 += 1;
        }
        fn line_to(&mut self, _x: f32, _y: f32) {}
        fn quad_to(&mut self, _a: f32, _b: f32, _x: f32, _y: f32) {}
        fn curve_to(&mut self, _a: f32, _b: f32, _c: f32, _d: f32, _x: f32, _y: f32) {}
        fn close(&mut self) {}
    }

    /// A derived bold whose stroke cancels the glyph comes out as outlined, hollow letters.
    #[test]
    fn derived_bold_letters_are_solid() {
        let set = FontSet::new();
        for (bold, italic) in [(true, false), (true, true)] {
            let f = &set.fonts[set.index(FontSpec { family: Family::Serif, bold, italic })];
            let g = f.face.glyph_index('l').expect("l");
            let mut c = Contours(0);
            f.face.outline_glyph(g, &mut c);
            assert_eq!(c.0, 1, "{}", f.name);
        }
    }
}
