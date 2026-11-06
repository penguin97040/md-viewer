//! Embedded assets: fonts, the app icon, licence texts and the SVG icons.
//!
//! The icon path data is copied verbatim from the 0.2 web interface. GPUI renders SVGs as an alpha mask tinted with the element's text colour, so
//! the documents use a black stroke and `text_color` supplies the CSS `currentColor`.

use anyhow::Result;
use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub const GEIST_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Geist-Regular.ttf");
pub const GEIST_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Geist-SemiBold.ttf");
pub const GEIST_MONO_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/GeistMono-Regular.ttf");
pub const APP_ICON: &[u8] = include_bytes!("../../../assets/appicon.png");
pub const OFL_GEIST: &str = include_str!("../../../assets/fonts/OFL-Geist.txt");

/// (asset path, inner SVG markup, stroke width in viewBox units)
const ICONS: &[(&str, &str, f32)] = &[
    ("toc", r#"<path d="M4 6h16M4 12h10M4 18h13"/>"#, 1.7),
    ("open", r#"<path d="M3 7.5V18a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2V9.5a2 2 0 0 0-2-2h-7l-2-2.5H5a2 2 0 0 0-2 2z"/>"#, 1.7),
    ("reload", r#"<path d="M20 11a8 8 0 1 0-2.3 5.7M20 4v7h-7"/>"#, 1.7),
    ("find", r#"<circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/>"#, 1.7),
    ("print", r#"<path d="M7 9V4h10v5M7 17H5a2 2 0 0 1-2-2v-4a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v4a2 2 0 0 1-2 2h-2"/><path d="M7 14h10v6H7z"/>"#, 1.7),
    ("settings", r#"<path d="M4 7h10M18 7h2M4 17h4M12 17h8"/><circle cx="16" cy="7" r="2"/><circle cx="10" cy="17" r="2"/>"#, 1.7),
    ("tools", r#"<path d="M4 6h16M4 12h16M4 18h16"/>"#, 1.7),
    ("minimise", r#"<path d="M5 12h14"/>"#, 1.7),
    ("maximise", r#"<path d="M6 6h12v12H6z"/>"#, 1.7),
    ("restore", r#"<path d="M5 9h10v10H5zM9 9V5h10v10h-4"/>"#, 1.7),
    ("close", r#"<path d="m6 6 12 12M18 6 6 18"/>"#, 1.7),
    ("prev", r#"<path d="m6 15 6-6 6 6"/>"#, 1.7),
    ("next", r#"<path d="m6 9 6 6 6-6"/>"#, 1.7),
    ("dismiss", r#"<path d="M6 6l12 12M18 6 6 18"/>"#, 1.7),
    // .tab .x svg: stroke-width 2 (style.css line 150)
    ("tab-x", r#"<path d="M7 7l10 10M17 7 7 17"/>"#, 2.0),
    // the checkbox tick drawn on the accent fill (the browser draws its own)
    ("check", r#"<path d="m5 12.5 4.5 4.5L19 7.5"/>"#, 3.0),
];

const NZ_STARS: &str = r#"<polygon points="12.0,0.6 12.8,2.9 15.2,2.9 13.2,4.4 14.0,6.8 12.0,5.3 10.0,6.8 10.8,4.4 8.8,2.9 11.2,2.9"/>
<polygon points="4.5,10.1 5.3,12.4 7.7,12.4 5.7,13.9 6.5,16.3 4.5,14.8 2.5,16.3 3.3,13.9 1.3,12.4 3.7,12.4"/>
<polygon points="19.0,8.8 19.6,10.7 21.6,10.7 20.0,11.8 20.6,13.7 19.0,12.5 17.4,13.7 18.0,11.8 16.4,10.7 18.4,10.7"/>
<polygon points="12.0,21.7 12.9,24.3 15.6,24.3 13.4,25.9 14.2,28.6 12.0,27.0 9.8,28.6 10.6,25.9 8.4,24.3 11.1,24.3"/>"#;

pub struct Assets;

impl Assets {
    pub fn icon(name: &str) -> SharedString {
        format!("icons/{name}.svg").into()
    }
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let Some(name) = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg")) else {
            return Ok(None);
        };
        if name == "nz" {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 30" fill="#000">{NZ_STARS}</svg>"##
            );
            return Ok(Some(Cow::Owned(svg.into_bytes())));
        }
        Ok(ICONS.iter().find(|(n, _, _)| *n == name).map(|(_, inner, stroke)| {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="{stroke}" stroke-linecap="round" stroke-linejoin="round">{inner}</svg>"##
            );
            Cow::Owned(svg.into_bytes())
        }))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
