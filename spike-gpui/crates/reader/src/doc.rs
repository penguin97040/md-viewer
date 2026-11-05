//! A loaded document: chunking, outline, bounded parse cache and search. Port of
//! `internal/doc/doc.go`.
//!
//! Files up to 1 MB are parsed once and their chunks kept. Larger files are cut into safe sections
//! of about 16 KB, scanned once in parallel for headings and link references (nothing else of that
//! parse is kept), then each section is parsed again when asked for, with a bounded cache.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::fold;
use crate::model::*;
use crate::parse::{self, RefMap};
use crate::slug::Slugger;
use crate::split;

pub const TARGET_CHUNK_BYTES: usize = 16 * 1024;
pub const WHOLE_PARSE_LIMIT: usize = 1 << 20;
pub const CACHE_LIMIT: usize = 96;
pub const MAX_FILE_BYTES: u64 = 256 << 20;

/// What the viewer needs to estimate a chunk's height before parsing it.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChunkMeta {
    pub start: u32,
    pub end: u32,
    pub lines: u32,
    pub first_heading: u32,
    pub heading_count: u32,
}

impl ChunkMeta {
    pub fn bytes(&self) -> u32 {
        self.end - self.start
    }
}

enum Mode {
    Whole(Vec<Arc<ChunkContent>>),
    Sections(Mutex<Cache>),
}

#[derive(Default)]
struct Cache {
    map: HashMap<u32, (Arc<ChunkContent>, u64)>,
    clock: u64,
    /// Parses done since the document opened; a test and the performance log read it.
    parses: u64,
}

pub struct Document {
    pub source: Arc<str>,
    pub path: Option<PathBuf>,
    pub chunks: Vec<ChunkMeta>,
    pub headings: Vec<HeadingInfo>,
    pub parse_time: Duration,
    /// One parse of one file: a reload gets a new id, so chunks can never mix versions.
    pub id: u64,
    refs: RefMap,
    mode: Mode,
    index: OnceLock<Vec<Box<str>>>,
    pub index_time: Mutex<Option<Duration>>,
}

fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Document {
    /// Reads and parses a file, refusing anything but a regular file of a sensible size.
    pub fn load(path: &Path) -> Result<Self, String> {
        let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err("That is not a file".into());
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(format!("the file is larger than {} MB", MAX_FILE_BYTES >> 20));
        }
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            return Err(format!("the file is larger than {} MB", MAX_FILE_BYTES >> 20));
        }
        let mut d = Self::parse(bytes);
        d.path = Some(path.to_path_buf());
        Ok(d)
    }

    /// Parses markdown source. Invalid UTF-8 is replaced rather than refused.
    pub fn parse(bytes: Vec<u8>) -> Self {
        let t = Instant::now();
        let source: Arc<str> = normalise_newlines(bytes).into();
        let mut d = if source.len() <= WHOLE_PARSE_LIMIT { Self::parse_whole(source) } else { Self::parse_sections(source) };
        d.parse_time = t.elapsed();
        d
    }

    fn parse_whole(source: Arc<str>) -> Self {
        let w = parse::parse_whole(&source, TARGET_CHUNK_BYTES, false);
        let b = source.as_bytes();
        let mut chunks: Vec<ChunkMeta> = w
            .ranges
            .iter()
            .map(|&(s, e)| ChunkMeta { start: s, end: e, lines: count_lines(&b[s as usize..e as usize]), first_heading: 0, heading_count: 0 })
            .collect();
        fill_heading_ranges(&mut chunks, &w.headings);
        let mut contents = w.chunks;
        contents.par_iter_mut().for_each(colour_chunk);
        Self {
            source,
            path: None,
            chunks,
            headings: w.headings,
            parse_time: Duration::ZERO,
            id: next_id(),
            refs: RefMap::default(),
            mode: Mode::Whole(contents.into_iter().map(Arc::new).collect()),
            index: OnceLock::new(),
            index_time: Mutex::new(None),
        }
    }

    fn parse_sections(source: Arc<str>) -> Self {
        let ranges = split::split_sections(&source, TARGET_CHUNK_BYTES);
        let scans: Vec<parse::SectionScan> = ranges.par_iter().map(|&(s, e)| parse::scan_section(&source[s..e])).collect();
        let b = source.as_bytes();
        let lines: Vec<u32> = ranges.par_iter().map(|&(s, e)| count_lines(&b[s..e])).collect();
        let mut slugs = Slugger::new();
        let mut headings = Vec::with_capacity(scans.iter().map(|s| s.headings.len()).sum());
        let mut chunks = Vec::with_capacity(ranges.len());
        let mut refs = RefMap::default();
        for (ci, scan) in scans.into_iter().enumerate() {
            let first = headings.len() as u32;
            let n = scan.headings.len() as u32;
            for h in scan.headings {
                let id = match h.explicit_id.filter(|i| !i.is_empty()) {
                    Some(i) => {
                        slugs.reserve(&i);
                        i
                    }
                    None => slugs.slug(&h.text),
                };
                headings.push(HeadingInfo { level: h.level, text: h.text, id, chunk: ci as u32 });
            }
            for (label, dest, title) in scan.refs {
                refs.0.entry(label).or_insert((dest, title));
            }
            chunks.push(ChunkMeta { start: ranges[ci].0 as u32, end: ranges[ci].1 as u32, lines: lines[ci], first_heading: first, heading_count: n });
        }
        Self {
            source,
            path: None,
            chunks,
            headings,
            parse_time: Duration::ZERO,
            id: next_id(),
            refs,
            mode: Mode::Sections(Mutex::new(Cache::default())),
            index: OnceLock::new(),
            index_time: Mutex::new(None),
        }
    }

    pub fn is_whole(&self) -> bool {
        matches!(self.mode, Mode::Whole(_))
    }

    fn section_ids(&self, i: usize) -> Vec<String> {
        let m = &self.chunks[i];
        self.headings[m.first_heading as usize..(m.first_heading + m.heading_count) as usize].iter().map(|h| h.id.clone()).collect()
    }

    fn parse_chunk(&self, i: usize, colours: bool) -> ChunkContent {
        let m = &self.chunks[i];
        parse::parse_section(&self.source[m.start as usize..m.end as usize], m.start, i as u32, &self.refs, &self.section_ids(i), colours)
    }

    /// A parsed chunk, from the cache or parsed now. Safe from any thread.
    pub fn chunk(&self, i: usize) -> Arc<ChunkContent> {
        match &self.mode {
            Mode::Whole(c) => c[i].clone(),
            Mode::Sections(cache) => {
                if let Some(c) = self.cached(i) {
                    return c;
                }
                let content = Arc::new(self.parse_chunk(i, true));
                let mut g = cache.lock().expect("cache");
                g.clock += 1;
                g.parses += 1;
                let clock = g.clock;
                g.map.insert(i as u32, (content.clone(), clock));
                if g.map.len() > CACHE_LIMIT {
                    if let Some(old) = g.map.iter().min_by_key(|(_, v)| v.1).map(|(k, _)| *k) {
                        g.map.remove(&old);
                    }
                }
                content
            }
        }
    }

    /// A parsed chunk only if it is ready without parsing.
    pub fn cached(&self, i: usize) -> Option<Arc<ChunkContent>> {
        match &self.mode {
            Mode::Whole(c) => Some(c[i].clone()),
            Mode::Sections(cache) => {
                let mut g = cache.lock().expect("cache");
                g.clock += 1;
                let clock = g.clock;
                g.map.get_mut(&(i as u32)).map(|e| {
                    e.1 = clock;
                    e.0.clone()
                })
            }
        }
    }

    pub fn cached_count(&self) -> usize {
        match &self.mode {
            Mode::Whole(c) => c.len(),
            Mode::Sections(cache) => cache.lock().expect("cache").map.len(),
        }
    }

    pub fn parse_count(&self) -> u64 {
        match &self.mode {
            Mode::Whole(_) => 0,
            Mode::Sections(cache) => cache.lock().expect("cache").parses,
        }
    }

    fn text_index(&self) -> &Vec<Box<str>> {
        self.index.get_or_init(|| {
            let t = Instant::now();
            let v = (0..self.chunks.len())
                .into_par_iter()
                .map(|i| {
                    let folded = match &self.mode {
                        Mode::Whole(c) => fold::fold_skipping(&c[i].text, &c[i].no_find).0,
                        Mode::Sections(_) => {
                            // Colours are not needed for the text, and take most of a parse.
                            let c = self.parse_chunk(i, false);
                            fold::fold_skipping(&c.text, &c.no_find).0
                        }
                    };
                    folded.into_boxed_str()
                })
                .collect();
            *self.index_time.lock().expect("time") = Some(t.elapsed());
            v
        })
    }

    /// The number of case-insensitive matches of `q` in each chunk, parsed or not.
    pub fn search(&self, q: &str) -> Vec<u32> {
        let q = fold::fold(q);
        if q.is_empty() {
            return vec![0; self.chunks.len()];
        }
        self.text_index().par_iter().map(|t| fold::count(t, &q)).collect()
    }

    /// Text between two document positions, in reading order, parsing chunks as needed. Does not
    /// mount anything, and keeps at most the cache limit of parsed chunks alive.
    pub fn text_between(&self, a: DocPos, b: DocPos) -> String {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let mut out = String::new();
        for ci in a.chunk..=b.chunk {
            if ci as usize >= self.chunks.len() {
                break;
            }
            let c = self.chunk(ci as usize);
            let from = if ci == a.chunk { (a.offset as usize).min(c.text.len()) } else { 0 };
            let to = if ci == b.chunk { (b.offset as usize).min(c.text.len()) } else { c.text.len() };
            if from >= to {
                continue;
            }
            if !out.is_empty() {
                out.push_str(SEP_BLOCK);
            }
            out.push_str(c.text.get(from..to).unwrap_or(""));
        }
        out
    }

    /// The chunk holding an element id, looking at headings and at any parsed chunk.
    pub fn locate_id(&self, id: &str, prefer: usize) -> Option<(usize, u32)> {
        if prefer < self.chunks.len() {
            if let Some(c) = self.cached(prefer) {
                if let Some(a) = c.anchors.iter().find(|a| a.id == id) {
                    return Some((prefer, a.offset));
                }
            }
        }
        let lower = id.to_lowercase();
        let h = self.headings.iter().find(|h| h.id == id || h.id == lower)?;
        let i = h.chunk as usize;
        let c = self.chunk(i);
        c.anchors.iter().find(|a| a.id == h.id).map(|a| (i, a.offset))
    }
}

fn colour_chunk(c: &mut ChunkContent) {
    let text = std::mem::take(&mut c.text);
    colour_blocks(&mut c.blocks, &text);
    c.text = text;
}

fn colour_blocks(blocks: &mut [Block], text: &str) {
    for b in blocks {
        match b {
            Block::Code(cb) if cb.copyable => {
                if let Some(lang) = cb.lang.as_deref() {
                    cb.tokens = crate::highlight::highlight(lang, &text[cb.text.range()], cb.text.start);
                }
            }
            Block::Quote { blocks, .. } | Block::Aligned { blocks, .. } => colour_blocks(blocks, text),
            Block::Details(d) => colour_blocks(&mut d.blocks, text),
            Block::List(l) => l.items.iter_mut().for_each(|i| colour_blocks(&mut i.blocks, text)),
            Block::DefinitionList { items, .. } => items.iter_mut().flat_map(|i| i.definitions.iter_mut()).for_each(|d| colour_blocks(d, text)),
            Block::Footnotes { items, .. } => items.iter_mut().for_each(|i| colour_blocks(&mut i.blocks, text)),
            _ => {}
        }
    }
}

fn fill_heading_ranges(chunks: &mut [ChunkMeta], headings: &[HeadingInfo]) {
    let mut first = vec![None; chunks.len()];
    for (i, h) in headings.iter().enumerate() {
        let c = h.chunk as usize;
        if c < chunks.len() {
            first[c].get_or_insert(i as u32);
            chunks[c].heading_count += 1;
        }
    }
    let mut next = headings.len() as u32;
    for i in (0..chunks.len()).rev() {
        chunks[i].first_heading = first[i].unwrap_or(next);
        next = chunks[i].first_heading;
    }
}

fn count_lines(b: &[u8]) -> u32 {
    memchr::memchr_iter(b'\n', b).count() as u32 + 1
}

fn normalise_newlines(bytes: Vec<u8>) -> String {
    let s = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    };
    if !s.contains('\r') {
        return s;
    }
    s.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn big_doc(n: usize) -> Vec<u8> {
        let mut b = String::new();
        let mut i = 0;
        while b.len() < n {
            write!(
                b,
                "## Heading {i}\n\nA paragraph with **bold**, *italic*, `code` and a [link](https://example.com/{i}). Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore.\n\n- item one\n- item two\n  - nested\n\n| col a | col b | col c |\n|---|---|---|\n| {i} | two | three |\n| x | y | z |\n\n```js\nfunction f{i}(a, b) {{ return a + b; }}\n```\n\n> A quote line.\n\n"
            )
            .unwrap();
            i += 1;
        }
        b.into_bytes()
    }

    /// `TestSearch`.
    #[test]
    fn search_counts() {
        let d = Document::parse(b"# Apple\n\nAn apple a day.\n\n```\napple pie\n```\n\n| APPLE | x |\n|---|---|\n".to_vec());
        assert_eq!(d.search("apple")[0], 4);
        assert_eq!(d.search("  ")[0], 0);
    }

    /// `TestSectionMode`.
    #[test]
    fn section_mode() {
        let mut b = String::from("See [the ref][r] and [jump](#heading-1).\n\n");
        let mut i = 0;
        while b.len() < WHOLE_PARSE_LIMIT + 100_000 {
            write!(b, "## Heading\n\nText {i} with some words to fill the section up nicely.\n\n").unwrap();
            i += 1;
        }
        b.push_str("[r]: https://example.com/ref\n");
        let d = Document::parse(b.into_bytes());
        assert!(!d.is_whole() && d.chunks.len() >= 10, "{}", d.chunks.len());
        let c0 = d.chunk(0);
        let Block::Paragraph { content, .. } = &c0.blocks[0] else { panic!() };
        assert!(
            content.iter().any(|i| matches!(i, Inline::Link { target: LinkTarget::Web(u), .. } if u == "https://example.com/ref")),
            "cross-section reference not resolved: {content:?}"
        );
        assert_eq!(d.headings[1].id, "heading-1");
        let last = d.headings.last().unwrap();
        let c = d.chunk(last.chunk as usize);
        assert!(c.anchors.iter().any(|a| a.id == last.id), "id {} not in chunk {}", last.id, last.chunk);
        assert_eq!(d.search("text 5 with")[0], 1);
    }

    #[test]
    fn section_cache_is_bounded() {
        let d = Document::parse(big_doc(4 << 20));
        assert!(d.chunks.len() > CACHE_LIMIT * 2, "{}", d.chunks.len());
        for i in 0..d.chunks.len() {
            let _ = d.chunk(i);
        }
        assert!(d.cached_count() <= CACHE_LIMIT, "{}", d.cached_count());
        assert_eq!(d.parse_count() as usize, d.chunks.len());
    }

    #[test]
    fn opening_a_large_file_keeps_no_blocks() {
        let d = Document::parse(big_doc(2 << 20));
        assert_eq!(d.cached_count(), 0);
        assert_eq!(d.parse_count(), 0);
        let total: u32 = d.chunks.iter().map(ChunkMeta::bytes).sum();
        assert_eq!(total as usize, d.source.len());
    }

    #[test]
    fn selection_text_crosses_unparsed_chunks() {
        let d = Document::parse(big_doc(2 << 20));
        // Chunks 5..=7, with the middle one never asked for before.
        let (a, b) = (d.chunk(5), d.chunk(7));
        assert!(d.cached(6).is_none());
        let start = a.text.len() as u32 - 10;
        let t = d.text_between(DocPos::new(5, start), DocPos::new(7, 20));
        assert!(t.starts_with(&a.text[start as usize..]));
        assert!(t.ends_with(&b.text[..20]));
        assert!(t.contains(&d.chunk(6).text));
        // The same with the positions the other way round.
        assert_eq!(t, d.text_between(DocPos::new(7, 20), DocPos::new(5, start)));
    }

    #[test]
    fn crlf_and_invalid_utf8() {
        let d = Document::parse(b"# A\r\n\r\nB\rC \xff D\n".to_vec());
        assert_eq!(d.source.as_ref(), "# A\n\nB\nC \u{fffd} D\n");
    }

    #[test]
    fn refuses_folders() {
        assert!(Document::load(Path::new("/")).is_err());
        assert!(Document::load(Path::new("/dev/null")).is_err());
    }

    #[test]
    fn chunk_meta_heading_ranges() {
        let d = Document::parse(big_doc(2 << 20));
        for (ci, m) in d.chunks.iter().enumerate() {
            for h in &d.headings[m.first_heading as usize..(m.first_heading + m.heading_count) as usize] {
                assert_eq!(h.chunk as usize, ci);
            }
        }
        let n: u32 = d.chunks.iter().map(|m| m.heading_count).sum();
        assert_eq!(n as usize, d.headings.len());
    }

    /// Run with `MDV_BIG=<big25.md> cargo test --release -p mdv-reader big_file -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn big_file_timing() {
        let path = std::env::var("MDV_BIG").expect("MDV_BIG");
        let t = Instant::now();
        let d = Document::load(Path::new(&path)).unwrap();
        eprintln!("load+parse {:?} (parse {:?}), chunks {}, headings {}", t.elapsed(), d.parse_time, d.chunks.len(), d.headings.len());
        let t = Instant::now();
        let n: u32 = d.search("colour number").iter().sum();
        eprintln!("first search {:?}: {n} matches (index {:?})", t.elapsed(), d.index_time.lock().unwrap());
        let t = Instant::now();
        let n: u32 = d.search("lorem ipsum").iter().sum();
        eprintln!("second search {:?}: {n} matches", t.elapsed());
        let t = Instant::now();
        let _ = d.chunk(d.chunks.len() / 2);
        eprintln!("one chunk with colours {:?}", t.elapsed());
    }
}
