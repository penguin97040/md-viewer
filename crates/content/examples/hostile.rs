//! Hostile-input battery. Prints one line per case: outcome, time, and peak-RSS growth, then
//! re-renders ordinary inputs to show later renders still work. Independent of `svgcheck`, every
//! successful SVG is also scanned for network or script markers.
//!
//! cargo run --release -p mdv-content --example hostile

use std::time::Instant;

use mdv_content::{render_math, render_mermaid, ContentError, ContentTheme, DiagramOptions, MathOptions, RenderedAsset};

fn hwm_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()))
        .unwrap_or(0)
}

fn scan(a: &RenderedAsset) -> String {
    // Markup is checked separately from text: a label that spells out `<script>` is only text.
    let doc = match roxmltree::Document::parse(&a.svg) {
        Ok(d) => d,
        Err(e) => return format!("NOT WELL-FORMED: {e}"),
    };
    let mut markup = vec![];
    let mut text_hits = false;
    for n in doc.descendants() {
        if n.is_text() {
            let t = n.text().unwrap_or("").to_ascii_lowercase();
            if ["<script", "javascript:", "onerror", "onload", "http://", "https://"].iter().any(|m| t.contains(m)) {
                text_hits = true;
            }
        } else if n.is_element() {
            let name = n.tag_name().name();
            if ["script", "foreignObject", "iframe", "a", "image", "animate", "set", "handler"].contains(&name) {
                markup.push(format!("<{name}>"));
            }
            for at in n.attributes() {
                let v = at.value().to_ascii_lowercase();
                if at.name().starts_with("on") || v.contains("javascript:") || v.contains("http:") || v.contains("https:") || at.name() == "href" && !v.starts_with('#') {
                    markup.push(format!("{}={}", at.name(), &v[..v.len().min(30)]));
                }
            }
            if n.tag_name().namespace().map_or(false, |ns| ns != "http://www.w3.org/2000/svg") {
                markup.push("foreign namespace".into());
            }
        }
    }
    let fonts: std::collections::BTreeSet<String> = doc.descendants().filter_map(|n| n.attribute("font-family").map(|f| f.to_string())).collect();
    let tail = format!("fonts={fonts:?}");
    if !markup.is_empty() {
        format!("MARKUP: {markup:?}")
    } else if text_hits {
        format!("clean (marker strings appear only as visible text) {tail}")
    } else {
        format!("clean {tail}")
    }
}

fn report(kind: &str, name: &str, r: Result<RenderedAsset, ContentError>, ms: f64, d_rss: i64) {
    match r {
        Ok(a) => println!("{kind}\t{name}\tOK {:.0}x{:.0}\tsvg={}B\t{}\t{ms:.1} ms\tdRSS={d_rss} kB", a.width_px, a.height_px, a.svg.len(), scan(&a)),
        Err(e) => {
            let mut m = e.to_string();
            m.truncate(90);
            let variant = format!("{e:?}");
            let variant = variant.split('(').next().unwrap_or("").to_string();
            println!("{kind}\t{name}\tERR[{variant}] {m}\t{ms:.1} ms\tdRSS={d_rss} kB");
        }
    }
}

fn main() {
    let dopts = DiagramOptions { theme: ContentTheme::Dark, ..Default::default() };
    let mopts = MathOptions::default();
    let mut mer: Vec<(String, String)> = vec![];
    let add = |v: &mut Vec<(String, String)>, n: &str, s: String| v.push((n.to_string(), s));
    add(&mut mer, "init-font-theme-css", "%%{init: {\"fontFamily\":\"Comic Sans\",\"theme\":\"forest\",\"themeCSS\":\"@import url('https://evil.example/x.css');\",\"themeVariables\":{\"primaryColor\":\"#f00\"}}}%%\nflowchart LR\n  A-->B".into());
    add(&mut mer, "init-malformed", "%%{init: {oops\nflowchart LR\n A-->B".into());
    add(&mut mer, "html-label-img-onerror", "flowchart LR\n  A[\"<img src=x onerror=alert(1)>\"] --> B".into());
    add(&mut mer, "html-label-script", "flowchart LR\n  A[\"<script>alert(1)</script>\"] --> B[\"<iframe src='http://evil.example'></iframe>\"]".into());
    add(&mut mer, "html-label-anchor-js", "flowchart LR\n  A[\"<a href='javascript:alert(1)'>x</a>\"] --> B".into());
    add(&mut mer, "label-svg-foreignobject", "flowchart LR\n  A[\"</text><foreignObject><div>x</div></foreignObject><text>\"] --> B".into());
    add(&mut mer, "label-xml-breakout", "flowchart LR\n  A[\"x\\\" onload=\\\"alert(1)\"] --> B[\"]]></text><script>1</script>\"]".into());
    add(&mut mer, "click-link-callback", "flowchart LR\n  A-->B\n  click A \"https://evil.example\" \"tip\"\n  click B call alert()".into());
    add(&mut mer, "style-url-external", "flowchart LR\n  A-->B\n  style A fill:url(https://evil.example/x.svg#a),stroke:#f00\n  classDef x fill:#f00;}</style><script>alert(1)</script>\n  class B x".into());
    add(&mut mer, "architecture-iconify", "architecture-beta\n  service a(logos:aws)[A]\n  service b(mdi:web)[B]\n  a:R -- L:b".into());
    add(&mut mer, "empty", "".into());
    add(&mut mer, "whitespace", "  \n\t\n".into());
    add(&mut mer, "unknown-type", "fooDiagram\n  a-->b".into());
    add(&mut mer, "nul-bytes", "flowchart LR\n A[\"a\u{0}b\"] --> B\u{0}".into());
    add(&mut mer, "emoji-cjk-labels", "flowchart LR\n A[\"日本語 🙂\"] --> B[\"kia ora 🚀\"]".into());
    add(&mut mer, "label-40kB-word", format!("flowchart LR\n A[\"{}\"] --> B", "w".repeat(40_000)));
    add(&mut mer, "src-60kB", format!("flowchart LR\n{}", " A-->B\n".repeat(8_600)));
    add(&mut mer, "lines-2500", format!("flowchart LR\n{}", (0..2_500).map(|i| format!(" N{i}-->N{}\n", i + 1)).collect::<String>()));
    add(&mut mer, "chain-1900-nodes", format!("flowchart LR\n{}", (0..1_900).map(|i| format!(" N{i}-->N{}\n", i + 1)).collect::<String>()));
    add(&mut mer, "one-line-3000-edges", format!("flowchart LR\n {}", (0..3_000).map(|i| format!("N{i}-->N{};", i + 1)).collect::<String>()));
    add(&mut mer, "fan-out-500", format!("flowchart LR\n{}", (0..500).map(|i| format!(" R-->L{i}\n")).collect::<String>()));
    add(&mut mer, "dense-60x60-bipartite", format!("flowchart LR\n{}", (0..60).flat_map(|i| (0..60).map(move |j| format!(" A{i}-->B{j}\n"))).collect::<String>()));
    add(&mut mer, "subgraph-nested-150", {
        let mut s = String::from("flowchart TD\n");
        for i in 0..150 { s.push_str(&format!("{}subgraph S{i}\n", " ".repeat(1))); }
        s.push_str(" X-->Y\n");
        for _ in 0..150 { s.push_str(" end\n"); }
        s
    });
    add(&mut mer, "mindmap-depth-400", { let mut s = String::from("mindmap\n root\n"); for i in 0..400 { s.push_str(&format!("{}n{i}\n", " ".repeat(i + 2))); } s });
    add(&mut mer, "state-nested-100", { let mut s = String::from("stateDiagram-v2\n"); for i in 0..100 { s.push_str(&format!("state S{i} {{\n")); } s.push_str("[*] --> a\n"); for _ in 0..100 { s.push_str("}\n"); } s });
    add(&mut mer, "sequence-3000-msgs", format!("sequenceDiagram\n{}", (0..1_900).map(|i| format!(" A->>B: m{i}\n")).collect::<String>()));
    add(&mut mer, "pie-1000-slices", format!("pie\n{}", (0..1_000).map(|i| format!(" \"s{i}\" : {}\n", i + 1)).collect::<String>()));
    add(&mut mer, "gantt-1500-tasks", format!("gantt\n dateFormat YYYY-MM-DD\n section s\n{}", (0..1_500).map(|i| format!(" t{i} :2026-01-01, 1d\n")).collect::<String>()));
    add(&mut mer, "gantt-huge-duration", "gantt\n dateFormat YYYY-MM-DD\n section s\n a :2026-01-01, 999999999d\n b :2026-01-01, 3d".into());
    add(&mut mer, "xychart-huge-values", "xychart-beta\n x-axis [a,b]\n y-axis \"v\" 0 --> 1e300\n bar [1e300, 5]".into());
    add(&mut mer, "pie-nan-negative", "pie\n \"a\" : -5\n \"b\" : 0".into());
    add(&mut mer, "packet-huge-range", "packet-beta\n 0-99999999: \"big\"".into());
    add(&mut mer, "block-columns-100000", "block-beta\n columns 100000\n a b c".into());
    add(&mut mer, "timeline-1000-periods", format!("timeline\n{}", (0..1_000).map(|i| format!(" {i} : e\n")).collect::<String>()));
    println!("-- Mermaid ({} cases), dark theme", mer.len());
    for (n, src) in &mer {
        let h0 = hwm_kb() as i64;
        let t = Instant::now();
        let r = render_mermaid(src, &dopts);
        report("mermaid", n, r, t.elapsed().as_secs_f64() * 1000.0, hwm_kb() as i64 - h0);
    }

    let mut mat: Vec<(String, String)> = vec![];
    add(&mut mat, "braces-nest-100000", format!("{}x{}", "{".repeat(100_000), "}".repeat(100_000)));
    add(&mut mat, "braces-nest-9000", format!("{}x{}", "{".repeat(9_000), "}".repeat(9_000)));
    add(&mut mat, "frac-nest-5000", format!("{}x{}", "\\frac{1}{".repeat(2_000), "}".repeat(2_000)));
    for d in [10usize, 20, 30, 40, 50, 60, 100] {
        add(&mut mat, &format!("frac-nest-{d}"), format!("{}x{}", "\\frac{1}{".repeat(d), "}".repeat(d)));
    }
    add(&mut mat, "frac-nest-200", format!("{}x{}", "\\frac{1}{".repeat(200), "}".repeat(200)));
    add(&mut mat, "sqrt-nest-3000", format!("{}x{}", "\\sqrt{".repeat(3_000), "}".repeat(3_000)));
    add(&mut mat, "sup-nest-3000", format!("x{}1{}", "^{".repeat(3_000), "}".repeat(3_000)));
    add(&mut mat, "left-right-nest-2000", format!("{}x{}", "\\left(".repeat(2_000), "\\right)".repeat(2_000)));
    add(&mut mat, "begin-nest-1000", format!("{}x{}", "\\begin{matrix}".repeat(1_000), "\\end{matrix}".repeat(1_000)));
    add(&mut mat, "unbalanced-open", "{".repeat(50_000));
    add(&mut mat, "macro-infinite", "\\def\\x{\\x\\x}\\x".into());
    add(&mut mat, "macro-exponential", "\\def\\a{\\b\\b}\\def\\b{\\c\\c}\\def\\c{\\d\\d}\\def\\d{\\e\\e}\\def\\e{\\f\\f}\\def\\f{\\g\\g}\\def\\g{\\h\\h}\\def\\h{\\i\\i}\\def\\i{\\j\\j}\\def\\j{\\k\\k}\\def\\k{\\l\\l}\\def\\l{\\m\\m}\\def\\m{\\n\\n}\\def\\n{x}\\a".into());
    add(&mut mat, "macro-900-expansions", format!("\\def\\a{{x}}{}", "\\a".repeat(900)));
    add(&mut mat, "macro-1100-expansions", format!("\\def\\a{{x}}{}", "\\a".repeat(1_100)));
    add(&mut mat, "macro-arg-bomb", "\\def\\d#1{#1#1}\\def\\e#1{\\d{\\d{\\d{\\d{\\d{\\d{\\d{\\d{\\d{\\d{\\d{#1}}}}}}}}}}}}\\e{\\e{x}}".into());
    add(&mut mat, "rule-1000em", "\\rule{1000em}{1000em}".into());
    add(&mut mat, "rule-50em", "\\rule{50em}{50em}".into());
    add(&mut mat, "rule-60em-x-1em", "a\\rule{60em}{1em}b".into());
    add(&mut mat, "kern-1e9em", "a\\kern{1000000000em}b".into());
    add(&mut mat, "hspace-1e9em", "a\\hspace{1000000000em}b".into());
    add(&mut mat, "vspace-raise", "a\\raisebox{100000em}{b}".into());
    add(&mut mat, "phantom-huge", "\\hphantom{\\rule{900em}{1em}}x".into());
    add(&mut mat, "href-javascript", "\\href{javascript:alert(1)}{click}".into());
    add(&mut mat, "url-external", "\\url{http://evil.example/x}".into());
    add(&mut mat, "includegraphics", "\\includegraphics[width=1em]{http://evil.example/x.png}".into());
    add(&mut mat, "html-extension", "\\htmlClass{evil}{x}\\htmlStyle{color:red}{y}\\htmlData{a=b}{z}\\htmlId{i}{w}".into());
    add(&mut mat, "src-20kB-letters", "x+".repeat(10_000));
    add(&mut mat, "src-19kB-glyph-heavy", "\\sum\\int\\oint\\prod\\bigcup".repeat(700));
    add(&mut mat, "src-1MB", "x".repeat(1_000_000));
    add(&mut mat, "nul-and-controls", "a\u{0}b\u{1}c\u{7f}d".into());
    add(&mut mat, "unicode-astral", "𝔸𝕏 \\text{😀 日本語}".into());
    add(&mut mat, "matrix-60x60", format!("\\begin{{matrix}}{}\\end{{matrix}}", (0..60).map(|_| (0..60).map(|_| "1").collect::<Vec<_>>().join("&")).collect::<Vec<_>>().join("\\\\")));
    add(&mut mat, "array-hline-spam", format!("\\begin{{array}}{{c}}{}\\end{{array}}", "\\hline x \\\\".repeat(3_000)));
    add(&mut mat, "dashed-tall", "\\begin{array}{c:c}x&y\\\\ \\hline\\end{array}".into());
    add(&mut mat, "empty", "".into());
    println!("-- Maths ({} cases), default options", mat.len());
    for (n, src) in &mat {
        let h0 = hwm_kb() as i64;
        let t = Instant::now();
        let r = render_math(src, true, &mopts);
        report("math", n, r, t.elapsed().as_secs_f64() * 1000.0, hwm_kb() as i64 - h0);
    }
    println!("-- Recovery after the battery");
    let t = Instant::now();
    report("math", "after: simple", render_math("e^{i\\pi}+1=0", false, &mopts), t.elapsed().as_secs_f64() * 1000.0, 0);
    let t = Instant::now();
    report("mermaid", "after: simple", render_mermaid("flowchart LR\n A-->B", &dopts), t.elapsed().as_secs_f64() * 1000.0, 0);
    println!("peak RSS (VmHWM) at end: {} kB", hwm_kb());
}
