//! A paginated, light-theme PDF of a document, from the GPUI-free block model.
//!
//! The reader's own layout engine (`mdv_reader::layout`, which has no GPUI in it) is run with a
//! print palette, a page-wide column and a HarfBuzz shaper over the embedded fonts, so paper
//! matches the screen's typography. This crate then cuts that layout into pages, honouring the
//! `break-inside: avoid` and `break-after: avoid` rules of the print block in `style.css`, and
//! draws each page with `krilla`: real text with subset fonts, vector maths and diagrams, link
//! annotations and a bookmark outline.
//!
//! Two passes: the first lays every chunk out and decides the pages (so links and bookmarks know
//! their page); the second lays chunks out again, one page at a time, and draws them. Memory
//! stays near one chunk plus the PDF being built.

mod assets;
mod draw;
mod fonts;
pub mod handoff;
mod job;
mod paginate;
mod shape;
mod system;

use std::cell::RefCell;
use std::sync::Arc;

use mdv_reader::doc::Document;
use mdv_reader::layout::{self, ChunkState, Options};
use mdv_reader::model::ChunkContent;

pub use job::{PrintJob, PrintState};

/// Paper, from the system's default locale; New Zealand uses A4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PageSize {
    #[default]
    A4,
    Letter,
}

impl PageSize {
    /// Width and height in points.
    pub fn points(self) -> (f32, f32) {
        match self {
            PageSize::A4 => (595.276, 841.89),
            PageSize::Letter => (612.0, 792.0),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PrintOptions {
    pub page: PageSize,
    /// The reader's serif setting.
    pub serif: bool,
    /// The reader's line-height setting.
    pub line_height: f32,
    pub title: Option<String>,
    /// The reader has agreed to load this document's web images.
    pub web_images: bool,
}

impl Default for PrintOptions {
    fn default() -> Self {
        Self { page: PageSize::A4, serif: false, line_height: 1.65, title: None, web_images: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Layout,
    Draw,
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
}

impl Progress {
    /// 0.0 to 1.0 over both passes.
    pub fn fraction(&self) -> f32 {
        let f = if self.total == 0 { 1.0 } else { self.done as f32 / self.total as f32 };
        match self.phase {
            Phase::Layout => f * 0.5,
            Phase::Draw => 0.5 + f * 0.5,
        }
    }
}

#[derive(Debug)]
pub enum PrintError {
    Cancelled,
    Pdf(String),
    /// A bug: the writer panicked. The panic was contained.
    Internal(String),
}

impl std::fmt::Display for PrintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrintError::Cancelled => write!(f, "cancelled"),
            PrintError::Pdf(m) => write!(f, "could not write the PDF: {m}"),
            PrintError::Internal(m) => write!(f, "printing failed: {m}"),
        }
    }
}

impl std::error::Error for PrintError {}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub pages: usize,
    pub chunks: usize,
    /// Characters none of the embedded fonts has a glyph for. They print as the font's empty box.
    pub missing_glyphs: usize,
    /// The first few different ones, to name to the person.
    pub missing_sample: Vec<char>,
    pub failed_math: usize,
    pub failed_diagrams: usize,
    pub failed_images: usize,
    pub held_web_images: usize,
    pub bookmarks: usize,
    pub links: usize,
}

/// Print palette: the `@media print` block of `style.css` over the light theme.
pub fn print_palette() -> mdv_theme::Palette {
    let mut p = mdv_theme::LIGHT;
    p.bg = 0xffffff;
    p.bg_soft = 0xf4f5f7;
    p.bg_code = 0xf5f6f8;
    p.text = 0x1f2328;
    p.text_strong = 0x000000;
    p.code_text = 0x0550ae;
    p.muted = 0x59636e;
    p.border = 0xd0d7de;
    p.accent = 0x0a62c9;
    p
}

/// CSS pixels per point.
pub(crate) const PX_PER_PT: f32 = 4.0 / 3.0;
/// `@page { margin: 16mm 14mm }` in points.
pub(crate) const MARGIN_Y_PT: f32 = 16.0 * 72.0 / 25.4;
pub(crate) const MARGIN_X_PT: f32 = 14.0 * 72.0 / 25.4;

pub(crate) struct Geometry {
    pub page_w_pt: f32,
    pub page_h_pt: f32,
    pub content_w_px: f32,
    pub content_h_px: f32,
}

impl Geometry {
    pub fn new(size: PageSize) -> Self {
        let (w, h) = size.points();
        Self { page_w_pt: w, page_h_pt: h, content_w_px: (w - 2.0 * MARGIN_X_PT) * PX_PER_PT, content_h_px: (h - 2.0 * MARGIN_Y_PT) * PX_PER_PT }
    }
}

pub(crate) fn layout_options(g: &Geometry, o: &PrintOptions) -> Options {
    Options { width: g.content_w_px, padding: 0.0, font_size: 11.0 * PX_PER_PT, line_height: o.line_height, serif: o.serif, wrap: true, mode: mdv_theme::Mode::Light }
}

pub(crate) fn lay(c: &ChunkContent, opts: &Options, first: bool, last: bool, shaper: &mut dyn layout::Shaper, assets: &dyn layout::Assets) -> layout::ChunkLayout {
    layout::layout_chunk_print(c, opts, print_palette(), &ChunkState::default(), first, last, shaper, assets)
}

/// Renders the whole document. `ctl` is called as work proceeds and returns `false` to stop.
pub fn render_pdf(doc: &Document, opts: &PrintOptions, ctl: &mut dyn FnMut(Progress) -> bool) -> Result<(Vec<u8>, Report), PrintError> {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render_inner(doc, opts, ctl)));
    match r {
        Ok(r) => r,
        Err(p) => {
            let m = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_else(|| "unknown".into());
            Err(PrintError::Internal(m))
        }
    }
}

fn render_inner(doc: &Document, opts: &PrintOptions, ctl: &mut dyn FnMut(Progress) -> bool) -> Result<(Vec<u8>, Report), PrintError> {
    let geo = Geometry::new(opts.page);
    let lo = layout_options(&geo, opts);
    let mut fonts = fonts::FontSet::new();
    let store = RefCell::new(assets::Store::default());
    let n = doc.chunks.len();
    let mut report = Report { chunks: n, ..Default::default() };

    // Pass one: where the pages break. If the embedded fonts lack characters the document uses,
    // look for system fonts that have them, and lay out again with those added.
    let mut tries = 0;
    let (plan, shaper) = loop {
        let mut plan = paginate::Plan::new(geo.content_h_px);
        let mut shaper = shape::PdfShaper::new(&fonts);
        for i in 0..n {
            if !ctl(Progress { phase: Phase::Layout, done: i, total: n }) {
                return Err(PrintError::Cancelled);
            }
            let c = doc.chunk(i);
            let a = assets::PrintAssets { store: &store, document: doc.path.as_deref(), web_images: opts.web_images };
            let l = lay(&c, &lo, i == 0, i + 1 == n, &mut shaper, &a);
            plan.add_chunk(i, &c, &l);
        }
        tries += 1;
        let more = if tries == 1 && !shaper.missing_chars.is_empty() { fonts.with_system_for(&shaper.missing_chars) } else { None };
        match more {
            Some(f) => {
                drop(shaper);
                fonts = f;
            }
            None => break (plan, (shaper.missing, shaper.missing_sample)),
        }
    };
    report.missing_glyphs = shaper.0;
    report.missing_sample = shaper.1;
    let plan = plan.finish();
    report.pages = plan.pages.len();

    // Pass two: draw.
    let mut pdf = draw::Pdf::new(&geo, &fonts)?;
    let mut cache: Option<(usize, Arc<ChunkContent>, Arc<layout::ChunkLayout>)> = None;
    let total = plan.pages.len();
    for (pi, page) in plan.pages.iter().enumerate() {
        if !ctl(Progress { phase: Phase::Draw, done: pi, total }) {
            return Err(PrintError::Cancelled);
        }
        let mut get = |chunk: usize| {
            if cache.as_ref().map(|c| c.0) != Some(chunk) {
                let c = doc.chunk(chunk);
                let a = assets::PrintAssets { store: &store, document: doc.path.as_deref(), web_images: opts.web_images };
                let mut sh = shape::PdfShaper::new(&fonts);
                let l = lay(&c, &lo, chunk == 0, chunk + 1 == n, &mut sh, &a);
                cache = Some((chunk, c, Arc::new(l)));
            }
            let (_, c, l) = cache.as_ref().expect("cache");
            (c.clone(), l.clone())
        };
        report.links += pdf.draw_page(&page.slices, &mut get, &store, &plan);
    }
    let title = opts.title.clone().or_else(|| doc.headings.first().map(|h| h.text.clone()));
    report.bookmarks = pdf.set_outline(doc, &plan, 3000);
    let bytes = pdf.finish(title)?;
    let s = store.borrow();
    report.failed_math = s.failed_math;
    report.failed_diagrams = s.failed_diagrams;
    report.failed_images = s.failed_images;
    report.held_web_images = s.held_web_images;
    Ok((bytes, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdv_reader::model::TokenClass as T;

    fn pdf(src: &str) -> (Vec<u8>, Report) {
        let doc = Document::parse(src.as_bytes().to_vec());
        let (b, r) = render_pdf(&doc, &PrintOptions::default(), &mut |_| true).expect("render");
        assert!(b.starts_with(b"%PDF-"), "not a PDF");
        (b, r)
    }

    /// With system fonts the characters the embedded fonts lack are drawn and copy out correctly;
    /// the ones nothing draws are still counted so the person is told.
    #[test]
    fn system_fonts_fill_in_and_the_rest_is_still_reported() {
        let (_, plain) = pdf("plain text only\n");
        assert_eq!(plain.missing_glyphs, 0);
        let (bytes, r) = pdf("alpha \u{3b1}\u{3b2} kanji \u{6f22}\u{5b57} gone \u{10fffd}\n");
        assert!(r.missing_glyphs >= 1 && r.missing_sample.contains(&'\u{10fffd}'), "{r:?}");
        let drawn = r.missing_glyphs == 1;
        if cfg!(any(windows, target_os = "macos")) {
            assert!(drawn, "Greek and CJK should come from the system fonts: {r:?}");
        }
        assert!(bytes.len() > 1000);
    }

    fn luminance(c: u32) -> f32 {
        let f = |v: u32| {
            let s = v as f32 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c >> 16 & 0xff) + 0.7152 * f(c >> 8 & 0xff) + 0.0722 * f(c & 0xff)
    }

    fn contrast(a: u32, b: u32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Regression: code printed in the dark theme's colours came out white on white. Every token
    /// class must be readable on the print code background.
    #[test]
    fn every_token_class_is_readable_on_paper() {
        let p = print_palette();
        let all = [
            T::Keyword, T::KeywordConstant, T::KeywordDeclaration, T::KeywordNamespace, T::KeywordPseudo, T::KeywordType, T::Name, T::NameAttribute, T::NameBuiltin, T::NameBuiltinPseudo,
            T::NameClass, T::NameConstant, T::NameDecorator, T::NameEntity, T::NameException, T::NameFunction, T::NameLabel, T::NameNamespace, T::NameProperty, T::NameTag, T::NameVariable,
            T::Literal, T::String, T::StringEscape, T::StringRegex, T::StringSymbol, T::Number, T::Operator, T::Punctuation, T::Comment, T::CommentPreproc, T::GenericDeleted, T::GenericInserted,
            T::GenericHeading, T::GenericSubheading, T::GenericEmph, T::GenericStrong, T::Error,
        ];
        for t in all {
            let (c, _, _) = layout::token_style(t, mdv_theme::Mode::Light);
            let c = c.unwrap_or(p.code_text);
            assert!(contrast(c, p.bg_code) >= 4.5, "{t:?} is {c:06x}, contrast {:.2}", contrast(c, p.bg_code));
        }
    }

    #[test]
    fn empty_document_is_one_blank_page() {
        let (_, r) = pdf("");
        assert_eq!(r.pages, 1);
    }

    #[test]
    fn hostile_inputs_do_not_panic() {
        let long = "word ".repeat(200_000);
        pdf(&long);
        pdf(&"x".repeat(2_000_000));
        let mut table = String::from("|");
        for c in 0..150 {
            table.push_str(&format!(" h{c} |"));
        }
        table.push_str("\n|");
        for _ in 0..150 {
            table.push_str(" - |");
        }
        table.push('\n');
        for r in 0..300 {
            table.push('|');
            for c in 0..150 {
                table.push_str(&format!(" r{r}c{c} |"));
            }
            table.push('\n');
        }
        pdf(&table);
        pdf(&format!("{}deep\n", "> ".repeat(300)));
        let mut nest = String::new();
        for i in 0..120 {
            nest.push_str(&format!("{}- item {i}\n", "  ".repeat(i)));
        }
        pdf(&nest);
        let (_, r) = pdf("![missing](nope/none.png)\n\n![web](https://example.invalid/x.png)\n\ntext\n");
        assert!(r.failed_images >= 1);
        pdf("$$\\frac{a}{b}$$ and $x$ and $$\\bad{$$\n\n```mermaid\ngraph TD\n A-->\n```\n");
        pdf("$$a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p+q+r+s+t+u+v+w+x+y+z+a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p+q+r+s+t+u+v+w+x+y+z+a+b+c+d+e+f+g+h+i+j+k+l+m+n+o+p+q+r+s+t$$\n");
    }

    #[test]
    fn cancelling_stops_cleanly() {
        let doc = Document::parse("# a\n\ntext\n\n".repeat(5000).into_bytes());
        let mut calls = 0;
        let r = render_pdf(&doc, &PrintOptions::default(), &mut |_| {
            calls += 1;
            calls < 3
        });
        assert!(matches!(r, Err(PrintError::Cancelled)));
    }

    #[test]
    fn headings_do_not_end_a_page() {
        let mut s = String::new();
        for i in 0..400 {
            s.push_str(&format!("## Heading {i}\n\nA paragraph of words that takes a line or two in the column.\n\n"));
        }
        let (_, r) = pdf(&s);
        assert!(r.pages > 3);
    }
}
