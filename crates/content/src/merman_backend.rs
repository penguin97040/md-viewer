//! Optional merman backend (feature `merman`): same inputs and outputs as the mermaid-rs-renderer
//! path in `mermaid.rs`.

use std::sync::OnceLock;

use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment, SvgRequest};
use merman_export::{NativeFontContext, NativeFontLimits};

use crate::{ContentError, ContentTheme, DiagramOptions};

/// Theme control is the Mermaid `%%{init}%%` directive, written by this crate after the user's own
/// directives have been stripped.
fn init_directive(opts: &DiagramOptions) -> String {
    let theme = match opts.theme {
        ContentTheme::Dark => "dark",
        ContentTheme::Light | ContentTheme::Print => "default",
    };
    let mut vars = format!("\"fontFamily\":\"Geist\",\"fontSize\":\"{}px\"", opts.font_size);
    let text = match (opts.text_color, opts.theme) {
        (Some(c), _) => Some(c & 0xff_ffff),
        (None, ContentTheme::Print) => Some(0),
        _ => None,
    };
    if let Some(c) = text {
        let c = format!("#{c:06x}");
        vars.push_str(&format!(",\"textColor\":\"{c}\",\"primaryTextColor\":\"{c}\""));
    }
    let mut extra = String::new();
    if opts.theme == ContentTheme::Dark {
        vars.push_str(&dark_variables());
        extra = dark_config();
    }
    format!("%%{{init: {{\"theme\":\"{theme}\",{extra}\"htmlLabels\":false,\"flowchart\":{{\"htmlLabels\":false}},\"themeVariables\":{{{vars}}}}}}}%%\n")
}

fn hex(c: u32) -> String {
    format!("#{:06x}", c & 0xff_ffff)
}

/// Quadrant points default to black, which does not show on the dark page.
fn dark_variables() -> String {
    let text = hex(mdv_theme::Mode::Dark.palette().text);
    format!(",\"quadrantPointFill\":\"{text}\",\"quadrantPointTextFill\":\"{text}\"")
}

/// An external system in C4 is white text on a light grey that is hard to read.
fn dark_config() -> String {
    let border = hex(mdv_theme::Mode::Dark.palette().border);
    format!("\"c4\":{{\"external_person_bg_color\":\"#55555b\",\"external_person_border_color\":\"{border}\",\"external_system_bg_color\":\"#55555b\",\"external_system_border_color\":\"{border}\"}},")
}

/// Colours merman keeps for two kinds of diagram whatever the theme says: black byte numbers and
/// light blocks for a packet, and dark grey relationships for C4. Each is an exact string of its
/// output, so a change in merman leaves the diagram as it was, and a test notices.
fn repaint_for_dark(svg: String) -> String {
    let p = mdv_theme::Mode::Dark.palette();
    let (text, fill, border) = (hex(p.text), hex(p.bg_hover), hex(p.border));
    let head = &svg[..svg.len().min(512)];
    if head.contains("aria-roledescription=\"packet\"") {
        svg.replace(".packetByte.start{fill:black;}", &format!(".packetByte.start{{fill:{text};}}"))
            .replace(".packetByte.end{fill:black;}", &format!(".packetByte.end{{fill:{text};}}"))
            .replace(".packetLabel{fill:black;", &format!(".packetLabel{{fill:{text};"))
            .replace(".packetTitle{fill:black;", &format!(".packetTitle{{fill:{text};"))
            .replace(".packetBlock{stroke:black;stroke-width:1;fill:#efefef;}", &format!(".packetBlock{{stroke:{border};stroke-width:1;fill:{fill};}}"))
    } else if head.contains("aria-roledescription=\"c4\"") {
        svg.replace("fill=\"#444444\"", &format!("fill=\"{text}\""))
            .replace("stroke=\"#444444\"", &format!("stroke=\"{text}\""))
            .replace("<path fill=\"black\" stroke=\"#000000\"", &format!("<path fill=\"{text}\" stroke=\"{text}\""))
    } else {
        svg
    }
}

/// Text is measured with the three embedded fonts only; no system font is read.
fn fonts() -> Result<NativeFontContext, ContentError> {
    static CTX: OnceLock<Result<NativeFontContext, String>> = OnceLock::new();
    CTX.get_or_init(|| {
        let bytes = [crate::raster::GEIST_REGULAR, crate::raster::GEIST_SEMIBOLD, crate::raster::GEIST_MONO];
        NativeFontContext::from_font_bytes(bytes.iter().map(|b| b.to_vec()), NativeFontLimits::default(), &OperationControl::new()).map_err(|e| e.to_string())
    })
    .clone()
    .map_err(ContentError::Parse)
}

pub(crate) fn render_svg(src: &str, opts: &DiagramOptions, card: Option<u32>) -> Result<String, ContentError> {
    let full = format!("{}{}", init_directive(opts), src);
    let mut req = SvgRequest::default();
    req.environment = SvgEnvironment::deterministic().with_native_font_context(fonts()?).map_err(|e| ContentError::Parse(e.to_string()))?;
    let out = Renderer::new()
        .render(RenderRequest::document(&full, OperationControl::new(), req))
        .map_err(|e| ContentError::Parse(e.to_string()))?;
    match out {
        RenderOutput::Document(Some(d)) => Ok(match card {
            Some(c) => with_card(d.svg(), c),
            None if opts.theme == ContentTheme::Dark => repaint_for_dark(d.svg().to_string()),
            None => d.svg().to_string(),
        }),
        _ => Err(ContentError::Parse("no diagram".into())),
    }
}

/// Kinds that hard-code light colours (see `hard_codes_light_colours`) are drawn light on a card:
/// a rectangle covering the viewBox is inserted as the first child of the root element.
fn with_card(svg: &str, rgb: u32) -> String {
    let Some(end) = svg.find('>') else { return svg.to_string() };
    let root = &svg[..end];
    let vb: Vec<f64> = root
        .split("viewBox=\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .map(|v| v.split(|c: char| c == ' ' || c == ',').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_default();
    if vb.len() != 4 || root.ends_with('/') {
        return svg.to_string();
    }
    format!("{}><rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" fill=\"#{:06x}\"/>{}", root, vb[0], vb[1], vb[2], vb[3], rgb & 0xff_ffff, &svg[end + 1..])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dark(src: &str) -> String {
        let opts = DiagramOptions { theme: ContentTheme::Dark, ..Default::default() };
        render_svg(src, &opts, None).expect("diagram")
    }

    #[test]
    fn packet_is_themed_in_dark_mode() {
        let svg = dark("packet-beta\n  0-15: \"Source\"\n  16-31: \"Destination\"\n");
        assert!(!svg.contains("fill:#efefef") && !svg.contains(".packetByte.start{fill:black"), "{}", &svg[..800.min(svg.len())]);
    }

    #[test]
    fn c4_relationships_are_themed_in_dark_mode() {
        let svg = dark("C4Context\n  Person(a, \"A\", \"x\")\n  System(b, \"B\", \"y\")\n  Rel(a, b, \"Uses\")\n");
        assert!(!svg.contains("#444444"));
    }

    #[test]
    fn quadrant_points_show_in_dark_mode() {
        let svg = dark("quadrantChart\n  title T\n  quadrant-1 A\n  Point: [0.3, 0.6]\n");
        let fill = hex(mdv_theme::Mode::Dark.palette().text);
        assert!(svg.contains(&fill));
    }
}
