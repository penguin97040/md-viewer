//! Mutation fuzz of the section splitter, the parser-to-model step, layout and PDF generation, run
//! on a 1 MiB stack, the smallest a Windows main thread may have. The input is saved before each
//! run, so one that kills the process (a stack overflow) can be found afterwards.
//!
//! fuzz_doc <seed.md> <out-dir> <seconds> [rng-seed]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mdv_reader::layout::{layout_chunk, Assets, ChunkState, FontSpec, Options, RawGlyph, Shaper};
use mdv_reader::model::DocPos;
use mdv_reader::{doc::Document, split};
use mdv_theme::Mode;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const TOKENS: &[&str] = &[
    "\n", "\n\n", "> ", ">>>>", "- ", "1. ", "* ", "    ", "\t", "# ", "###### ", "---\n", "===\n", "```\n", "```mermaid\n", "~~~", "`", "``", "*", "**", "_", "__", "~~", "[", "](", ")", "![", "]: ", "[^1]", "[^1]: ",
    "|", "|-|\n", "|:-:|\n", "$", "$$\n", "\\frac{", "}", "<", ">", "</", "<div>", "</div>", "<details>", "<details open>", "<summary>", "</details>", "<table><tr><td>", "<pre>", "</pre>", "<!--", "-->", "<script>", "<b>", "<a href=x>", "<ol start=9>", "<li>", "<img src=x>",
    "&amp;", "&#0;", "&#x110000;", "\\", "\u{0}", "\u{feff}", "\u{202e}", "\u{e000}", "é", "😀", "\u{301}", "\r\n", "\r", "http://x.example", "www.x.example", "a@b.example", "- [ ] ", "- [x] ", ": ", "{#id}", "%%{init}%%", "graph TD; A-->B\n", "```perl\n",
];

fn mutate(src: &[char], r: &mut Rng) -> String {
    // A window, as whole documents make each run slow and most bugs are local.
    let len = 200 + r.below(8_000);
    let at = r.below(src.len().saturating_sub(len).max(1));
    let mut c: Vec<char> = src[at..(at + len).min(src.len())].to_vec();
    for _ in 0..1 + r.below(24) {
        let p = r.below(c.len() + 1);
        match r.below(6) {
            0 => {
                let e = (p + r.below(50)).min(c.len());
                c.drain(p..e);
            }
            1 | 2 => {
                let t: Vec<char> = TOKENS[r.below(TOKENS.len())].chars().collect();
                c.splice(p..p, t);
            }
            3 => {
                // A storm of one token.
                let t: Vec<char> = TOKENS[r.below(TOKENS.len())].chars().collect();
                let most = if r.below(10) == 0 { 2000 } else { 40 };
                let n = 1 + r.below(most);
                c.splice(p..p, t.iter().copied().cycle().take(t.len() * n));
            }
            4 => {
                let e = (p + r.below(200)).min(c.len());
                let seg = c[p..e].to_vec();
                let q = r.below(c.len() + 1);
                c.splice(q..q, seg);
            }
            _ => {
                if p < c.len() {
                    c[p] = char::from_u32(r.below(0x3100) as u32).unwrap_or('x');
                }
            }
        }
    }
    c.into_iter().collect()
}

static STAGE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
fn stage(s: String) {
    *STAGE.lock().unwrap_or_else(|e| e.into_inner()) = s;
}

struct Fake;
impl Shaper for Fake {
    fn shape(&mut self, text: &str, _f: FontSpec, size: f32, out: &mut Vec<RawGlyph>) -> f32 {
        let mut x = 0.0;
        for (i, _) in text.char_indices() {
            out.push(RawGlyph { x, byte: i as u32, id: 1, font: 1, emoji: false });
            x += size * 0.5;
        }
        x
    }
    fn metrics(&mut self, _f: FontSpec, size: f32) -> (f32, f32) {
        ((size * 0.8).round(), (size * 0.2).round())
    }
}
struct NoAssets;
impl Assets for NoAssets {}

/// Everything except the PDF. The checks are the point: a wrong answer is as bad as a panic.
fn exercise(src: &str, r: &mut Rng) {
    for (s, e) in [(split::split_sections(src, 1 << 10), 1), (split::split_sections(src, 16 << 10), 2)] {
        let mut at = 0;
        for &(a, b) in &s {
            assert!(a == at && a <= b && src.is_char_boundary(a) && src.is_char_boundary(b), "split {e}: ranges must cover the source in order");
            at = b;
        }
        assert_eq!(at, src.len(), "split {e}: must reach the end");
    }
    stage("parse".into());
    let d = Document::parse(src.as_bytes().to_vec());
    let o = Options {
        width: [1.0, 40.0, 320.0, 800.0, 3000.0][r.below(5)],
        padding: 20.0,
        font_size: 17.0,
        line_height: 1.65,
        serif: r.below(2) == 0,
        wrap: r.below(2) == 0,
        mode: if r.below(2) == 0 { Mode::Dark } else { Mode::Light },
    };
    let n = d.chunks.len();
    for i in 0..n.min(64) {
        let c = d.chunk(i);
        stage(format!("layout chunk {i} of {n} at width {} wrap {}", o.width, o.wrap));
        let l = layout_chunk(&c, &o, &ChunkState::default(), i == 0, i + 1 == n, &mut Fake, &NoAssets);
        assert!(l.height.is_finite() && l.height >= 0.0, "height {}", l.height);
    }
    stage("search and selection".into());
    let _ = d.search("a");
    if n > 0 {
        let _ = d.text_between(DocPos::new(0, 0), DocPos::new(n as u32 - 1, u32::MAX));
    }
    let _ = d.locate_id("id", 0);
}

fn run_all(src: Arc<String>, seed: u64, pdf: bool) -> Result<(), String> {
    let h = std::thread::Builder::new().stack_size(1 << 20).spawn(move || {
        let mut r = Rng(seed | 1);
        stage("split".into());
        exercise(&src, &mut r);
        if pdf {
            stage("pdf".into());
            let d = Document::parse(src.as_bytes().to_vec());
            let _ = mdv_print::render_pdf(&d, &mdv_print::PrintOptions::default(), &mut |_| true);
        }
    });
    match h.expect("thread").join() {
        Ok(()) => Ok(()),
        Err(p) => Err(p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()),
    }
}

/// `fuzz_doc replay FILE`: runs each stage in turn and names it, to find what a saved input trips.
fn replay(path: &str) {
    let src = std::fs::read_to_string(path).expect("input");
    for target in [1 << 10, 16 << 10] {
        println!("split {target}");
        let _ = split::split_sections(&src, target);
    }
    println!("parse");
    let d = Document::parse(src.as_bytes().to_vec());
    for width in [1.0, 40.0, 320.0, 800.0, 3000.0] {
        for wrap in [true, false] {
            println!("layout width {width} wrap {wrap}");
            let o = Options { width, padding: 20.0, font_size: 17.0, line_height: 1.65, serif: false, wrap, mode: Mode::Dark };
            for i in 0..d.chunks.len() {
                let t = Instant::now();
                let _ = layout_chunk(&d.chunk(i), &o, &ChunkState::default(), i == 0, i + 1 == d.chunks.len(), &mut Fake, &NoAssets);
                println!("  chunk {i}: {:?}", t.elapsed());
            }
        }
    }
    println!("search and selection");
    let _ = d.search("a");
    let _ = d.text_between(DocPos::new(0, 0), DocPos::new(d.chunks.len() as u32 - 1, u32::MAX));
    println!("pdf");
    let _ = mdv_print::render_pdf(&d, &mdv_print::PrintOptions::default(), &mut |_| true);
    println!("all stages finished");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a[1] == "replay" {
        return replay(&a[2]);
    }
    let src: Vec<char> = std::fs::read_to_string(&a[1]).expect("seed").chars().collect();
    let out = std::path::PathBuf::from(&a[2]);
    std::fs::create_dir_all(&out).expect("out dir");
    let secs: u64 = a[3].parse().expect("seconds");
    let mut r = Rng(a.get(4).map_or(0x9E37_79B9_7F4A_7C15, |s| s.parse().expect("seed")));
    std::panic::set_hook(Box::new(|_| {}));

    // A hung input cannot be cancelled: name it from outside and leave the process to the runner.
    let started = Arc::new(AtomicU64::new(0));
    let clock = Instant::now();
    let watch = started.clone();
    let hang_dir = out.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let s = watch.load(Ordering::SeqCst);
        if s != 0 && (clock.elapsed().as_millis() as u64).saturating_sub(s) > 60_000 {
            let _ = std::fs::copy(hang_dir.join("current.md"), hang_dir.join("hang.md"));
            // Spinning or blocked? Each thread's name, state and CPU time.
            let mut states = String::new();
            for t in std::fs::read_dir("/proc/self/task").into_iter().flatten().flatten() {
                let stat = std::fs::read_to_string(t.path().join("stat")).unwrap_or_default();
                let f: Vec<&str> = stat.split_whitespace().collect();
                if f.len() > 14 {
                    states.push_str(&format!("{} {} u{} s{}; ", f[1], f[2], f[13], f[14]));
                }
            }
            println!("threads: {states}");
            println!("HANG over 60 s in {}; input saved", STAGE.lock().unwrap_or_else(|e| e.into_inner()));
            std::process::exit(3);
        }
    });

    let (mut inputs, mut panics, mut slow, mut worst, mut pdfs) = (0u64, 0u64, 0u64, 0f32, 0u64);
    let mut messages: Vec<String> = Vec::new();
    while clock.elapsed().as_secs() < secs {
        let mut text = mutate(&src, &mut r);
        // Every so often, over the limit that switches to section parsing.
        if r.below(30) == 0 {
            let copies = (1 << 20) / text.len().max(1) + 2;
            text = text.repeat(copies.min(4000));
        }
        let pdf = r.below(40) == 0 && text.len() < 200_000;
        pdfs += pdf as u64;
        std::fs::write(out.join("current.md"), &text).expect("save");
        started.store(clock.elapsed().as_millis() as u64 + 1, Ordering::SeqCst);
        let t = Instant::now();
        let result = run_all(Arc::new(text.clone()), r.next(), pdf);
        started.store(0, Ordering::SeqCst);
        let took = t.elapsed().as_secs_f32();
        inputs += 1;
        worst = worst.max(took);
        if took > 5.0 {
            slow += 1;
            std::fs::write(out.join(format!("slow-{slow}.md")), &text).expect("save");
        }
        if let Err(m) = result {
            panics += 1;
            let short: String = m.chars().take(120).collect();
            if !messages.contains(&short) {
                messages.push(short);
                std::fs::write(out.join(format!("panic-{}.md", messages.len())), &text).expect("save");
            }
        }
    }
    println!("inputs {inputs}; with PDF {pdfs}; panics {panics}; over 5 s {slow}; slowest {worst:.2} s");
    for m in messages {
        println!("panic: {m}");
    }
}
