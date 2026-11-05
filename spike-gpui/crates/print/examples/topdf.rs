//! `topdf IN.md OUT.pdf [serif] [letter]`: renders a markdown file and prints a report line.
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 3 {
        eprintln!("usage: topdf IN.md OUT.pdf [serif] [letter] [cancel-at-pct N]");
        std::process::exit(2);
    }
    let t = Instant::now();
    let doc = match mdv_reader::doc::Document::load(std::path::Path::new(&a[1])) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("load failed: {e}");
            std::process::exit(1);
        }
    };
    let load = t.elapsed();
    let mut o = mdv_print::PrintOptions { serif: a.iter().any(|x| x == "serif"), ..Default::default() };
    if a.iter().any(|x| x == "letter") {
        o.page = mdv_print::PageSize::Letter;
    }
    let cancel_at: Option<f32> = a.iter().position(|x| x == "cancel-at").and_then(|i| a.get(i + 1)).and_then(|v| v.parse().ok());
    let t = Instant::now();
    let mut last = 0usize;
    let r = mdv_print::render_pdf(&doc, &o, &mut |p| {
        if p.done / 200 != last {
            last = p.done / 200;
            eprintln!("  {:?} {}/{}", p.phase, p.done, p.total);
        }
        cancel_at.is_none_or(|c| p.fraction() < c)
    });
    let took = t.elapsed();
    match r {
        Ok((bytes, rep)) => {
            std::fs::write(&a[2], &bytes).expect("write");
            let rss = std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmHWM")).map(|l| l.to_string())).unwrap_or_default();
            println!("ok load={load:.2?} render={took:.2?} pages={} bytes={} {rss} report={rep:?}", rep.pages, bytes.len());
        }
        Err(e) => println!("error after {took:.2?}: {e}"),
    }
}
