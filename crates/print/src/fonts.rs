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
        let style = usize::from(f.bold) + 2 * usize::from(f.italic);
        match f.family {
            Family::Sans => self.sans[style],
            Family::Mono => self.mono[style],
            Family::Serif => self.serif[style],
        }
    }
}
