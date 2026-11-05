use std::sync::Arc;

use mermaid_rs_renderer::{render_with_options, RenderOptions, Theme};

use crate::{guard, limits, svgcheck, ContentError, ContentTheme, DiagramOptions, RenderedAsset};

pub(crate) fn render(src: &str, opts: &DiagramOptions) -> Result<RenderedAsset, ContentError> {
    if src.len() > limits::MAX_MERMAID_BYTES {
        return Err(ContentError::TooLarge(format!("diagram source is {} bytes (limit {})", src.len(), limits::MAX_MERMAID_BYTES)));
    }
    let clean = strip_directives(&crate::strip_xml_illegal(src));
    let kind = kind(&clean);
    let statements = clean.lines().filter(|l| !l.trim().is_empty()).count() + clean.matches(';').count();
    let (cap, what) = if kind.starts_with("flowchart") || kind.starts_with("graph") {
        // Layout time grows faster than linearly: 100 fan-out edges took 2.1 s, 200 took 6.8 s.
        let edges = ["--", "==", "-.", "~~~"].iter().map(|o| clean.matches(o).count()).sum::<usize>();
        if edges > limits::MAX_FLOWCHART_EDGES {
            return Err(ContentError::TooLarge(format!("flowchart has about {edges} edges (limit {})", limits::MAX_FLOWCHART_EDGES)));
        }
        (limits::MAX_MERMAID_STATEMENTS, "statements")
    } else if kind.starts_with("state") || kind.starts_with("class") || kind.starts_with("erdiagram") {
        (limits::MAX_GRAPH_STATEMENTS, "statements")
    } else {
        (limits::MAX_MERMAID_STATEMENTS, "statements")
    };
    if statements > cap {
        return Err(ContentError::TooLarge(format!("diagram has {statements} {what} (limit {cap})")));
    }
    let clean = if strips_click(&kind) { strip_click(&clean) } else { clean };
    let mut opts = opts.clone();
    let mut card = false;
    if opts.theme == ContentTheme::Dark && hard_codes_light_colours(&kind) {
        opts.theme = ContentTheme::Light;
        card = true;
    }
    let ro = options(&opts, card);
    guard::run(&guard::MERMAID_ABANDONED, limits::MERMAID_BUDGET, move || {
        let svg = render_with_options(&clean, ro).map_err(|e| ContentError::Parse(e.to_string()))?;
        let svg = escape_bare_ampersands(svg);
        svgcheck::check(&svg)?;
        let (w, h) = size(&svg)?;
        if w > limits::MAX_DIAGRAM_PX || h > limits::MAX_DIAGRAM_PX {
            return Err(ContentError::TooLarge(format!("diagram is {w:.0} x {h:.0} px")));
        }
        Ok(RenderedAsset { width_px: w, height_px: h, baseline_px: h, svg: Arc::from(svg) })
    })
}

/// Mermaid.js blocks `fontFamily`, `themeCSS` and `themeVariables` in `%%{init}%%` (see
/// `secure` in extras.js) but lets `theme` through. Honouring none of it keeps the look under the
/// app's control, so every `%%{ ... }%%` line is dropped before the renderer sees it.
fn strip_directives(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let t = line.trim_start();
        if t.starts_with("%%{") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// mermaid-rs-renderer 0.3.1 leaves a literal `&` unescaped in some labels (found by fuzzing:
/// quadrant axis titles), which makes the SVG ill-formed. Escaping a bare `&` only turns it into
/// literal text, so it is safe to repair here.
fn escape_bare_ampersands(svg: String) -> String {
    if !svg.contains('&') {
        return svg;
    }
    let mut out = String::with_capacity(svg.len() + 16);
    let mut rest = svg.as_str();
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let known = tail.find(';').map_or(false, |e| {
            let name = &tail[1..e];
            e <= 10 && (matches!(name, "amp" | "lt" | "gt" | "quot" | "apos") || name.strip_prefix('#').map_or(false, |n| (!n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) || n.strip_prefix('x').map_or(false, |h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit()))))
        });
        out.push_str(if known { "&" } else { "&amp;" });
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// `click` lines attach links and callbacks. Mermaid.js ignores them at `securityLevel: strict`;
/// the renderer here would emit `<a>` elements that the SVG check rejects, losing the diagram.
fn strips_click(kind: &str) -> bool {
    kind.starts_with("flowchart") || kind.starts_with("graph") || kind.starts_with("class") || kind.starts_with("state")
}

fn strip_click(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.lines() {
        let t = line.trim_start().to_ascii_lowercase();
        if t.starts_with("click ") || t.starts_with("click\t") {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// First diagram keyword, lower-cased, skipping blank lines, `%%` comments and a `---` front
/// matter block.
fn kind(src: &str) -> String {
    let mut in_front = false;
    for line in src.lines() {
        let t = line.trim();
        if t == "---" {
            in_front = !in_front;
            continue;
        }
        if in_front || t.is_empty() || t.starts_with("%%") {
            continue;
        }
        return t.split(|c: char| c.is_whitespace() || c == ';').next().unwrap_or("").to_ascii_lowercase();
    }
    String::new()
}

/// mermaid-rs-renderer writes some light-theme colours into the SVG as literals (quadrant text
/// `#131300`, timeline pastel boxes under theme text, C4 labels). In dark mode these are drawn in
/// the light scheme on a light card instead of unreadable text on black.
fn hard_codes_light_colours(k: &str) -> bool {
    k.starts_with("quadrantchart") || k.starts_with("timeline") || k.starts_with("c4")
}

fn options(opts: &DiagramOptions, card: bool) -> RenderOptions {
    let mut o = RenderOptions::mermaid_default();
    o.theme = theme(opts, card);
    // Deterministic character-width estimates for ASCII text; no system fonts are read and no
    // font cache is written to disk. (Non-ASCII labels still reach the renderer's system-font
    // lookup; see the spike report.)
    o.layout.fast_text_metrics = true;
    o
}

fn hex(rgb: u32) -> String {
    format!("#{:06x}", rgb & 0xff_ffff)
}

/// Colours copied from the CSS Mermaid.js 12.1.0 emits for its `dark` and `default` themes (read
/// from the `<style>` block of SVGs rendered in headless Chrome with the app's configuration).
fn theme(opts: &DiagramOptions, card: bool) -> Theme {
    let mut t = Theme::mermaid_default();
    t.font_family = "Chakra Petch".into();
    t.font_size = opts.font_size;
    t.background = if card { hex(mdv_theme::BRAND_TEXT) } else { "none".into() };
    let s = |v: &str| v.to_string();
    match opts.theme {
        ContentTheme::Dark => {
            t.primary_color = s("#1f2020");
            t.primary_text_color = s("#cccccc");
            t.text_color = s("#cccccc");
            t.primary_border_color = s("#cccccc");
            t.line_color = s("#d3d3d3");
            t.secondary_color = s("#474949");
            t.tertiary_color = s("#474949");
            t.edge_label_background = s("#585858");
            t.cluster_background = s("#474949");
            t.cluster_border = s("#6b6b6b");
            t.sequence_actor_fill = s("#1f2020");
            t.sequence_actor_border = s("#cccccc");
            t.sequence_actor_line = s("#cccccc");
            t.sequence_note_fill = s("#474949");
            t.sequence_note_border = s("#2f2f2f");
            t.sequence_activation_fill = s("#474949");
            t.sequence_activation_border = s("#cccccc");
            t.pie_title_text_color = s("#cccccc");
            t.pie_section_text_color = s("#cccccc");
            t.pie_legend_text_color = s("#cccccc");
            t.pie_stroke_color = s("#cccccc");
            t.pie_outer_stroke_color = s("#cccccc");
        }
        ContentTheme::Light | ContentTheme::Print => {
            t.primary_color = s("#ececff");
            t.primary_text_color = s("#333333");
            t.text_color = s("#333333");
            t.primary_border_color = s("#9370db");
            t.line_color = s("#333333");
            t.secondary_color = s("#ffffde");
            t.edge_label_background = s("#e8e8e8");
            t.cluster_background = s("#ffffde");
            t.cluster_border = s("#aaaa33");
            t.sequence_actor_fill = s("#ececff");
            t.sequence_actor_border = s("#9370db");
            t.sequence_actor_line = s("#9370db");
            t.sequence_note_fill = s("#fff5ad");
            t.sequence_note_border = s("#aaaa33");
            t.sequence_activation_fill = s("#f4f4f4");
            t.sequence_activation_border = s("#666666");
        }
    }
    if opts.theme == ContentTheme::Print {
        for f in [&mut t.primary_text_color, &mut t.text_color, &mut t.pie_title_text_color, &mut t.pie_section_text_color, &mut t.pie_legend_text_color] {
            *f = s("#000000");
        }
    }
    if let Some(c) = opts.text_color {
        for f in [&mut t.primary_text_color, &mut t.text_color, &mut t.pie_title_text_color, &mut t.pie_section_text_color, &mut t.pie_legend_text_color] {
            *f = hex(c);
        }
    }
    t
}

fn size(svg: &str) -> Result<(f32, f32), ContentError> {
    let doc = roxmltree::Document::parse(svg).map_err(|e| ContentError::Unsafe(e.to_string()))?;
    let r = doc.root_element();
    let num = |s: &str| s.trim_end_matches("px").parse::<f32>().ok();
    let wh = r.attribute("width").and_then(num).zip(r.attribute("height").and_then(num));
    let vb = r.attribute("viewBox").and_then(|v| {
        let p: Vec<f32> = v.split(|c: char| c == ' ' || c == ',').filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect();
        (p.len() == 4).then(|| (p[2], p[3]))
    });
    let (w, h) = wh.or(vb).ok_or_else(|| ContentError::Parse("SVG has no size".into()))?;
    if w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0 {
        Ok((w, h))
    } else {
        Err(ContentError::Parse("SVG has an invalid size".into()))
    }
}
