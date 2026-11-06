//! Inputs written to end the process. A stack overflow or failed allocation can't be caught, so the
//! passing condition is that this test is still alive afterwards, and later renders still work. Run
//! it under a memory limit to see allocation failures for what they are:
//!
//! (ulimit -v 14000000; cargo test -p mdv-content --test hostile -- --nocapture)

use std::time::{Duration, Instant};

use mdv_content::{new_render_owner, render_math, render_mermaid, render_svg_image, set_render_owner, ContentError, DiagramOptions, MathOptions};

fn hwm_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())).unwrap_or(0) / 1024
}

/// Runs each case as its own document, so the limit on stuck renders per document doesn't hide
/// later ones, and waits while earlier ones hold every thread.
fn run<T>(name: &str, mut f: impl FnMut() -> Result<T, ContentError>) -> Result<T, ContentError> {
    set_render_owner(new_render_owner());
    let t = Instant::now();
    let before = hwm_mb();
    let r = loop {
        match f() {
            Err(ContentError::Busy) if t.elapsed() < Duration::from_secs(180) => std::thread::sleep(Duration::from_millis(200)),
            r => break r,
        }
    };
    let outcome = match &r {
        Ok(_) => "ok".to_string(),
        Err(e) => e.to_string(),
    };
    println!("{name:<28} {:>7.0} ms  peak +{:>5} MB  {}", t.elapsed().as_secs_f64() * 1e3, hwm_mb().saturating_sub(before), outcome.chars().take(70).collect::<String>());
    r
}

const NS: &str = r#"xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink""#;

fn svg(body: &str) -> String {
    format!(r#"<svg {NS} width="800" height="600">{body}</svg>"#)
}

fn nested(open: &str, close: &str, n: usize, inner: &str) -> String {
    format!("{}{inner}{}", open.repeat(n), close.repeat(n))
}

/// Nine levels of ten references each.
fn use_bomb() -> String {
    let mut defs = String::from(r#"<g id="l0"><rect width="10" height="10"/></g>"#);
    for i in 1..10 {
        defs.push_str(&format!(r#"<g id="l{i}">"#));
        for _ in 0..10 {
            defs.push_str(&format!(r##"<use href="#l{}"/>"##, i - 1));
        }
        defs.push_str("</g>");
    }
    svg(&format!(r##"<defs>{defs}</defs><use href="#l9"/>"##))
}

fn pattern_nest(levels: usize) -> String {
    let mut defs = String::from(r#"<pattern id="p0" width="9000" height="9000" patternUnits="userSpaceOnUse"><rect width="9000" height="9000" fill="red"/></pattern>"#);
    for i in 1..levels {
        defs.push_str(&format!(r##"<pattern id="p{i}" width="9000" height="9000" patternUnits="userSpaceOnUse"><rect width="9000" height="9000" fill="url(#p{})"/></pattern>"##, i - 1));
    }
    svg(&format!(r##"<defs>{defs}</defs><rect width="800" height="600" fill="url(#p{})"/>"##, levels - 1))
}

#[test]
fn hostile_svg_images() {
    let rect = r#"<rect width="800" height="600" fill="red"/>"#;
    let cases: Vec<(&str, String)> = vec![
        ("groups 2000 deep", svg(&nested("<g>", "</g>", 2_000, rect))),
        ("groups 100k deep", svg(&nested("<g>", "</g>", 100_000, rect))),
        ("svg 1000 deep", svg(&nested(r#"<svg width="800" height="600">"#, "</svg>", 1_000, rect))),
        ("opacity 1000 deep", svg(&nested(r#"<g opacity="0.9">"#, "</g>", 1_000, rect))),
        ("opacity 100 deep", svg(&nested(r#"<g opacity="0.9">"#, "</g>", 100, rect))),
        ("use bomb", use_bomb()),
        ("use cycle", svg(r##"<g id="a"><use href="#b"/></g><g id="b"><use href="#a"/></g>"##)),
        ("use self", svg(r##"<use id="a" href="#a"/>"##)),
        ("huge tile", svg(r##"<defs><pattern id="p" width="100000" height="100000" patternUnits="userSpaceOnUse"><rect width="9" height="9"/></pattern></defs><rect width="800" height="600" fill="url(#p)"/>"##)),
        ("tile scaled up", svg(r##"<defs><pattern id="p" width="100" height="100" patternUnits="userSpaceOnUse" patternTransform="scale(1000)"><rect width="9" height="9"/></pattern></defs><g transform="scale(100)"><rect width="8" height="6" fill="url(#p)"/></g>"##)),
        ("nested tiles", pattern_nest(30)),
        ("pattern cycle", svg(r##"<defs><pattern id="p" width="9" height="9"><rect width="9" height="9" fill="url(#q)"/></pattern><pattern id="q" width="9" height="9"><rect width="9" height="9" fill="url(#p)"/></pattern></defs><rect width="9" height="9" fill="url(#p)"/>"##)),
        ("filter 5000 steps", svg(&format!(r##"<defs><filter id="f" x="-500%" y="-500%" width="1000%" height="1000%">{}</filter></defs><rect width="800" height="600" filter="url(#f)"/>"##, r#"<feGaussianBlur stdDeviation="1"/>"#.repeat(5_000)))),
        ("huge blur", svg(r##"<defs><filter id="f" x="-1000%" y="-1000%" width="2000%" height="2000%"><feGaussianBlur stdDeviation="100000"/></filter></defs><rect width="800" height="600" filter="url(#f)"/>"##)),
        ("masks 500 deep", svg(&format!("<defs>{}</defs><rect width=\"800\" height=\"600\" mask=\"url(#m499)\"/>", (0..500).map(|i| if i == 0 { r#"<mask id="m0"><rect width="800" height="600" fill="white"/></mask>"#.to_string() } else { format!(r##"<mask id="m{i}" mask="url(#m{})"><rect width="800" height="600" fill="white"/></mask>"##, i - 1) }).collect::<String>()))),
        ("clip paths 500 deep", svg(&format!("<defs>{}</defs><rect width=\"800\" height=\"600\" clip-path=\"url(#c499)\"/>", (0..500).map(|i| if i == 0 { r#"<clipPath id="c0"><rect width="800" height="600"/></clipPath>"#.to_string() } else { format!(r##"<clipPath id="c{i}" clip-path="url(#c{})"><rect width="800" height="600"/></clipPath>"##, i - 1) }).collect::<String>()))),
        ("use chain 100k", svg(&format!("<defs><rect id=\"u0\" width=\"9\" height=\"9\"/>{}</defs><use href=\"#u99999\"/>", (1..100_000).map(|i| format!(r##"<use id="u{i}" href="#u{}"/>"##, i - 1)).collect::<String>()))),
        ("masks 100k deep", svg(&format!("<defs><mask id=\"m0\"/>{}</defs><rect width=\"8\" height=\"6\" mask=\"url(#m99999)\"/>", (1..100_000).map(|i| format!(r##"<mask id="m{i}" mask="url(#m{})"/>"##, i - 1)).collect::<String>()))),
        ("clips 100k deep", svg(&format!("<defs><clipPath id=\"c0\"/>{}</defs><rect width=\"8\" height=\"6\" clip-path=\"url(#c99999)\"/>", (1..100_000).map(|i| format!(r##"<clipPath id="c{i}" clip-path="url(#c{})"/>"##, i - 1)).collect::<String>()))),
        ("gradients 100k deep", svg(&format!("<defs><linearGradient id=\"g0\"><stop offset=\"1\"/></linearGradient>{}</defs><rect width=\"8\" height=\"6\" fill=\"url(#g99999)\"/>", (1..100_000).map(|i| format!(r##"<linearGradient id="g{i}" href="#g{}"/>"##, i - 1)).collect::<String>()))),
        ("patterns 100k deep", svg(&format!("<defs><pattern id=\"p0\" width=\"9\" height=\"9\"/>{}</defs><rect width=\"8\" height=\"6\" fill=\"url(#p99999)\"/>", (1..100_000).map(|i| format!(r##"<pattern id="p{i}" width="9" height="9"><rect width="9" height="9" fill="url(#p{})"/></pattern>"##, i - 1)).collect::<String>()))),
        ("pattern links 100k", svg(&format!("<defs><pattern id=\"p0\" width=\"9\" height=\"9\"/>{}</defs><rect width=\"8\" height=\"6\" fill=\"url(#p99999)\"/>", (1..100_000).map(|i| format!(r##"<pattern id="p{i}" href="#p{}"/>"##, i - 1)).collect::<String>()))),
        ("style chain", format!(r#"<svg {NS} width="9" height="9"><style>{}</style><rect id="m0" width="9" height="9"/>{}</svg>"#, (1..2_000).map(|i| format!("#m{i}{{mask:url(#m{})}}", i - 1)).collect::<String>(), (1..2_000).map(|i| format!(r#"<mask id="m{i}"/>"#)).collect::<String>())),
        ("markers chain", svg(&format!("<defs><marker id=\"k0\"/>{}</defs><path d=\"M0 0L9 9\" stroke=\"red\" marker-start=\"url(#k99999)\"/>", (1..100_000).map(|i| format!(r##"<marker id="k{i}"><path d="M0 0L9 9" stroke="red" marker-start="url(#k{})"/></marker>"##, i - 1)).collect::<String>()))),
        ("filters chain", svg(&format!("<defs><filter id=\"f0\"/>{}</defs><rect width=\"8\" height=\"6\" filter=\"url(#f99999)\"/>", (1..100_000).map(|i| format!(r##"<filter id="f{i}"><feImage href="#f{}"/></filter>"##, i - 1)).collect::<String>()))),
        ("text paths", svg(&format!("<defs><path id=\"t0\" d=\"M0 0L9 9\"/>{}</defs><text><textPath href=\"#t99999\">x</textPath></text>", (1..100_000).map(|i| format!(r##"<text id="t{i}"><textPath href="#t{}">x</textPath></text>"##, i - 1)).collect::<String>()))),
        ("side 1e9", r#"<svg xmlns="http://www.w3.org/2000/svg" width="1e9" height="1e9"/>"#.to_string()),
        ("side 16384", r#"<svg xmlns="http://www.w3.org/2000/svg" width="16384" height="16384"><rect width="9" height="9"/></svg>"#.to_string()),
        ("thin 16384", r#"<svg xmlns="http://www.w3.org/2000/svg" width="16384" height="1"><rect width="9" height="9"/></svg>"#.to_string()),
        ("zero size", r#"<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"/>"#.to_string()),
        ("nan size", r#"<svg xmlns="http://www.w3.org/2000/svg" width="NaN" height="-5"/>"#.to_string()),
        ("stroke 1e30", svg(r#"<path d="M0 0L1e30 1e30" stroke="red" stroke-width="1e30" stroke-dasharray="0.0001"/>"#)),
        ("dash tiny", svg(r#"<path d="M0 0L800 600" stroke="red" stroke-width="2" stroke-dasharray="0.00001 0.00001"/>"#)),
        ("gradient stops", svg(&format!(r##"<defs><linearGradient id="g">{}</linearGradient></defs><rect width="800" height="600" fill="url(#g)"/>"##, r#"<stop offset="0.5" stop-color="red"/>"#.repeat(200_000)))),
        ("text 1 MB", svg(&format!("<text y=\"20\">{}</text>", "w ".repeat(500_000)))),
        ("font size 1e9", svg(r#"<text y="20" font-size="1e9">x</text>"#)),
        ("embedded svg", svg(r#"<image href="data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=" width="9" height="9"/>"#)),
        ("linked file", svg(r#"<image href="/etc/passwd" width="9" height="9"/><image xlink:href="file:///etc/passwd" width="9" height="9"/>"#)),
        ("linked css", format!(r#"<svg {NS} width="9" height="9"><style>@import url(http://a.example/x.css); * {{ fill: url(http://a.example/x.svg#a) }}</style><rect width="9" height="9"/></svg>"#)),
        ("dtd", r#"<!DOCTYPE svg [<!ENTITY a "aaaa"><!ENTITY b "&a;&a;&a;&a;">]><svg xmlns="http://www.w3.org/2000/svg">&b;</svg>"#.to_string()),
        ("not xml", "<svg <<<>>>".to_string()),
        ("not utf-8", "<svg \u{0}\u{1}".to_string()),
    ];
    for (name, doc) in &cases {
        let r = run(name, || render_svg_image(doc.as_bytes()));
        if let Ok(img) = &r {
            assert_eq!(img.image.pixels.len(), img.image.width as usize * img.image.height as usize * 4, "{name}");
        }
    }
    set_render_owner(new_render_owner());
    assert!(render_svg_image(svg(rect).as_bytes()).is_ok(), "later images still draw");
    assert!(hwm_mb() < 10_000, "peak {} MB", hwm_mb());
}

#[test]
fn hostile_diagrams() {
    let many = |n: usize, f: &dyn Fn(usize) -> String| (0..n).map(f).collect::<String>();
    let cases: Vec<(&str, String)> = vec![
        ("subgraphs 490 deep", format!("flowchart TD\n{}X-->Y\n{}", "subgraph s\n".repeat(490), "end\n".repeat(490))),
        ("state 240 deep", format!("stateDiagram-v2\n{}[*]-->a\n{}", many(240, &|i| format!("state s{i} {{\n")), "}\n".repeat(240))),
        ("mindmap 300 deep", format!("mindmap\n{}", many(300, &|i| format!("{}n{i}\n", " ".repeat(i + 1))))),
        ("block 490 deep", format!("block-beta\n{}a\n{}", many(490, &|i| format!("block:b{i}\n")), "end\n".repeat(490))),
        ("sequence 490 deep", format!("sequenceDiagram\n{}A->>B: x\n{}", "loop x\n".repeat(490), "end\n".repeat(490))),
        ("c4 400 deep", format!("C4Context\n{}{}", many(400, &|i| format!("Boundary(b{i}, \"x\") {{\n")), "}\n".repeat(400))),
        ("treemap 300 deep", format!("treemap-beta\n{}", many(300, &|i| format!("{}\"n{i}\"\n", " ".repeat(i))))),
        ("class namespaces", format!("classDiagram\n{}", "namespace a {\n".repeat(400))),
        ("nested brackets", format!("flowchart LR\n A[\"{}\"]-->B", "(".repeat(40_000))),
        ("brackets unquoted", format!("flowchart LR\n A{}", "[".repeat(40_000))),
        ("parens", format!("flowchart LR\n A{}x{}", "(".repeat(20_000), ")".repeat(20_000))),
        ("gantt centuries", "gantt\n dateFormat YYYY\n section s\n a :0001, 9999\n b :0001, 999999999d\n".to_string()),
        ("gantt tiny tick", "gantt\n dateFormat YYYY-MM-DD\n tickInterval 1millisecond\n section s\n a :2000-01-01, 2030-01-01\n".to_string()),
        ("packet 4 billion", "packet-beta\n 0-4294967295: \"big\"\n".to_string()),
        ("packet row bits", "packet-beta\n bitsPerRow 1000000000\n 0-9: \"x\"\n".to_string()),
        ("block columns", "block-beta\n columns 1000000000\n a b c\n".to_string()),
        ("block span", "block-beta\n a:1000000000\n".to_string()),
        ("xychart range", "xychart-beta\n x-axis 0 --> 1e300\n y-axis 0 --> 1e300\n bar [1e300, -1e300, 1e-300]\n".to_string()),
        ("radar", format!("radar-beta\n axis {}\n curve c{{{}}}\n max 1e300\n", many(500, &|i| format!("a{i},")), many(500, &|_| "1e300,".to_string()))),
        ("sankey huge", "sankey-beta\na,b,1e308\nb,a,1e308\n".to_string()),
        ("sankey cycle", many(1, &|_| "sankey-beta\na,b,1\nb,c,1\nc,a,1\n".to_string())),
        ("git branches", format!("gitGraph\n{}", many(900, &|i| format!("branch b{i}\ncommit\n")))),
        ("er self links", format!("erDiagram\n{}", many(490, &|i| format!("A{i} ||--o{{ A{i} : x\n")))),
        ("font size huge", "flowchart LR\n classDef c font-size:1e9px,stroke-width:1e9px\n A:::c-->B\n".to_string()),
        ("style huge", "flowchart LR\n A-->B\n style A width:1e30px,height:1e30px\n".to_string()),
        ("quadrant", format!("quadrantChart\n{}", many(990, &|i| format!("P{i}: [1e300, -1e300]\n")))),
        ("kanban", format!("kanban\n{}", many(990, &|i| format!("{}c{i}[x]\n", " ".repeat(i % 50))))),
        ("architecture nest", format!("architecture-beta\n{}", many(490, &|i| format!("group g{i}(cloud)[x] in g{}\n", i + 1)))),
    ];
    let opts = DiagramOptions::default();
    for (name, doc) in &cases {
        let _ = run(name, || render_mermaid(doc, &opts));
    }
    set_render_owner(new_render_owner());
    assert!(render_mermaid("flowchart LR\n A-->B", &opts).is_ok(), "later diagrams still draw");
    println!("peak {} MB", hwm_mb());
}

#[test]
fn hostile_maths() {
    let cases: Vec<(&str, String)> = vec![
        ("braces", format!("{}x{}", "{".repeat(10_000), "}".repeat(10_000))),
        ("fractions", format!("{}x{}", "\\frac{1}{".repeat(2_000), "}".repeat(2_000))),
        ("roots", format!("{}x{}", "\\sqrt{".repeat(3_000), "}".repeat(3_000))),
        ("powers", format!("x{}1{}", "^{".repeat(6_000), "}".repeat(6_000))),
        ("left right", format!("{}x{}", "\\left(".repeat(2_000), "\\right)".repeat(2_000))),
        ("environments", format!("{}x{}", "\\begin{matrix}".repeat(1_000), "\\end{matrix}".repeat(1_000))),
        ("arrays", format!("{}x{}", "\\begin{array}{c}".repeat(1_000), "\\end{array}".repeat(1_000))),
        ("macro loop", "\\def\\x{\\x\\x}\\x".to_string()),
        ("macro bomb", "\\def\\a{\\b\\b}\\def\\b{\\c\\c}\\def\\c{\\d\\d}\\def\\d{\\e\\e}\\def\\e{\\f\\f}\\def\\f{\\g\\g}\\def\\g{xxxxxxxx}\\a\\a\\a\\a".to_string()),
        ("huge rule", "\\rule{1e30em}{1e30em}".to_string()),
        ("huge kern", "\\kern{1e30em}x\\hspace{1e30em}\\mkern{1e30mu}".to_string()),
        ("huge size", "\\Huge\\Huge\\Huge\\scalebox{1e30}{x}\\raisebox{1e30em}{x}".to_string()),
        ("matrix wide", format!("\\begin{{matrix}}{}\\end{{matrix}}", "a&".repeat(9_000))),
        ("matrix tall", format!("\\begin{{matrix}}{}\\end{{matrix}}", "a\\\\".repeat(6_000))),
    ];
    let o = MathOptions::default();
    for (name, doc) in &cases {
        let _ = run(name, || render_math(doc, true, &o));
    }
    set_render_owner(new_render_owner());
    assert!(render_math("x^2", false, &o).is_ok(), "later maths still draws");
}
