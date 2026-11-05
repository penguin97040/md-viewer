//! Optional merman backend (feature `merman`). Spike code: same inputs and outputs as the
//! mermaid-rs-renderer path in `mermaid.rs`.

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
    let mut vars = format!("\"fontFamily\":\"Chakra Petch\",\"fontSize\":\"{}px\"", opts.font_size);
    let text = match (opts.text_color, opts.theme) {
        (Some(c), _) => Some(c & 0xff_ffff),
        (None, ContentTheme::Print) => Some(0),
        _ => None,
    };
    if let Some(c) = text {
        let c = format!("#{c:06x}");
        vars.push_str(&format!(",\"textColor\":\"{c}\",\"primaryTextColor\":\"{c}\""));
    }
    format!("%%{{init: {{\"theme\":\"{theme}\",\"htmlLabels\":false,\"flowchart\":{{\"htmlLabels\":false}},\"themeVariables\":{{{vars}}}}}}}%%\n")
}

/// Text is measured with the three embedded fonts only; no system font is read.
fn fonts() -> Result<NativeFontContext, ContentError> {
    static CTX: OnceLock<Result<NativeFontContext, String>> = OnceLock::new();
    CTX.get_or_init(|| {
        let bytes = [crate::raster::CHAKRA_REGULAR, crate::raster::CHAKRA_SEMIBOLD, crate::raster::SHARE_TECH_MONO];
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
