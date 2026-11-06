//! Times the raw merman call (no size limits, no guard) on growing synthetic inputs, with the same
//! embedded-font measurement the crate uses. Prints seconds per size; stops a series above 20 s.
//!
//! cargo run --release -p mdv-content --features merman --example scale_merman [series]
use std::time::Instant;

use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment, SvgRequest};
use merman_export::{NativeFontContext, NativeFontLimits};

const FONTS: [&[u8]; 3] = [
    include_bytes!("../../../assets/fonts/Geist-Regular.ttf"),
    include_bytes!("../../../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../../../assets/fonts/GeistMono-Regular.ttf"),
];

fn main() {
    let only = std::env::args().nth(1);
    std::thread::Builder::new().stack_size(64 << 20).spawn(move || run(only)).unwrap().join().unwrap();
}

fn run(only: Option<String>) {
    let ctx = NativeFontContext::from_font_bytes(FONTS.iter().map(|b| b.to_vec()), NativeFontLimits::default(), &OperationControl::new()).unwrap();
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
        ("label-chars", Box::new(|n| format!("flowchart LR\n A[\"{}\"] --> B", "w".repeat(n * 10)))),
        ("label-words", Box::new(|n| format!("flowchart LR\n A[\"{}\"] --> B", "word ".repeat(n * 2)))),
    ];
    for (name, f) in &series {
        if only.as_deref().map_or(false, |o| o != *name) {
            continue;
        }
        for n in [25usize, 50, 100, 200, 300, 500, 800, 1200, 2000, 4000] {
            let src = f(n);
            let mut req = SvgRequest::default();
            req.environment = SvgEnvironment::deterministic().with_native_font_context(ctx.clone()).unwrap();
            let t = Instant::now();
            let r = Renderer::new().render(RenderRequest::document(&src, OperationControl::new(), req));
            let s = t.elapsed().as_secs_f64();
            let ok = matches!(r, Ok(RenderOutput::Document(Some(_))));
            println!("{name}\tn={n}\t{s:.3} s\t{}", if ok { "ok" } else { "err" });
            if s > 20.0 {
                break;
            }
        }
    }
}
