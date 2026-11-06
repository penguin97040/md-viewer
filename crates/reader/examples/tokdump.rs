//! Prints per-byte token classes as "colour bold italic" runs, for comparison with chroma.
use mdv_reader::layout::token_style;
use mdv_theme::Mode;
use std::io::Read;
fn main() {
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    // Blocks are separated by a line holding only U+0001; the first line of each is the language.
    for block in s.split("\u{1}\n") {
        let Some((lang, code)) = block.split_once('\n') else { continue };
        let toks = mdv_reader::highlight::highlight(lang, code, 0);
        println!("B\t{lang}");
        for t in toks {
            let (d, db, di) = token_style(t.class, Mode::Dark);
            let (l, lb, li) = token_style(t.class, Mode::Light);
            println!("T\t{}\t{}\t{}\t{}", t.span.start, t.span.end, fmt(d, db, di), fmt(l, lb, li));
        }
    }
}
fn fmt(c: Option<u32>, b: bool, i: bool) -> String {
    let mut s = c.map_or(String::new(), |c| format!("#{c:06x}"));
    if b { s.push_str(" b"); }
    if i { s.push_str(" i"); }
    s
}
