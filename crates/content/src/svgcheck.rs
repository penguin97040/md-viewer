//! Allow-list check applied to every SVG this crate returns.
//!
//! Rejects (rather than rewrites) anything that could run script or fetch a resource: unknown
//! elements, `on*` attributes, non-fragment links, CSS `url()` to anything but a fragment,
//! `@import`/`@font-face`, DTDs and foreign namespaces. Mermaid.js needs DOMPurify for the same
//! job because its labels are HTML; this renderer emits plain SVG text, so a strict allow-list is
//! enough, and it is cheap enough to run on every asset.

use crate::ContentError;

const SVG_NS: &str = "http://www.w3.org/2000/svg";

const ELEMENTS: &[&str] = &[
    "svg", "g", "defs", "path", "rect", "circle", "ellipse", "line", "polyline", "polygon", "text", "tspan",
    "title", "desc", "marker", "linearGradient", "radialGradient", "stop", "clipPath", "mask", "pattern",
    "symbol", "use", "style", "image", "filter", "feGaussianBlur", "feDropShadow", "feOffset", "feMerge",
    "feMergeNode", "feFlood", "feComposite", "feColorMatrix",
];

const MAX_NODES: u32 = 500_000;

pub fn check(svg: &str) -> Result<(), ContentError> {
    let bad = |m: String| Err(ContentError::Unsafe(m));
    let lower = svg.to_ascii_lowercase();
    if lower.contains("<!doctype") || lower.contains("<!entity") {
        return bad("DTD or entity declaration".into());
    }
    let opt = roxmltree::ParsingOptions { allow_dtd: false, nodes_limit: MAX_NODES, ..Default::default() };
    let doc = roxmltree::Document::parse_with_options(svg, opt).map_err(|e| ContentError::Unsafe(format!("not well-formed: {e}")))?;
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return bad("root is not <svg>".into());
    }
    for n in doc.descendants().filter(|n| n.is_element()) {
        let name = n.tag_name().name();
        if n.tag_name().namespace().unwrap_or(SVG_NS) != SVG_NS {
            return bad(format!("foreign namespace on <{name}>"));
        }
        if !ELEMENTS.contains(&name) {
            return bad(format!("element <{name}>"));
        }
        for a in n.attributes() {
            let an = a.name();
            let av = a.value();
            if an.len() >= 2 && an[..2].eq_ignore_ascii_case("on") {
                return bad(format!("attribute {an}"));
            }
            match an {
                "href" => {
                    let ok = av.starts_with('#')
                        || (name == "image" && (av.starts_with("data:image/png;base64,") || av.starts_with("data:image/jpeg;base64,")));
                    if !ok {
                        return bad(format!("link on <{name}>"));
                    }
                }
                "style" => css(av)?,
                _ => {
                    if av.contains("url(") {
                        css(av)?;
                    }
                }
            }
        }
        if name == "style" {
            css(&n.descendants().filter(|c| c.is_text()).filter_map(|c| c.text()).collect::<String>())?;
        }
    }
    Ok(())
}

fn css(s: &str) -> Result<(), ContentError> {
    let l = s.to_ascii_lowercase();
    if l.contains('\\') || l.contains("@import") || l.contains("@font-face") || l.contains("expression(") || l.contains("javascript:") || l.contains("behavior") || l.contains("-moz-binding") {
        return Err(ContentError::Unsafe("CSS escape, at-rule or script".into()));
    }
    let mut rest = l.as_str();
    while let Some(i) = rest.find("url(") {
        let after = rest[i + 4..].trim_start().trim_start_matches(['"', '\'']);
        if !after.starts_with('#') {
            return Err(ContentError::Unsafe("CSS url() to a non-fragment".into()));
        }
        rest = &rest[i + 4..];
    }
    Ok(())
}
