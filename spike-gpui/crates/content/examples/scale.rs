//! Times the raw mermaid-rs-renderer call (no guard) on growing synthetic flowcharts, to choose
//! the size limits. Prints seconds per size; stops a series after it exceeds 20 s.
use std::time::Instant;
use mermaid_rs_renderer::{render_with_options, RenderOptions};

fn main() {
    let mut o = RenderOptions::mermaid_default();
    o.layout.fast_text_metrics = true;
    let series: Vec<(&str, Box<dyn Fn(usize) -> String>)> = vec![
        ("chain-LR", Box::new(|n| format!("flowchart LR\n{}", (0..n).map(|i| format!(" N{i}-->N{}\n", i + 1)).collect::<String>()))),
        ("chain-TD", Box::new(|n| format!("flowchart TD\n{}", (0..n).map(|i| format!(" N{i}-->N{}\n", i + 1)).collect::<String>()))),
        ("fan-out", Box::new(|n| format!("flowchart LR\n{}", (0..n).map(|i| format!(" R-->L{i}\n")).collect::<String>()))),
        ("tree", Box::new(|n| format!("flowchart TD\n{}", (1..=n).map(|i| format!(" N{}-->N{i}\n", i / 2)).collect::<String>()))),
        ("sequence", Box::new(|n| format!("sequenceDiagram\n{}", (0..n).map(|i| format!(" A->>B: m{i}\n")).collect::<String>()))),
        ("pie", Box::new(|n| format!("pie\n{}", (0..n).map(|i| format!(" \"s{i}\" : {}\n", i + 1)).collect::<String>()))),
        ("gantt", Box::new(|n| format!("gantt\n dateFormat YYYY-MM-DD\n section s\n{}", (0..n).map(|i| format!(" t{i} :2026-01-01, 1d\n")).collect::<String>()))),
        ("class", Box::new(|n| format!("classDiagram\n{}", (0..n).map(|i| format!(" C{i} <|-- C{}\n", i + 1)).collect::<String>()))),
        ("state", Box::new(|n| format!("stateDiagram-v2\n{}", (0..n).map(|i| format!(" S{i} --> S{}\n", i + 1)).collect::<String>()))),
        ("er", Box::new(|n| format!("erDiagram\n{}", (0..n).map(|i| format!(" E{i} ||--o{{ E{} : r\n", i + 1)).collect::<String>()))),
        ("mindmap", Box::new(|n| format!("mindmap\n root\n{}", (0..n).map(|i| format!("  c{i}\n")).collect::<String>()))),
    ];
    let only = std::env::args().nth(1);
    for (name, f) in &series {
        if only.as_deref().map_or(false, |o| o != *name) { continue; }
        for n in [25usize, 50, 100, 200, 300, 500, 800, 1200] {
            let src = f(n);
            let t = Instant::now();
            let r = render_with_options(&src, o.clone());
            let s = t.elapsed().as_secs_f64();
            println!("{name}\tn={n}\t{:.3} s\t{}", s, if r.is_ok() { "ok" } else { "err" });
            if s > 20.0 { break; }
        }
    }
}
