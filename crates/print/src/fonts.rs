//! The fonts a PDF may use. Geist, Geist Mono and Geist Serif (with its derived bold and italic)
//! are embedded in the binary, so ordinary output never depends on what is installed. Characters
//! they lack are looked up in the system's fonts by [`crate::system`], and only then.

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
    /// The face within a font collection (`.ttc`); 0 for a plain font file.
    pub index: u32,
    pub face: Face<'static>,
    pub upem: f32,
    pub name: String,
    unicode: OnceLock<HashMap<u16, char>>,
}

impl Loaded {
    pub fn new(data: &'static [u8], index: u32, name: String) -> Option<Self> {
        let face = Face::from_slice(data, index)?;
        let upem = face.units_per_em() as f32;
        if upem <= 0.0 {
            return None;
        }
        Some(Self { data, index, face, upem, name, unicode: OnceLock::new() })
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
    /// The fonts to try for each of twelve styles (family, then regular, bold, italic, bold
    /// italic), in order: the embedded three, then any system fonts added for this document.
    chains: [Vec<usize>; 12],
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
        .filter_map(|(d, n)| Loaded::new(d, 0, n.to_string()).map(Arc::new))
        .collect()
    })
}

/// Chain slot of a style: family (sans, mono, serif) times four for bold and italic.
fn slot(f: FontSpec) -> usize {
    let family = match f.family {
        Family::Sans => 0,
        Family::Mono => 1,
        Family::Serif => 2,
    };
    family * 4 + usize::from(f.bold) + 2 * usize::from(f.italic)
}

impl FontSet {
    pub fn new() -> Self {
        // Geist Serif has the fewest characters, so it falls back to Geist, as it does on screen;
        // Geist Mono has the most.
        let chains = std::array::from_fn(|i| {
            let (family, style) = (i / 4, i % 4);
            let (sans, mono, serif) = (style, 4 + style, 8 + style);
            match family {
                0 => vec![sans, mono, serif],
                1 => vec![mono, sans, serif],
                _ => vec![serif, sans, mono],
            }
        });
        Self { fonts: embedded().to_vec(), chains }
    }

    /// This set with system fonts added after the embedded ones. `extra` holds the fonts and, for
    /// each of the twelve styles, which of them to try, in order (indexes into `extra`).
    pub fn with_system(&self, extra: Vec<Arc<Loaded>>, per_style: &[Vec<usize>; 12]) -> Self {
        let base = self.fonts.len();
        let mut fonts = self.fonts.clone();
        fonts.extend(extra);
        let chains = std::array::from_fn(|i| {
            let mut c = self.chains[i].clone();
            c.extend(per_style[i].iter().filter(|&&e| base + e < fonts.len()).map(|e| base + e));
            c
        });
        Self { fonts, chains }
    }

    pub fn index(&self, f: FontSpec) -> usize {
        self.chain(f)[0]
    }

    /// The fonts to try for a character, in order.
    pub fn chain(&self, f: FontSpec) -> &[usize] {
        &self.chains[slot(f)]
    }

    /// Whether any font in the set draws `c`.
    pub fn has_any(&self, c: char) -> bool {
        self.fonts.iter().any(|f| f.face.glyph_index(c).is_some())
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
