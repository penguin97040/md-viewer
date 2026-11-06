//! Documents written to hurt. A stack overflow or a failed allocation ends the process and every
//! window with it, and `catch_unwind` can't help, so these run the parser, layout, search and drop
//! on a thread with the 1 MiB a Windows main thread may have. Sizes that pass here without tripping
//! anything are bounded by the input; what must not happen is a crash, a hang or a NaN.

use std::time::{Duration, Instant};

use crate::doc::Document;
use crate::layout::{layout_chunk, Assets, AssetBox, ChunkState, FontSpec, ImageState, NoAssets, Options, RawGlyph, Shaper};
use crate::model::ImageSource;
use crate::parse::parse_whole;
use mdv_theme::Mode;

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

const OPTS: Options = Options { width: 800.0, padding: 20.0, font_size: 16.0, line_height: 1.5, serif: false, wrap: true, mode: Mode::Dark };
/// Everything arrives, and at the most awkward size the renderers could claim.
struct Huge;
impl Assets for Huge {
    fn image(&self, _s: &ImageSource) -> ImageState {
        ImageState::Ready { width: 16_384.0, height: 16_384.0, key: 1 }
    }
    fn math(&self, _src: &str, _display: bool, _font_size: f32) -> Option<AssetBox> {
        Some(AssetBox { width: 20_000.0, height: 20_000.0, baseline: 20_000.0, key: 2 })
    }
    fn mermaid(&self, _src: &str) -> Option<AssetBox> {
        Some(AssetBox { width: 20_000.0, height: 20_000.0, baseline: 20_000.0, key: 3 })
    }
}

fn check(name: &str, src: String) {
    let name = name.to_string();
    let budget = Duration::from_secs(120);
    let t = Instant::now();
    let label = name.clone();
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || {
            let w = parse_whole(&src, 1 << 30, false);
            for c in &w.chunks {
                for a in [&NoAssets as &dyn Assets, &Huge] {
                    let l = layout_chunk(c, &OPTS, &ChunkState::default(), true, true, &mut Fake, a);
                    assert!(l.height.is_finite() && l.height >= 0.0, "{label}: height {}", l.height);
                    for layer in &l.layers {
                        for b in &layer.boxes {
                            let r = b.rect;
                            assert!(r.x.is_finite() && r.y.is_finite() && r.w.is_finite() && r.h.is_finite(), "{label}: box {r:?}");
                        }
                    }
                }
            }
            drop(w);
        })
        .unwrap()
        .join()
        .unwrap_or_else(|_| panic!("{name}: panicked"));
    assert!(t.elapsed() < budget, "{name}: took {:?}", t.elapsed());
}

#[test]
fn deep_nesting() {
    let n = 100_000;
    check("quotes", ">".repeat(n) + " x\n");
    check("quotes spaced", "> ".repeat(n) + "x\n");
    check("lists", (0..2_000).map(|i| format!("{}- x\n", " ".repeat(i * 2))).collect());
    check("list markers", "- ".repeat(n) + "x\n");
    check("ordered markers", "1. ".repeat(n) + "x\n");
    check("divs", "<div>".repeat(n) + "x");
    check("spans", "<span>".repeat(n) + "x");
    check("bold html", "<b>".repeat(n) + "x");
    check("details", "<details><summary>a</summary>".repeat(10_000) + "x");
    check("tables in html", "<table><tr><td>".repeat(20_000) + "x");
    check("html lists", "<ul><li>".repeat(30_000) + "x");
    check("blockquote html", "<blockquote>".repeat(30_000) + "x");
    check("sub sup", "<sub><sup>".repeat(30_000) + "x");
    check("code in quotes", ">".repeat(5_000) + " ```\nx\n");
}

#[test]
fn deep_inline_markers() {
    let n = 20_000;
    check("stars", "*a ".repeat(n));
    check("unders", "_a ".repeat(n));
    check("brackets", "[".repeat(n));
    check("links", "[a".repeat(n) + &"](b)".repeat(n));
    check("images", "![".repeat(n) + &"](b)".repeat(n));
    check("strikes", "~~a ".repeat(n));
    check("backticks", "`a ".repeat(n));
    check("autolinks", "https://a.example/ ".repeat(n));
    check("entities", "&#x110000;&#0;&amp;".repeat(n));
    check("footnotes", (0..5_000).map(|i| format!("[^{i}]\n\n[^{i}]: [^{}]\n\n", i + 1)).collect());
}

#[test]
fn document_chosen_sizes() {
    for v in ["1e30", "1e308", "-1", "NaN", "inf", "-inf", "99999999999999999999999", "0", "1e-30", "%", "100%", "1px", ""] {
        check(&format!("image size {v}"), format!("<img src=\"a.png\" width=\"{v}\" height=\"{v}\">\n\n![x](y.png)\n\n<img src=\"https://a.example/b.png\" width=\"{v}\" height=\"{v}\">\n"));
    }
    check("table columns", format!("|{}\n|{}\nx\n", "a|".repeat(50_000), "-|".repeat(50_000)));
    check("table rows", format!("|a|b|\n|-|-|\n{}", "|1|2|\n".repeat(50_000)));
    check("colspan", "<table><tr><td colspan=\"99999999\">x</td><td rowspan=\"99999999\">y</td></tr></table>\n".to_string());
    check("long word", "w".repeat(2_000_000));
    check("long code line", format!("```\n{}\n```\n", "w".repeat(2_000_000)));
    check("many formulas", "$x$ ".repeat(20_000));
    check("many diagrams", "```mermaid\nflowchart LR\n A-->B\n```\n\n".repeat(5_000));
    check("headings", "# a\n\n".repeat(50_000));
    check("nul and controls", "a\0b\u{1}\u{7f}\u{feff}\u{202e}\r\r\n".repeat(10_000));
}

#[test]
fn search_and_outline_over_a_hostile_document() {
    let dir = std::env::temp_dir().join(format!("mdv-hostile-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("h.md");
    std::fs::write(&path, format!("{}\n\n{}\n", "> ".repeat(50_000), "*a ".repeat(50_000))).unwrap();
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(move || {
            let d = Document::load(&path).unwrap();
            let _ = d.search("a");
            for i in 0..d.chunks.len() {
                drop(d.chunk(i));
            }
            let _ = std::fs::remove_dir_all(path.parent().unwrap());
        })
        .unwrap()
        .join()
        .unwrap();
}
