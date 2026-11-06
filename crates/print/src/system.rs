//! Fonts from the machine, for characters the embedded Geist fonts lack (Greek, CJK, symbols,
//! emoji). Nothing here runs for a document the embedded fonts can draw: the system's fonts are
//! scanned the first time a document needs them, once per process, on the thread that renders.
//!
//! A document only decides which characters are wanted. It never names a font or a path: faces
//! are chosen from what `fontdb` found, by coverage of those characters and then by how well
//! they match the style asked for. Every chosen face is test-embedded first, so one that cannot
//! be parsed or subset is skipped instead of failing the print.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, OnceLock};

use krilla::geom::Point;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla::Document as KDocument;
use mdv_reader::layout::Family;

use crate::fonts::{FontSet, Loaded};

/// The largest font file looked at. Files are memory-mapped, so this bounds work, not memory.
const MAX_FONT_BYTES: usize = 512 * 1024 * 1024;
/// The most system fonts added for one style.
const MAX_PER_STYLE: usize = 6;
/// The most faces examined, so a machine with a huge font folder still answers quickly.
const MAX_FACES: usize = 6000;
/// Weight of the coverage count against style fit. Style fit stays well inside this.
const COVERAGE: i32 = 1000;

/// What the shaper needs from a face to choose it, with the font itself left out so the choice
/// can be tested with invented fonts.
#[derive(Clone, Debug)]
pub struct Cand {
    pub key: usize,
    pub weight: u16,
    pub italic: bool,
    pub mono: bool,
    pub serif: bool,
    /// A variable font. Only its default instance is drawn, so it suits regular text best.
    pub variable: bool,
    /// Has colour glyphs (COLR, sbix, CBDT or SVG): emoji fonts.
    pub colour: bool,
    /// Which of the wanted characters it draws.
    pub covers: Vec<char>,
    /// How many glyphs the font has. Catch-all fonts (CJK, Unifont) lose ties to focused ones.
    pub glyphs: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct Want {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
}

/// Characters drawn as colour pictures by default. They prefer a colour font; everything else
/// prefers a plain one, so a tick does not arrive from an emoji font.
pub fn is_emoji(c: char) -> bool {
    matches!(c as u32, 0x1F000..=0x1FAFF | 0x2B50 | 0x2B55 | 0x2705 | 0x274C | 0x274E | 0x2728 | 0x26A1 | 0x231A | 0x231B | 0x23E9..=0x23F3)
}

fn style_fit(c: &Cand, w: Want, colour_wanted: bool) -> i32 {
    let target = if w.bold { 700 } else { 400 };
    let mut s = 100 - (c.weight as i32 - target).abs() / 10;
    if c.italic == w.italic {
        s += 30;
    }
    match w.family {
        Family::Mono => s += if c.mono { 60 } else { 0 },
        Family::Sans => s += if c.mono { -20 } else { 0 } + if c.serif { 0 } else { 20 },
        Family::Serif => s += if c.serif { 20 } else { 0 } + if c.mono { -20 } else { 0 },
    }
    if c.variable && (w.bold || w.italic) {
        s -= 10;
    }
    s -= (c.glyphs / 2000).min(40) as i32;
    s += if c.colour == colour_wanted { 40 } else { -40 };
    s
}

/// A coarse script or symbol group, so each group gets the face that suits it: Greek from a
/// proper Greek face rather than whichever CJK face happens to carry it too.
fn group(c: char) -> u32 {
    let u = c as u32;
    if is_emoji(c) {
        return 0;
    }
    match u {
        0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xAC00..=0xD7FF => 1,
        0x2E80..=0x33FF | 0x3400..=0x9FFF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF | 0x20000..=0x3FFFF => 2,
        0x370..=0x3FF | 0x1F00..=0x1FFF => 3,
        0x2000..=0x2BFF => 4,
        _ if u < 0x2000 => 0x100 + (u >> 8),
        _ => 0x1000 + (u >> 12),
    }
}

/// Picks the candidate keys to try, in order, for one style. Each group of characters (see
/// [`group`]) is served greedily: the face drawing the most of its characters still wanted, style
/// fit breaking ties, until all are drawn or `limit` faces are chosen for the group. Emoji prefer
/// colour fonts and everything else plain ones.
pub fn choose(cands: &[Cand], wanted: &[char], w: Want, limit: usize, skip: &HashSet<usize>) -> Vec<usize> {
    let mut groups: Vec<(u32, Vec<char>)> = Vec::new();
    for &c in wanted {
        let g = group(c);
        match groups.iter_mut().find(|x| x.0 == g) {
            Some(x) => x.1.push(c),
            None => groups.push((g, vec![c])),
        }
    }
    // The big CJK faces come last, so they only draw what no smaller face does.
    groups.sort_by_key(|g| if matches!(g.0, 1 | 2) { u32::MAX - g.0 } else { g.0 });
    let mut chosen: Vec<usize> = Vec::new();
    for (g, mut left) in groups {
        let mut n = 0;
        while n < limit && !left.is_empty() {
            let best = cands
                .iter()
                .filter(|c| !skip.contains(&c.key))
                .map(|c| (c, left.iter().filter(|p| c.covers.contains(p)).count() as i32))
                .filter(|(_, n)| *n > 0)
                .max_by_key(|(c, n)| (n * COVERAGE + style_fit(c, w, g == 0), std::cmp::Reverse(c.key)));
            let Some((c, _)) = best else { break };
            if !chosen.contains(&c.key) {
                chosen.push(c.key);
            }
            left.retain(|l| !c.covers.contains(l));
            n += 1;
        }
    }
    chosen
}

fn db() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

fn looks_serif(name: &str) -> bool {
    let n = name.to_lowercase();
    (n.contains("serif") && !n.contains("sans"))
        || ["mincho", "song", "ming", "times", "georgia", "garamond", "palatino", "cambria", "batang", "myeongjo", "simsun", "minion", "charter", "bookman", "roman"].iter().any(|k| n.contains(k))
}

/// Placeholder faces (macOS ships LastResort, whose cmap covers every code point but only draws a
/// box naming the Unicode block) must never count as drawing a character.
fn is_placeholder(name: &str) -> bool {
    let n: String = name.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').flat_map(char::to_lowercase).collect();
    n.starts_with("lastresort")
}

/// Every face on the machine that draws at least one wanted character.
fn candidates(wanted: &[char]) -> Vec<(fontdb::ID, Cand)> {
    let db = db();
    let mut out = Vec::new();
    for (key, info) in db.faces().take(MAX_FACES).enumerate() {
        let id = info.id;
        if is_placeholder(&info.post_script_name) || info.families.iter().any(|f| is_placeholder(&f.0)) {
            continue;
        }
        let probe = catch_unwind(AssertUnwindSafe(|| {
            db.with_face_data(id, |data, index| {
                if data.len() > MAX_FONT_BYTES {
                    return None;
                }
                let face = rustybuzz::ttf_parser::Face::parse(data, index).ok()?;
                let covers: Vec<char> = wanted.iter().copied().filter(|c| face.glyph_index(*c).is_some_and(|g| g.0 != 0)).collect();
                let t = face.tables();
                let colour = t.colr.is_some() || t.sbix.is_some() || t.cbdt.is_some() || t.svg.is_some();
                Some((covers, face.is_variable(), colour, face.number_of_glyphs() as u32))
            })
            .flatten()
        }));
        let Ok(Some((covers, variable, colour, glyphs))) = probe else { continue };
        if covers.is_empty() {
            continue;
        }
        let name = info.families.first().map(|f| f.0.as_str()).unwrap_or("");
        let italic = !matches!(info.style, fontdb::Style::Normal);
        out.push((id, Cand { key, weight: info.weight.0, italic, mono: info.monospaced, serif: looks_serif(name), variable, colour, covers, glyphs }));
    }
    out
}

/// Loads a face for use (its data lives for the rest of the process, as the embedded fonts do),
/// and checks it can be embedded with the characters it was chosen for. `None` if not.
fn load(id: fontdb::ID, covers: &[char]) -> Option<Arc<Loaded>> {
    static CACHE: OnceLock<Mutex<HashMap<fontdb::ID, Option<Arc<Loaded>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok()?.get(&id) {
        return hit.clone();
    }
    let made = catch_unwind(AssertUnwindSafe(|| {
        let (source, index) = db().face_source(id)?;
        // Memory-mapped by the scan, so a large collection costs address space, not memory.
        let data: &'static [u8] = match source {
            fontdb::Source::Binary(a) | fontdb::Source::SharedFile(_, a) => {
                let a: &'static Arc<dyn AsRef<[u8]> + Send + Sync> = Box::leak(Box::new(a));
                (**a).as_ref()
            }
            fontdb::Source::File(path) => {
                if std::fs::metadata(&path).ok()?.len() > MAX_FONT_BYTES as u64 {
                    return None;
                }
                Box::leak(std::fs::read(path).ok()?.into_boxed_slice())
            }
        };
        if data.len() > MAX_FONT_BYTES {
            return None;
        }
        let name = db().face(id).and_then(|i| i.families.first().map(|f| f.0.clone())).unwrap_or_else(|| "system font".into());
        let font = Arc::new(Loaded::new(data, index, name)?);
        embeds(&font, covers).then_some(font)
    }))
    .ok()
    .flatten();
    if let Ok(mut c) = cache.lock() {
        c.insert(id, made.clone());
    }
    made
}

/// Embeds `chars` from `font` in a throwaway document, which is where subsetting would fail.
fn embeds(font: &Loaded, chars: &[char]) -> bool {
    let Some(kfont) = Font::new(font.data.into(), font.index) else { return false };
    let mut text = String::new();
    let mut glyphs = Vec::new();
    for &c in chars.iter().take(64) {
        let Some(g) = font.face.glyph_index(c) else { continue };
        let start = text.len();
        text.push(c);
        glyphs.push(KrillaGlyph::new(GlyphId::new(g.0 as u32), 1.0, 0.0, 0.0, 0.0, start..text.len(), None));
    }
    if glyphs.is_empty() {
        return false;
    }
    catch_unwind(AssertUnwindSafe(|| {
        let mut doc = KDocument::new();
        let mut page = doc.start_page();
        let mut s = page.surface();
        s.draw_glyphs(Point::from_xy(10.0, 30.0), &glyphs, kfont, &text, 12.0, false);
        s.finish();
        page.finish();
        doc.finish().is_ok()
    }))
    .unwrap_or(false)
}

type Plan = Arc<(Vec<Arc<Loaded>>, [Vec<usize>; 12])>;

/// The system fonts for `wanted`, for all twelve styles. Cached for the last few requests.
fn plan(wanted: &[char]) -> Plan {
    static RECENT: Mutex<Vec<(Vec<char>, Plan)>> = Mutex::new(Vec::new());
    let mut key = wanted.to_vec();
    key.sort_unstable();
    key.dedup();
    if let Ok(r) = RECENT.lock() {
        if let Some((_, p)) = r.iter().find(|(k, _)| *k == key) {
            return p.clone();
        }
    }
    let cands = candidates(&key);
    let plain: Vec<Cand> = cands.iter().map(|(_, c)| c.clone()).collect();
    let ids: HashMap<usize, fontdb::ID> = cands.iter().map(|(id, c)| (c.key, *id)).collect();
    let mut fonts: Vec<Arc<Loaded>> = Vec::new();
    let mut slot_of: HashMap<usize, usize> = HashMap::new();
    let mut bad: HashSet<usize> = HashSet::new();
    let mut styles: [Vec<usize>; 12] = Default::default();
    for (i, style) in styles.iter_mut().enumerate() {
        let w = Want { family: [Family::Sans, Family::Mono, Family::Serif][i / 4], bold: i % 2 == 1, italic: i % 4 >= 2 };
        for _ in 0..8 {
            let picks = choose(&plain, &key, w, MAX_PER_STYLE, &bad);
            let mut failed = false;
            style.clear();
            for k in picks {
                if let Some(&s) = slot_of.get(&k) {
                    style.push(s);
                    continue;
                }
                let covers = &plain.iter().find(|c| c.key == k).map(|c| c.covers.clone()).unwrap_or_default();
                match load(ids[&k], covers) {
                    Some(f) => {
                        slot_of.insert(k, fonts.len());
                        style.push(fonts.len());
                        fonts.push(f);
                    }
                    None => {
                        bad.insert(k);
                        failed = true;
                    }
                }
            }
            if !failed {
                break;
            }
        }
    }
    let p: Plan = Arc::new((fonts, styles));
    if let Ok(mut r) = RECENT.lock() {
        r.truncate(3);
        r.insert(0, (key, p.clone()));
    }
    p
}

impl FontSet {
    /// This set plus the system fonts that draw `wanted`. `None` if there are none to add.
    pub fn with_system_for(&self, wanted: &[char]) -> Option<FontSet> {
        if wanted.is_empty() {
            return None;
        }
        let p = plan(wanted);
        if p.0.is_empty() {
            return None;
        }
        Some(self.with_system(p.0.clone(), &p.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(key: usize, covers: &str) -> Cand {
        Cand { key, weight: 400, italic: false, mono: false, serif: false, variable: false, colour: false, covers: covers.chars().collect(), glyphs: 0 }
    }
    const SANS: Want = Want { family: Family::Sans, bold: false, italic: false };

    fn pick(c: &[Cand], wanted: &str, w: Want) -> Vec<usize> {
        choose(c, &wanted.chars().collect::<Vec<_>>(), w, 6, &HashSet::new())
    }

    #[test]
    fn the_face_covering_most_wins_and_the_rest_fill_in() {
        let c = [cand(0, "漢字"), cand(1, "漢字語"), cand(2, "한")];
        assert_eq!(pick(&c, "漢字語한", SANS), vec![1, 2]);
    }

    #[test]
    fn each_script_gets_its_own_face_and_focused_faces_win_ties() {
        let mut cjk = cand(0, "αβ漢");
        cjk.glyphs = 40000;
        let greek = cand(1, "αβ");
        assert_eq!(pick(&[cjk, greek], "αβ漢", SANS), vec![1, 0]);
    }

    #[test]
    fn style_decides_between_equal_coverage() {
        let mut regular = cand(0, "α");
        let mut bold = cand(1, "α");
        bold.weight = 700;
        let mut italic = cand(2, "α");
        italic.italic = true;
        let mut mono = cand(3, "α");
        mono.mono = true;
        let mut serif = cand(4, "α");
        serif.serif = true;
        regular.weight = 400;
        let c = [regular, bold, italic, mono, serif];
        assert_eq!(pick(&c, "α", SANS)[0], 0);
        assert_eq!(pick(&c, "α", Want { bold: true, ..SANS })[0], 1);
        assert_eq!(pick(&c, "α", Want { italic: true, ..SANS })[0], 2);
        assert_eq!(pick(&c, "α", Want { family: Family::Mono, ..SANS })[0], 3);
        assert_eq!(pick(&c, "α", Want { family: Family::Serif, ..SANS })[0], 4);
    }

    #[test]
    fn variable_fonts_lose_ties_when_bold_or_italic() {
        let mut v = cand(0, "α");
        v.variable = true;
        v.weight = 700;
        let mut fixed = cand(1, "α");
        fixed.weight = 700;
        assert_eq!(pick(&[v, fixed], "α", Want { bold: true, ..SANS })[0], 1);
    }

    #[test]
    fn emoji_prefer_colour_and_symbols_avoid_it() {
        let mut colour = cand(0, "😀✓");
        colour.colour = true;
        let plain_emoji = cand(1, "😀");
        let symbols = cand(2, "✓😀");
        assert_eq!(pick(&[colour.clone(), plain_emoji.clone()], "😀", SANS), vec![0]);
        // The tick comes from a plain font even though the colour font also has it.
        assert_eq!(pick(&[colour, symbols], "✓", SANS), vec![2]);
    }

    #[test]
    fn nothing_covering_picks_nothing_and_skipped_faces_are_not_used() {
        let c = [cand(0, "α"), cand(1, "α")];
        assert!(pick(&c, "漢", SANS).is_empty());
        let skip: HashSet<usize> = [0].into();
        assert_eq!(choose(&c, &['α'], SANS, 6, &skip), vec![1]);
    }

    #[test]
    fn placeholder_faces_are_skipped_so_the_character_is_missing() {
        for n in ["LastResort", "Last Resort", "LastResort-Regular", "last-resort"] {
            assert!(is_placeholder(n), "{n}");
        }
        assert!(!is_placeholder("Geist") && !is_placeholder("Hiragino Sans"));
        // A mock machine: the placeholder claims every character, so only filtering keeps it out.
        let faces = [("Last Resort", cand(0, "\u{10fffd}漢"))];
        let pool: Vec<Cand> = faces.into_iter().filter(|(n, _)| !is_placeholder(n)).map(|(_, c)| c).collect();
        assert!(pick(&pool, "\u{10fffd}", SANS).is_empty());
    }

    #[test]
    fn at_most_the_limit_of_faces() {
        let c: Vec<Cand> = (0..10).map(|i| cand(i, &char::from_u32(0x4e00 + i as u32).unwrap().to_string())).collect();
        let w: Vec<char> = (0..10).map(|i| char::from_u32(0x4e00 + i).unwrap()).collect();
        assert_eq!(choose(&c, &w, SANS, 3, &HashSet::new()).len(), 3);
    }

    /// Runs on every OS. Windows and macOS always ship fonts with Greek and CJK, so there it must
    /// find them; on Linux it is skipped only when no such fonts are installed.
    #[test]
    fn this_machines_fonts_draw_greek_and_cjk() {
        let base = FontSet::new();
        let wanted: Vec<char> = "αβγδ漢字日本語".chars().collect();
        let had: Vec<&char> = wanted.iter().filter(|c| base.has_any(**c)).collect();
        assert!(had.is_empty(), "the embedded fonts were expected to lack {had:?}");
        let set = base.with_system_for(&wanted);
        let must = cfg!(any(windows, target_os = "macos"));
        let Some(set) = set else {
            assert!(!must, "no system font draws Greek or CJK on this OS");
            eprintln!("skipped: no system fonts with Greek or CJK installed");
            return;
        };
        let spec = mdv_reader::layout::FontSpec { family: Family::Sans, bold: false, italic: false };
        let drawn = |c: char| set.chain(spec).iter().any(|&i| set.fonts[i].face.glyph_index(c).is_some());
        let greek = "αβγδ".chars().all(drawn);
        let cjk = "漢字日本語".chars().all(drawn);
        if must {
            assert!(greek && cjk, "greek {greek}, cjk {cjk}");
        } else if !(greek && cjk) {
            eprintln!("partial: greek {greek}, cjk {cjk}");
        }
        assert!(greek || cjk);
        // Collection faces keep their index and the embedded fonts stay first.
        assert_eq!(set.chain(spec)[0], base.chain(spec)[0]);
        assert!(set.fonts[base.fonts.len()..].iter().all(|f| Font::new(f.data.into(), f.index).is_some()));
    }
}
