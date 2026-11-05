//! Mutation fuzz of the 20 diagram samples and the maths set. Reports outcome counts by error
//! variant, slowest case, and any panic or timeout (with the input saved to the out dir).
//!
//! fuzz <samples-dir> <maths.tsv> <out-dir> <mutations-per-input>
use std::collections::BTreeMap;
use std::time::Instant;
use mdv_content::{render_math, render_mermaid, ContentError, DiagramOptions, MathOptions};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
    fn below(&mut self, n: usize) -> usize { if n == 0 { 0 } else { (self.next() % n as u64) as usize } }
}

fn mutate(src: &str, toks: &[&str], r: &mut Rng) -> String {
    let mut chars: Vec<char> = src.chars().collect();
    for _ in 0..1 + r.below(4) {
        match r.below(6) {
            0 => { let p = r.below(chars.len() + 1); chars.truncate(p); }
            1 => { if !chars.is_empty() { let a = r.below(chars.len()); let b = (a + r.below(40)).min(chars.len()); chars.drain(a..b); } }
            2 => { let t: Vec<char> = toks[r.below(toks.len())].chars().collect(); let p = r.below(chars.len() + 1); for (i, c) in t.into_iter().enumerate() { chars.insert(p + i, c); } }
            3 => { if !chars.is_empty() { let a = r.below(chars.len()); let b = (a + r.below(60)).min(chars.len()); let seg: Vec<char> = chars[a..b].to_vec(); let p = r.below(chars.len() + 1); for (i, c) in seg.into_iter().enumerate() { chars.insert(p + i, c); } } }
            4 => { if !chars.is_empty() { let a = r.below(chars.len()); chars[a] = char::from_u32(r.below(0x3000) as u32).unwrap_or('x'); } }
            _ => { let p = r.below(chars.len() + 1); chars.insert(p, '\n'); }
        }
    }
    chars.into_iter().collect()
}

fn tally(kind: &str, r: Result<mdv_content::RenderedAsset, ContentError>, ms: f64, input: &str, out: &str, idx: usize, counts: &mut BTreeMap<String, usize>, slow: &mut (f64, String)) {
    let key = match &r { Ok(_) => "Ok".to_string(), Err(e) => format!("{e:?}").split('(').next().unwrap().to_string() };
    if ms > slow.0 { *slow = (ms, input.chars().take(60).collect::<String>().replace('\n', "\\n")); }
    if matches!(key.as_str(), "Panic" | "Timeout" | "Unsafe") {
        let _ = std::fs::write(format!("{out}/fuzz-{kind}-{key}-{idx}.txt"), input);
        eprintln!("{kind}: {key} on input #{idx}: {:?}", r.as_ref().err());
    }
    *counts.entry(key).or_default() += 1;
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a[4].parse().unwrap();
    std::fs::create_dir_all(&a[3]).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(&a[1]).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "mmd")).collect();
    files.sort();
    let mtoks = ["{", "}", "\"", "<", ">", ";", "%%{init:{\"theme\":\"x\"}}%%", "\0", "-->", ":::", "end", "subgraph", "click a href \"http://x\"", "[", "]", "(", ")", "|", "&", "\\", "<script>", "style a fill:url(http://x)", "9999999999", "-1", "--- ", ": ", ",", "classDef x fill:#f00"];
    let ttoks = ["\\", "{", "}", "^", "_", "&", "\\\\", "\\frac", "\\left(", "\\right)", "\\begin{matrix}", "\\end{matrix}", "\\def\\a{\\a\\a}\\a", "\\sqrt[", "\\text{", "$", "\\rule{99em}{99em}", "\\kern 1e9em", "\u{0}", "𝔸", "\\color{", "\\sum_{", "\\overbrace{", "\\hline", "\\cases", "%", "\\verb|"];
    let mut r = Rng(0x9e3779b97f4a7c15);
    let (mut counts, mut slow, mut total) = (BTreeMap::new(), (0.0, String::new()), 0usize);
    let t0 = Instant::now();
    for (fi, f) in files.iter().enumerate() {
        let base = std::fs::read_to_string(f).unwrap();
        for i in 0..n {
            let m = mutate(&base, &mtoks, &mut r);
            let t = Instant::now();
            let res = render_mermaid(&m, &DiagramOptions::default());
            tally("mermaid", res, t.elapsed().as_secs_f64() * 1000.0, &m, &a[3], fi * n + i, &mut counts, &mut slow);
            total += 1;
        }
    }
    println!("mermaid: {total} mutated inputs in {:.1} s; outcomes {counts:?}; slowest {:.1} ms on {:?}", t0.elapsed().as_secs_f64(), slow.0, slow.1);
    let (mut counts, mut slow, mut total) = (BTreeMap::new(), (0.0, String::new()), 0usize);
    let t0 = Instant::now();
    for (fi, line) in std::fs::read_to_string(&a[2]).unwrap().lines().enumerate() {
        let tex = line.splitn(3, '\t').nth(2).unwrap();
        for i in 0..n * 4 {
            let m = mutate(tex, &ttoks, &mut r);
            let t = Instant::now();
            let res = render_math(&m, i % 2 == 0, &MathOptions::default());
            tally("math", res, t.elapsed().as_secs_f64() * 1000.0, &m, &a[3], fi * n * 4 + i, &mut counts, &mut slow);
            total += 1;
        }
    }
    println!("math: {total} mutated inputs in {:.1} s; outcomes {counts:?}; slowest {:.1} ms on {:?}", t0.elapsed().as_secs_f64(), slow.0, slow.1);
}
