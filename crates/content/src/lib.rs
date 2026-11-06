//! Maths and diagrams as self-contained SVG, with no browser and no GPUI types.
//!
//! * [`render_math`] typesets LaTeX with RaTeX (KaTeX-compatible), emitting glyph outlines as
//!   `<path>` elements so the SVG needs no fonts, CSS or web fonts.
//! * [`render_mermaid`] draws Mermaid text with merman, then checks the SVG against an allow-list
//!   ([`svgcheck`]) before returning it.
//! * [`rasterise`] turns either asset into RGBA pixels with resvg, using only fonts embedded in
//!   this crate (never system fonts), so output is identical on every machine.
//!
//! Every entry point returns an error instead of panicking, runs on a worker thread with a time
//! budget, and leaves later calls working.

use std::fmt;
use std::sync::Arc;

mod guard;
mod math;
mod merman_backend;
mod mermaid;
mod raster;
pub mod svgcheck;

pub use guard::{new_render_owner, set_render_owner};
pub use raster::{boost_light_on_dark, rasterise, render_svg_image, RgbaImage, SvgImage};

/// Colour scheme of a rendered asset. `Print` is the light scheme with pure-black text and no
/// background, as v0.2 printed diagrams.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ContentTheme {
    #[default]
    Dark,
    Light,
    Print,
}

impl ContentTheme {
    /// Default text colour (0xRRGGBB) for the scheme: the app's `--text`.
    pub fn text_color(self) -> u32 {
        match self {
            ContentTheme::Dark => mdv_theme::DARK.text,
            ContentTheme::Light => mdv_theme::LIGHT.text,
            ContentTheme::Print => 0x000000,
        }
    }
}

/// A rendered asset: an SVG plus the size it should occupy in logical pixels.
#[derive(Clone, Debug)]
pub struct RenderedAsset {
    pub width_px: f32,
    pub height_px: f32,
    /// Distance from the top edge to the text baseline. Equal to `height_px` for diagrams.
    pub baseline_px: f32,
    pub svg: Arc<str>,
}

#[derive(Clone, Debug)]
pub struct MathOptions {
    pub theme: ContentTheme,
    /// Size of 1 em in logical pixels. The app uses `.md .katex { font-size: 1.1em }`, so pass
    /// 1.1 x the body font size to match.
    pub font_size: f32,
    /// Overrides the theme's text colour (0xRRGGBB).
    pub text_color: Option<u32>,
}

impl Default for MathOptions {
    fn default() -> Self {
        Self { theme: ContentTheme::Dark, font_size: 16.0 * 1.1, text_color: None }
    }
}

#[derive(Clone, Debug)]
pub struct DiagramOptions {
    pub theme: ContentTheme,
    /// Base font size in logical pixels (Mermaid's default is 16).
    pub font_size: f32,
    pub text_color: Option<u32>,
}

impl Default for DiagramOptions {
    fn default() -> Self {
        Self { theme: ContentTheme::Dark, font_size: 16.0, text_color: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentError {
    /// The source text is not valid maths or Mermaid.
    Parse(String),
    /// The source or the result exceeds a limit (see [`limits`]).
    TooLarge(String),
    /// Rendering did not finish inside the time budget.
    Timeout,
    /// Too many earlier renders are still running. Worth asking again later, unlike the others.
    Busy,
    /// The renderer panicked; the panic was contained.
    Panic(String),
    /// The generated SVG failed the safety check.
    Unsafe(String),
    /// Rasterisation failed.
    Raster(String),
}

impl fmt::Display for ContentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentError::Parse(m) => write!(f, "{m}"),
            ContentError::TooLarge(m) => write!(f, "too large: {m}"),
            ContentError::Timeout => write!(f, "took too long to render"),
            ContentError::Busy => write!(f, "too many earlier renders are still running"),
            ContentError::Panic(m) => write!(f, "renderer failed: {m}"),
            ContentError::Unsafe(m) => write!(f, "unsafe SVG rejected: {m}"),
            ContentError::Raster(m) => write!(f, "could not rasterise: {m}"),
        }
    }
}

impl std::error::Error for ContentError {}

/// Limits added by this crate. Values were chosen from measurements.
pub mod limits {
    use std::time::Duration;
    /// Maths source length in bytes.
    pub const MAX_MATH_BYTES: usize = 20_000;
    /// Mermaid source length in bytes; Mermaid.js `maxTextSize` default is 50 000.
    pub const MAX_MERMAID_BYTES: usize = 50_000;
    /// Statements (non-blank lines plus `;` separators) for most diagram types. Measured: sequence
    /// diagrams of 1 200 messages took 2.5 s, so 1 000 keeps the worst case near the budget.
    pub const MAX_MERMAID_STATEMENTS: usize = 1_000;
    /// Statements for state, class and ER diagrams (800 took 1.7 to 3.2 s on a loaded machine).
    pub const MAX_GRAPH_STATEMENTS: usize = 500;
    /// Edge operators in a flowchart. Mermaid.js `maxEdges` is 500, but this renderer's flowchart
    /// layout needed 6.8 s for 200 fan-out edges and 36 s for 500.
    pub const MAX_FLOWCHART_EDGES: usize = 150;
    /// Largest accepted maths box, in em.
    pub const MAX_MATH_EM: f64 = 300.0;
    /// Largest accepted rule or bar, in em: the app runs KaTeX with `maxSize: 50`, which clamps.
    /// This crate rejects instead of clamping.
    pub const MAX_MATH_RULE_EM: f64 = 50.0;
    /// Largest SVG string from one formula. Glyphs are inlined as paths (about 2.6 kB each), so a
    /// 60 x 60 matrix was 3.3 MB.
    pub const MAX_SVG_BYTES: usize = 6 * 1024 * 1024;
    /// Largest accepted diagram side in logical pixels.
    pub const MAX_DIAGRAM_PX: f32 = 20_000.0;
    /// Largest raster side in pixels.
    pub const MAX_RASTER_PX: u32 = 16_384;
    /// Pixels in one raster. Allocation failure ends the process, so this comes before the memory:
    /// a page's worth of side on each axis would otherwise be 1 GB.
    pub const MAX_RASTER_PIXELS: u64 = 100_000_000;
    /// The largest SVG image file: the same as for any other image.
    pub const MAX_SVG_IMAGE_BYTES: usize = 25 << 20;
    /// Pixels in the raster of one SVG image. A larger one is drawn more coarsely, not refused.
    pub const MAX_SVG_IMAGE_PIXELS: f32 = 24_000_000.0;
    pub const MATH_BUDGET: Duration = Duration::from_millis(2_000);
    pub const MERMAID_BUDGET: Duration = Duration::from_millis(5_000);
    /// Threads rendering diagrams and SVG images at once, across all documents, including any
    /// still running after their budget expired. They can't be stopped from outside.
    pub const MAX_DIAGRAM_THREADS: usize = 16;
    /// The same for maths, which is quick and has a smaller budget.
    pub const MAX_MATH_THREADS: usize = 8;
    /// Threads past their budget that one document can hold of each kind.
    pub const MAX_STUCK_PER_DOCUMENT: usize = 6;
    /// Worker thread stack size. RaTeX has its own depth budget; this is a second line of defence.
    pub const WORKER_STACK: usize = 64 * 1024 * 1024;
}

/// Typeset LaTeX. `display` selects display style; the result is not centred.
pub fn render_math(src: &str, display: bool, opts: &MathOptions) -> Result<RenderedAsset, ContentError> {
    math::render(src, display, opts)
}

/// Draw a Mermaid diagram. A leading `%%{init}%%` directive is ignored (see `mermaid.rs`).
pub fn render_mermaid(src: &str, opts: &DiagramOptions) -> Result<RenderedAsset, ContentError> {
    mermaid::render(src, opts)
}

/// Characters XML 1.0 cannot carry (NUL and most C0 controls) are dropped so one stray byte does
/// not make the whole SVG ill-formed.
pub(crate) fn strip_xml_illegal(src: &str) -> String {
    src.chars().filter(|&c| matches!(c, '\t' | '\n' | '\r') || (c >= ' ' && c != '\u{7f}' && c != '\u{fffe}' && c != '\u{ffff}')).collect()
}
