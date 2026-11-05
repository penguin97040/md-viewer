//! The fonts a PDF may use. Chakra Petch and Share Tech Mono are embedded in the binary, so the
//! output never depends on what is installed. The serif setting looks for a serif face on the
//! system by file path (no font database) and falls back to Chakra Petch when there is none.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use mdv_reader::layout::{Family, FontSpec};
use rustybuzz::Face;

const CHAKRA_REGULAR: &[u8] = include_bytes!("../../../../frontend/fonts/ChakraPetch-Regular.ttf");
const CHAKRA_SEMIBOLD: &[u8] = include_bytes!("../../../../frontend/fonts/ChakraPetch-SemiBold.ttf");
const CHAKRA_ITALIC: &[u8] = include_bytes!("../../reader/fonts/ChakraPetch-Italic.ttf");
const CHAKRA_SEMIBOLD_ITALIC: &[u8] = include_bytes!("../../reader/fonts/ChakraPetch-SemiBoldItalic.ttf");
const MONO: &[u8] = include_bytes!("../../../../frontend/fonts/ShareTechMono-Regular.ttf");
const MONO_OBLIQUE: &[u8] = include_bytes!("../../reader/fonts/MDVMonoOblique.ttf");

/// Where a serif face might be, per platform: regular, bold, italic, bold italic.
const SERIF_FILES: &[[&str; 4]] = &[
    // Windows
    ["C:\\Windows\\Fonts\\georgia.ttf", "C:\\Windows\\Fonts\\georgiab.ttf", "C:\\Windows\\Fonts\\georgiai.ttf", "C:\\Windows\\Fonts\\georgiaz.ttf"],
    ["C:\\Windows\\Fonts\\times.ttf", "C:\\Windows\\Fonts\\timesbd.ttf", "C:\\Windows\\Fonts\\timesi.ttf", "C:\\Windows\\Fonts\\timesbi.ttf"],
    // macOS
    ["/System/Library/Fonts/Supplemental/Georgia.ttf", "/System/Library/Fonts/Supplemental/Georgia Bold.ttf", "/System/Library/Fonts/Supplemental/Georgia Italic.ttf", "/System/Library/Fonts/Supplemental/Georgia Bold Italic.ttf"],
    // Linux
    ["/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Italic.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSerif-BoldItalic.ttf"],
    ["/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf", "/usr/share/fonts/truetype/liberation/LiberationSerif-Bold.ttf", "/usr/share/fonts/truetype/liberation/LiberationSerif-Italic.ttf", "/usr/share/fonts/truetype/liberation/LiberationSerif-BoldItalic.ttf"],
    ["/usr/share/fonts/truetype/noto/NotoSerif-Regular.ttf", "/usr/share/fonts/truetype/noto/NotoSerif-Bold.ttf", "/usr/share/fonts/truetype/noto/NotoSerif-Italic.ttf", "/usr/share/fonts/truetype/noto/NotoSerif-BoldItalic.ttf"],
];

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
    /// Indexes into `fonts`: sans regular, bold, italic, bold italic; mono, mono italic; serif x4.
    sans: [usize; 4],
    mono: [usize; 2],
    serif: Option<[usize; 4]>,
    /// The serif setting was on but no serif face was found.
    pub serif_missing: bool,
}

fn embedded() -> &'static [Arc<Loaded>] {
    static E: OnceLock<Vec<Arc<Loaded>>> = OnceLock::new();
    E.get_or_init(|| {
        [
            (CHAKRA_REGULAR, "Chakra Petch"),
            (CHAKRA_SEMIBOLD, "Chakra Petch SemiBold"),
            (CHAKRA_ITALIC, "Chakra Petch Italic"),
            (CHAKRA_SEMIBOLD_ITALIC, "Chakra Petch SemiBold Italic"),
            (MONO, "Share Tech Mono"),
            (MONO_OBLIQUE, "Share Tech Mono Oblique"),
        ]
        .into_iter()
        .filter_map(|(d, n)| Loaded::new(d, n).map(Arc::new))
        .collect()
    })
}

/// The first serif family whose regular file can be read and parsed; missing styles use regular.
fn system_serif() -> &'static Option<Vec<Arc<Loaded>>> {
    static S: OnceLock<Option<Vec<Arc<Loaded>>>> = OnceLock::new();
    S.get_or_init(|| {
        for set in SERIF_FILES {
            let Some(reg) = read_font(set[0]) else { continue };
            let mut v = vec![reg];
            for f in &set[1..] {
                v.push(read_font(f).unwrap_or_else(|| v[0].clone()));
            }
            return Some(v);
        }
        None
    })
}

fn read_font(path: &str) -> Option<Arc<Loaded>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 40 << 20 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    // Fonts live for the life of the process, like the embedded ones.
    let data: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    Loaded::new(data, "System serif").map(Arc::new)
}

impl FontSet {
    pub fn new(serif: bool) -> Self {
        let mut fonts: Vec<Arc<Loaded>> = embedded().to_vec();
        let (mut serif_ix, mut serif_missing) = (None, false);
        if serif {
            match system_serif() {
                Some(v) => {
                    let base = fonts.len();
                    fonts.extend(v.iter().cloned());
                    serif_ix = Some([base, base + 1, base + 2, base + 3]);
                }
                None => serif_missing = true,
            }
        }
        Self { fonts, sans: [0, 1, 2, 3], mono: [4, 5], serif: serif_ix, serif_missing }
    }

    pub fn index(&self, f: FontSpec) -> usize {
        let style = usize::from(f.bold) + 2 * usize::from(f.italic);
        match f.family {
            Family::Sans => self.sans[style],
            Family::Mono => self.mono[usize::from(f.italic)],
            Family::Serif => self.serif.map_or(self.sans[style], |s| s[style]),
        }
    }
}
