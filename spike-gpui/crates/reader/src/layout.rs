//! Lays out one chunk of the model into positioned boxes and glyphs: the reader's CSS, by hand.
//!
//! GPUI's text elements take one font size per shaped line and no inline padding or borders, so
//! they cannot draw `.md code`, `kbd`, `sub` or `sup`. This module shapes style runs through the
//! [`Shaper`] trait and does its own line breaking, margin collapsing and table sizing, following
//! `frontend/style.css` lines 389-546. It has no GPUI types, so it can run on any thread and be
//! tested without a window. Positions are relative to the top of the chunk.

use std::collections::HashMap;

use mdv_theme::{Mode, Palette};
use unicode_segmentation::UnicodeSegmentation;

use crate::model::*;
use crate::sort::{self, SortDir};

// ---- shaping ----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    Sans,
    Serif,
    Mono,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontSpec {
    pub family: Family,
    pub bold: bool,
    pub italic: bool,
}

/// One glyph as shaped: `x` from the start of the run, `byte` an offset into the shaped string.
#[derive(Clone, Copy, Debug)]
pub struct RawGlyph {
    pub x: f32,
    pub byte: u32,
    pub id: u32,
    pub font: u32,
    pub emoji: bool,
}

/// What the layout needs from a text system.
pub trait Shaper {
    /// Appends the glyphs of `text` and returns its advance width.
    fn shape(&mut self, text: &str, font: FontSpec, size: f32, out: &mut Vec<RawGlyph>) -> f32;
    /// (ascent, descent), both positive, rounded to whole pixels as browsers do.
    fn metrics(&mut self, font: FontSpec, size: f32) -> (f32, f32);
    /// The font has no such face, so bold must be synthesised when painting.
    fn fake_bold(&self, _font: FontSpec) -> bool {
        false
    }
}

// ---- options and view state -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// The pane's width, excluding its scrollbar.
    pub width: f32,
    pub padding: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub serif: bool,
    pub wrap: bool,
    pub mode: Mode,
}

impl Options {
    pub fn palette(&self) -> &'static Palette {
        self.mode.palette()
    }
}

/// Reader state that changes a chunk's layout. Keys are a block's flat text start.
#[derive(Clone, Debug, Default)]
pub struct ChunkState {
    pub details: HashMap<u32, bool>,
    pub sorts: HashMap<u32, (usize, SortDir)>,
    /// A code block showing the done state on its copy button.
    pub copied: Option<u32>,
}

/// An asset's reserved box, in CSS px. `baseline` is measured down from its top.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AssetBox {
    pub width: f32,
    pub height: f32,
    pub baseline: f32,
    pub key: u64,
}

/// What is known about an image so far.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageState {
    /// Being read: nothing is known about its size.
    Pending,
    Ready { width: f32, height: f32, key: u64 },
    /// Refused or unreadable. Its alt text is shown instead.
    Failed,
}

/// Sizes of things that arrive asynchronously. The content crate will answer for maths and
/// diagrams; until it does, or until a given asset is ready, the reader shows a placeholder.
pub trait Assets {
    fn math(&self, _src: &str, _display: bool, _font_size: f32) -> Option<AssetBox> {
        None
    }
    fn mermaid(&self, _src: &str) -> Option<AssetBox> {
        None
    }
    fn image(&self, _source: &ImageSource) -> ImageState {
        ImageState::Failed
    }
}

pub struct NoAssets;
impl Assets for NoAssets {}

// ---- output -----------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Border {
    pub color: u32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
    pub dashed: bool,
}

impl Border {
    pub fn all(color: u32, w: f32) -> Self {
        Self { color, top: w, right: w, bottom: w, left: w, dashed: false }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RectItem {
    pub rect: Rect,
    pub fill: Option<u32>,
    pub border: Option<Border>,
    pub radius: f32,
}

/// A filled polygon, for the check mark and disclosure triangle.
#[derive(Clone, Debug)]
pub struct Poly {
    pub points: Vec<(f32, f32)>,
    pub color: u32,
}

pub const FLAG_SPACE: u8 = 1;
pub const FLAG_EMOJI: u8 = 2;
pub const FLAG_ATOM: u8 = 4;
pub const FLAG_FAKE_BOLD: u8 = 8;

/// A glyph in a text box. `x0` and `x1` are along the box's unwrapped text.
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub x0: f32,
    pub x1: f32,
    /// Offset into the chunk's flat text.
    pub byte: u32,
    pub id: u32,
    pub font: u32,
    pub span: u32,
    pub flags: u8,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SpanInfo {
    pub color: u32,
    pub size: f32,
    /// Added to the baseline: positive is down.
    pub shift: f32,
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
    pub underline: bool,
    pub strike: bool,
    pub link: Option<u32>,
    pub decor: Decor,
    pub pad_l: f32,
    pub pad_r: f32,
    /// Selection and find do not apply to this text (a label, or a marker).
    pub chrome: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Decor {
    #[default]
    None,
    Code,
    Kbd,
    Mark,
    HeldImage,
    MathPlaceholder,
}

#[derive(Clone, Copy, Debug)]
pub struct Line {
    pub x: f32,
    /// Subtract from a glyph's `x0` and add `x` to get its position on this line.
    pub x_shift: f32,
    pub top: f32,
    pub height: f32,
    pub baseline: f32,
    pub g0: u32,
    pub g1: u32,
    pub byte0: u32,
    pub byte1: u32,
    pub width: f32,
}

/// Something placed in a line that is not text: an image or a rendered asset.
#[derive(Clone, Debug)]
pub enum AtomDraw {
    Image { key: u64, w: f32, h: f32 },
    Asset { key: u64, w: f32, h: f32 },
    /// A held web image with no alt text.
    EmptyBox { w: f32, h: f32 },
}

#[derive(Clone, Debug, Default)]
pub struct TextBox {
    pub lines: Vec<Line>,
    pub glyphs: Vec<Glyph>,
    pub spans: Vec<SpanInfo>,
    pub atoms: Vec<AtomDraw>,
    pub byte0: u32,
    pub byte1: u32,
    pub rect: Rect,
    pub selectable: bool,
    /// Find highlights and selection use whole lines here, as `<pre>` text does.
    pub code: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Layer {
    pub rects: Vec<RectItem>,
    pub polys: Vec<Poly>,
    pub boxes: Vec<TextBox>,
}

/// A horizontally scrolling block. Layer 0 is the chunk itself; `areas[i]` draws layer `i + 1`.
#[derive(Clone, Copy, Debug)]
pub struct Area {
    pub key: u32,
    pub view: Rect,
    pub content_w: f32,
    pub scrollbar_y: f32,
}

#[derive(Clone, Debug)]
pub enum HitKind {
    Link(u32),
    Copy { key: u32, text: ByteSpan },
    Sort { key: u32, col: usize },
    Details { key: u32 },
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub rect: Rect,
    pub kind: HitKind,
    /// The layer it is drawn in: a scrolled area moves it.
    pub layer: u32,
    /// For a link: where hover draws its underline.
    pub underline: Option<(f32, u32)>,
}

#[derive(Clone, Debug, Default)]
pub struct ChunkLayout {
    pub height: f32,
    pub layers: Vec<Layer>,
    pub areas: Vec<Area>,
    pub hits: Vec<Hit>,
    pub links: Vec<LinkTarget>,
    pub glyphs: usize,
}

impl ChunkLayout {
    /// The vertical position of a flat text offset, if it is laid out (not in a closed details).
    pub fn y_of(&self, offset: u32) -> Option<f32> {
        self.locate(offset).map(|(_, l)| l.top)
    }

    fn locate(&self, offset: u32) -> Option<(usize, &Line)> {
        let mut best: Option<(usize, &Line)> = None;
        for (li, layer) in self.layers.iter().enumerate() {
            for b in &layer.boxes {
                if !b.selectable || offset < b.byte0 || offset > b.byte1 {
                    continue;
                }
                for l in &b.lines {
                    if l.byte0 <= offset && (offset < l.byte1 || (offset == l.byte1 && offset == b.byte1)) {
                        return Some((li, l));
                    }
                }
                if best.is_none() {
                    best = b.lines.first().map(|l| (li, l));
                }
            }
        }
        // Between boxes: use the next one down.
        best.or_else(|| {
            let mut next: Option<(u32, usize, &Line)> = None;
            for (li, layer) in self.layers.iter().enumerate() {
                for b in layer.boxes.iter().filter(|b| b.selectable && b.byte0 >= offset) {
                    if next.is_none_or(|n| b.byte0 < n.0) {
                        if let Some(l) = b.lines.first() {
                            next = Some((b.byte0, li, l));
                        }
                    }
                }
            }
            next.map(|n| (n.1, n.2))
        })
    }

    /// The first text at or below `y`, for anchoring the reading position.
    pub fn text_at_y(&self, y: f32) -> Option<(u32, f32)> {
        let mut best: Option<(u32, f32)> = None;
        for layer in &self.layers {
            for b in layer.boxes.iter().filter(|b| b.selectable) {
                for l in &b.lines {
                    if l.top + l.height > y && best.is_none_or(|n| l.top < n.1 || (l.top == n.1 && l.byte0 < n.0)) {
                        best = Some((l.byte0, l.top));
                        break;
                    }
                }
            }
        }
        best
    }

    /// The text position under a point, or the nearest one. `scroll` gives an area's offset.
    pub fn hit_test(&self, x: f32, y: f32, scroll: &dyn Fn(u32) -> f32) -> Option<u32> {
        let mut best: Option<(f32, u32)> = None;
        for (li, layer) in self.layers.iter().enumerate() {
            let sx = if li == 0 { 0.0 } else { self.areas.get(li - 1).map_or(0.0, |a| scroll(a.key)) };
            let px = x + sx;
            for b in layer.boxes.iter().filter(|b| b.selectable && !b.lines.is_empty()) {
                let dy = if y < b.rect.y {
                    b.rect.y - y
                } else if y > b.rect.bottom() {
                    y - b.rect.bottom()
                } else {
                    0.0
                };
                let dx = if px < b.rect.x {
                    b.rect.x - px
                } else if px > b.rect.x + b.rect.w {
                    px - (b.rect.x + b.rect.w)
                } else {
                    0.0
                };
                let d = dy * 1000.0 + dx;
                if best.is_some_and(|(bd, _)| bd <= d) {
                    continue;
                }
                let line = b.lines.iter().find(|l| y < l.top + l.height).or(b.lines.last()).expect("lines");
                best = Some((d, b.offset_at(line, px)));
            }
        }
        best.map(|b| b.1)
    }

    /// The hit under a point, topmost last-added first.
    pub fn hit_at(&self, x: f32, y: f32, scroll: &dyn Fn(u32) -> f32) -> Option<&Hit> {
        self.hits.iter().rev().find(|h| {
            let sx = if h.layer == 0 { 0.0 } else { self.areas.get(h.layer as usize - 1).map_or(0.0, |a| scroll(a.key)) };
            if h.layer > 0 && !self.areas[h.layer as usize - 1].view.contains(x, y) {
                return false;
            }
            h.rect.contains(x + sx, y)
        })
    }

    /// The bounds of the text box holding an offset: double and triple click use this.
    pub fn box_at(&self, offset: u32) -> Option<&TextBox> {
        self.layers.iter().flat_map(|l| &l.boxes).find(|b| b.selectable && offset >= b.byte0 && offset <= b.byte1)
    }
}

impl TextBox {
    /// The text offset nearest to `x` on a line.
    pub fn offset_at(&self, line: &Line, x: f32) -> u32 {
        let lx = x - line.x + line.x_shift;
        for g in &self.glyphs[line.g0 as usize..line.g1 as usize] {
            if lx < (g.x0 + g.x1) / 2.0 {
                return g.byte;
            }
        }
        line.byte1
    }

    /// The horizontal range of `[a, b)` on a line, if any.
    pub fn x_range(&self, line: &Line, a: u32, b: u32) -> Option<(f32, f32)> {
        if a >= line.byte1 && !(line.byte0 == line.byte1 && a <= line.byte0) || b <= line.byte0 && line.byte0 != line.byte1 {
            return None;
        }
        let glyphs = &self.glyphs[line.g0 as usize..line.g1 as usize];
        let (mut x0, mut x1) = (None, None);
        for g in glyphs {
            if g.byte >= a && g.byte < b {
                if x0.is_none() {
                    x0 = Some(g.x0);
                }
                x1 = Some(g.x1);
            }
        }
        match (x0, x1) {
            (Some(p), Some(q)) => Some((line.x + p - line.x_shift, line.x + q - line.x_shift)),
            _ => None,
        }
    }
}

// ---- the layouter -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Ctx {
    family: Family,
    bold: bool,
    italic: bool,
    size: f32,
    lh: f32,
    color: u32,
    align: Align,
    x: f32,
    w: f32,
}

pub struct Layouter<'a> {
    c: &'a ChunkContent,
    o: Options,
    p: Palette,
    st: &'a ChunkState,
    shaper: &'a mut dyn Shaper,
    assets: &'a dyn Assets,
    out: ChunkLayout,
    layer: usize,
    y: f32,
    pending: f32,
    glyph_buf: Vec<RawGlyph>,
}

/// Lays out one chunk. `first` and `last` say whether it starts or ends the document, which is
/// where the document padding goes.
pub fn layout_chunk(c: &ChunkContent, o: &Options, st: &ChunkState, first: bool, last: bool, shaper: &mut dyn Shaper, assets: &dyn Assets) -> ChunkLayout {
    let mut l = Layouter {
        c,
        o: *o,
        p: *o.palette(),
        st,
        shaper,
        assets,
        out: ChunkLayout { layers: vec![Layer::default()], ..Default::default() },
        layer: 0,
        y: 0.0,
        pending: 0.0,
        glyph_buf: Vec::new(),
    };
    let ctx = Ctx {
        family: if o.serif { Family::Serif } else { Family::Sans },
        bold: false,
        italic: false,
        size: o.font_size,
        lh: o.line_height,
        color: l.p.text,
        align: Align::Default,
        x: o.padding,
        w: (o.width - o.padding * 2.0).max(1.0),
    };
    if first {
        l.y += o.padding;
    }
    l.blocks(&c.blocks, ctx, first, false);
    // `.chunk` is a flow root: its children's margins stay inside it.
    l.flush();
    if last {
        l.y += o.padding;
    }
    l.out.height = l.y.ceil().max(1.0);
    l.out.glyphs = l.out.layers.iter().flat_map(|x| &x.boxes).map(|b| b.glyphs.len()).sum();
    l.out
}

/// The viewer's height estimate (`rawEstimate` in `viewer.js`).
pub fn estimate(bytes: u32, lines: u32, o: &Options) -> f32 {
    let lh = o.font_size * o.line_height;
    let width = (o.width - o.padding * 2.0).max(200.0);
    let per_line = (width / (o.font_size * 0.5)).max(20.0);
    (bytes as f32 / per_line + lines as f32 * 0.45) * lh
}

impl Layouter<'_> {
    fn flush(&mut self) {
        self.y += self.pending;
        self.pending = 0.0;
    }

    fn margin(&mut self, m: f32) {
        self.pending = self.pending.max(m);
    }

    fn rect(&mut self, item: RectItem) {
        self.out.layers[self.layer].rects.push(item);
    }

    fn blocks(&mut self, blocks: &[Block], ctx: Ctx, first_zero: bool, last_zero: bool) {
        let n = blocks.len();
        for (i, b) in blocks.iter().enumerate() {
            self.block(b, ctx, first_zero && i == 0, last_zero && i + 1 == n);
        }
    }

    fn block(&mut self, b: &Block, ctx: Ctx, no_top: bool, no_bottom: bool) {
        let bottom = |this: &mut Self, m: f32| {
            if !no_bottom {
                this.margin(m);
            }
        };
        match b {
            Block::Heading { level, content, .. } => self.heading(*level, content, ctx, no_top, no_bottom),
            Block::Paragraph { content, tight, .. } => {
                self.flush();
                let (tb, h) = self.inline_box(content, None, ctx, self.o.wrap || true);
                self.push_box(tb);
                self.y += h;
                if !*tight {
                    bottom(self, ctx.size);
                }
            }
            Block::Quote { blocks, .. } => {
                self.flush();
                let top = self.y;
                let pad_y = 0.65 * ctx.size;
                let pad_x = ctx.size;
                self.y += pad_y;
                let mut inner = ctx;
                inner.x += 4.0 + pad_x;
                inner.w = (ctx.w - 4.0 - pad_x * 2.0).max(1.0);
                self.blocks(blocks, inner, true, true);
                self.flush();
                self.y += pad_y;
                let h = self.y - top;
                self.rect(RectItem { rect: Rect::new(ctx.x, top, 4.0, h), fill: Some(self.p.accent), border: None, radius: 0.0 });
                bottom(self, ctx.size);
            }
            Block::List(l) => {
                self.list(l, ctx, 0, ListPlace::Block, no_bottom);
            }
            Block::Code(cb) => self.code(cb, ctx, no_bottom),
            Block::Table(t) => self.table(t, ctx, no_bottom),
            Block::Rule { .. } => {
                let m = 2.0 * ctx.size;
                if !no_top {
                    self.margin(m);
                }
                self.flush();
                self.rect(RectItem { rect: Rect::new(ctx.x, self.y, ctx.w, 1.0), fill: Some(self.p.border), border: None, radius: 0.0 });
                self.y += 1.0;
                bottom(self, m);
            }
            Block::Details(d) => self.details(d, ctx, no_bottom),
            Block::DefinitionList { items, .. } => {
                for item in items {
                    self.flush();
                    let mut term = ctx;
                    term.bold = true;
                    if !item.term.is_empty() {
                        let (tb, h) = self.inline_box(&item.term, None, term, true);
                        self.push_box(tb);
                        self.y += h;
                    }
                    for def in &item.definitions {
                        let mut inner = ctx;
                        inner.x += 1.5 * ctx.size;
                        inner.w = (ctx.w - 1.5 * ctx.size).max(1.0);
                        self.blocks(def, inner, false, false);
                    }
                }
                bottom(self, ctx.size);
            }
            Block::Footnotes { items, .. } => self.footnotes(items, ctx),
            Block::Aligned { align, blocks, .. } => {
                let mut inner = ctx;
                inner.align = *align;
                self.blocks(blocks, inner, no_top, no_bottom);
            }
            Block::Math { src, display, span } => self.math_block(src, *display, *span, ctx, no_bottom),
            Block::Mermaid { src, span } => self.mermaid(src, *span, ctx, no_bottom),
        }
    }

    fn heading(&mut self, level: u8, content: &[Inline], ctx: Ctx, no_top: bool, no_bottom: bool) {
        let ratio = match level {
            1 => 1.9,
            2 => 1.45,
            3 => 1.2,
            4 => 1.05,
            5 => 0.95,
            _ => 0.9,
        };
        let size = ctx.size * ratio;
        if !no_top {
            self.margin(1.6 * size);
        }
        self.flush();
        let mut hc = ctx;
        hc.family = Family::Sans;
        hc.bold = true;
        hc.size = size;
        hc.lh = 1.3;
        hc.color = if level == 6 { self.p.muted } else { self.p.text_strong };
        if level == 1 && ctx.align == Align::Default {
            hc.align = Align::Centre;
        }
        let prep = self.prepare(content, hc);
        let lines = balance(&prep, hc.w, self.c);
        let (tb, h) = self.place(prep, &lines, hc, hc.x, self.y, hc.w, true);
        self.push_box(tb);
        self.y += h;
        let pad = match level {
            1 => 0.3 * size,
            2 => 0.25 * size,
            _ => 0.0,
        };
        if pad > 0.0 {
            self.y += pad;
            self.rect(RectItem { rect: Rect::new(ctx.x, self.y, ctx.w, 1.0), fill: Some(self.p.border), border: None, radius: 0.0 });
            self.y += 1.0;
        }
        if !no_bottom {
            self.margin(0.6 * size);
        }
    }

    fn list(&mut self, l: &List, ctx: Ctx, depth: u32, place: ListPlace, no_bottom: bool) {
        // `ul:has(> li > input[type=checkbox])`: only a tight item has the box directly in `li`.
        let checkbox_list = !l.ordered && l.tight && l.items.iter().any(|i| i.checked.is_some());
        let indent = if checkbox_list { 1.2 } else { 1.7 } * ctx.size;
        let mut inner = ctx;
        inner.x += indent;
        inner.w = (ctx.w - indent).max(1.0);
        if place == ListPlace::InItem {
            self.margin(0.25 * ctx.size);
        }
        for (i, item) in l.items.iter().enumerate() {
            if i > 0 {
                self.margin(0.25 * ctx.size);
            }
            self.flush();
            let number = l.start.saturating_add(i as u32);
            let boxes_before = self.out.layers[self.layer].boxes.len();
            let y_before = self.y;
            let check_inset = if checkbox_list && item.checked.is_some() { (13.0 - 0.8 * ctx.size).max(0.0) } else { 0.0 };
            for (bi, b) in item.blocks.iter().enumerate() {
                match b {
                    Block::Paragraph { content, tight, .. } => {
                        self.flush();
                        let first_line_inset = if bi == 0 { check_inset } else { 0.0 };
                        let mut c2 = inner;
                        c2.x += first_line_inset;
                        c2.w -= first_line_inset;
                        let (tb, h) = self.inline_box(content, None, c2, true);
                        self.push_box(tb);
                        self.y += h;
                        if !*tight {
                            self.margin(0.5 * ctx.size);
                        }
                    }
                    Block::List(sub) => self.list(sub, inner, depth + 1, ListPlace::InItem, true),
                    other => self.block(other, inner, false, false),
                }
            }
            if self.y == y_before && self.pending == 0.0 {
                // An empty item still has a line for its marker.
                self.y += ctx.size * ctx.lh;
            }
            // Items in a list hold no margin of their own but their content's collapses through.
            let baseline = self.out.layers[self.layer].boxes[boxes_before..].iter().find_map(|b| b.lines.first().map(|l| l.baseline));
            let baseline = baseline.unwrap_or(y_before + ctx.size * ctx.lh * 0.75);
            self.marker(l, item, number, depth, inner, ctx, baseline, checkbox_list);
        }
        if place == ListPlace::InItem || no_bottom {
            // `li > ul, li > ol { margin-bottom: 0 }`.
        } else {
            self.margin(ctx.size);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn marker(&mut self, l: &List, item: &ListItem, number: u32, depth: u32, inner: Ctx, ctx: Ctx, baseline: f32, checkbox_list: bool) {
        if checkbox_list {
            if let Some(on) = item.checked {
                let x = inner.x - 1.25 * ctx.size;
                let y = baseline + 0.1 * ctx.size - 13.0;
                self.checkbox(x, y, on);
            }
            return;
        }
        let font = self.font(ctx);
        let (asc, _) = self.shaper.metrics(font, ctx.size);
        if l.ordered {
            let label = format!("{number}. ");
            let tb = self.chrome_text(&label, font, ctx.size, ctx.color, 0.0, 0.0);
            let w = tb.2;
            self.push_chrome(tb, inner.x - w, baseline);
            return;
        }
        if item.checked.is_some() {
            // A loose task item keeps its bullet and shows the box inline. The box sits at the
            // start of the first line, in the marker's column.
            if let Some(on) = item.checked {
                let x = inner.x - 18.0;
                self.checkbox(x, baseline + 0.1 * ctx.size - 13.0, on);
            }
            return;
        }
        // Blink's symbol marker metrics (ListMarker): integer arithmetic on the rounded ascent.
        let a = asc.round() as i32;
        let bw = ((a * 2 / 3 + 1) / 2) as f32;
        let off = (a * 2 / 3) as f32;
        let top_gap = (3 * (a - a * 2 / 3) / 2) as f32;
        let mx = inner.x - (off + 7.0 + 1.0) + 1.0;
        let my = baseline - a as f32 + top_gap;
        let rect = Rect::new(mx, my, bw, bw);
        match depth {
            0 => self.rect(RectItem { rect, fill: Some(ctx.color), border: None, radius: bw / 2.0 }),
            1 => self.rect(RectItem { rect, fill: None, border: Some(Border::all(ctx.color, 1.0)), radius: bw / 2.0 }),
            _ => self.rect(RectItem { rect, fill: Some(ctx.color), border: None, radius: 0.0 }),
        }
    }

    fn checkbox(&mut self, x: f32, y: f32, on: bool) {
        let rect = Rect::new(x, y, 13.0, 13.0);
        if on {
            // style.css sets no `accent-color` on document checkboxes, so they keep the browser's
            // own defaults for `color-scheme`.
            let (fill, tick) = if self.o.mode == Mode::Dark { (0x99c8ff, 0x000000) } else { (0x0075ff, 0xffffff) };
            self.rect(RectItem { rect, fill: Some(fill), border: None, radius: 2.0 });
            self.out.layers[self.layer].polys.extend(check_mark(x, y, 13.0, tick));
        } else {
            let border = if self.o.mode == Mode::Dark { 0x858585 } else { 0x767676 };
            self.rect(RectItem { rect, fill: Some(self.p.bg), border: Some(Border::all(border, 1.0)), radius: 2.0 });
        }
    }

    fn footnotes(&mut self, items: &[Footnote], ctx: Ctx) {
        let mut c = ctx;
        c.size = ctx.size * 0.9;
        c.color = self.p.muted;
        self.margin(2.5 * c.size);
        self.flush();
        self.rect(RectItem { rect: Rect::new(ctx.x, self.y, ctx.w, 1.0), fill: Some(self.p.border), border: None, radius: 0.0 });
        self.y += 1.0;
        self.margin(1.2 * c.size);
        let indent = 1.7 * c.size;
        let mut inner = c;
        inner.x += indent;
        inner.w = (c.w - indent).max(1.0);
        for (i, n) in items.iter().enumerate() {
            if i > 0 {
                self.margin(0.25 * c.size);
            }
            self.flush();
            let before = self.out.layers[self.layer].boxes.len();
            for b in &n.blocks {
                match b {
                    Block::Paragraph { content, .. } => {
                        self.flush();
                        let (tb, h) = self.inline_box(content, None, inner, true);
                        self.push_box(tb);
                        self.y += h;
                        self.margin(0.5 * c.size);
                    }
                    other => self.block(other, inner, false, false),
                }
            }
            if let Some(bl) = self.out.layers[self.layer].boxes[before..].iter().find_map(|b| b.lines.first().map(|l| l.baseline)) {
                let font = self.font(c);
                let tb = self.chrome_text(&format!("{}. ", n.number), font, c.size, c.color, 0.0, 0.0);
                let w = tb.2;
                self.push_chrome(tb, inner.x - w, bl);
            }
        }
        self.margin(ctx.size);
    }

    fn details(&mut self, d: &Details, ctx: Ctx, no_bottom: bool) {
        self.flush();
        let key = d.span.text.start;
        let open = self.st.details.get(&key).copied().unwrap_or(d.open);
        let top = self.y;
        let pad_y = 0.4 * ctx.size;
        let pad_x = 0.9 * ctx.size;
        self.y += 1.0 + pad_y;
        let mut inner = ctx;
        inner.x += 1.0 + pad_x;
        inner.w = (ctx.w - 2.0 - pad_x * 2.0).max(1.0);
        let mut sum = inner;
        sum.bold = true;
        let marker_w = 0.75 * ctx.size + 0.35 * ctx.size;
        sum.x += marker_w;
        sum.w = (sum.w - marker_w).max(1.0);
        let sum_top = self.y;
        let (tb, h) = if d.summary.is_empty() {
            let font = self.font(sum);
            let t = self.chrome_text("Details", font, sum.size, sum.color, 0.0, 0.0);
            let line_h = sum.size * sum.lh;
            let (ascent, descent) = self.shaper.metrics(font, sum.size);
            let baseline = self.y + (line_h - (ascent + descent)) / 2.0 + ascent;
            self.push_chrome(t, sum.x, baseline);
            (None, line_h)
        } else {
            let (tb, h) = self.inline_box(&d.summary, None, sum, true);
            (Some(tb), h)
        };
        // The disclosure triangle, drawn in the marker's column.
        let tri = 0.56 * ctx.size;
        let (tx, ty) = (inner.x, sum_top + (ctx.size * ctx.lh - tri) / 2.0);
        let color = ctx.color;
        let points = if open { vec![(tx, ty + tri * 0.12), (tx + tri, ty + tri * 0.12), (tx + tri / 2.0, ty + tri * 0.9)] } else { vec![(tx + tri * 0.12, ty), (tx + tri * 0.9, ty + tri / 2.0), (tx + tri * 0.12, ty + tri)] };
        self.out.layers[self.layer].polys.push(Poly { points, color });
        if let Some(tb) = tb {
            self.push_box(tb);
        }
        self.y += h;
        self.out.hits.push(Hit { rect: Rect::new(inner.x, sum_top, inner.w, h), kind: HitKind::Details { key }, layer: self.layer as u32, underline: None });
        if open {
            self.margin(0.5 * ctx.size);
            self.blocks(&d.blocks, inner, false, false);
            self.flush();
        }
        self.y += pad_y + 1.0;
        let hgt = self.y - top;
        self.rect(RectItem { rect: Rect::new(ctx.x, top, ctx.w, hgt), fill: None, border: Some(Border::all(self.p.border, 1.0)), radius: 0.0 });
        if !no_bottom {
            self.margin(ctx.size);
        }
    }

    // ---- inline content ---------------------------------------------------------------------

    fn font(&self, c: Ctx) -> FontSpec {
        FontSpec { family: c.family, bold: c.bold, italic: c.italic }
    }

    fn push_box(&mut self, tb: TextBox) {
        self.emit_decor(&tb);
        self.out.layers[self.layer].boxes.push(tb);
    }

    fn inline_box(&mut self, content: &[Inline], _unused: Option<()>, ctx: Ctx, wrap: bool) -> (TextBox, f32) {
        let prep = self.prepare(content, ctx);
        let lines = break_lines(&prep, if wrap { Some(ctx.w) } else { None }, self.c, true);
        self.place(prep, &lines, ctx, ctx.x, self.y, ctx.w, true)
    }

    /// Turns inlines into shaped runs along one unwrapped line per hard break.
    fn prepare(&mut self, content: &[Inline], ctx: Ctx) -> Prepared {
        let font = self.font(ctx);
        let (asc, desc) = self.shaper.metrics(font, ctx.size);
        let mut p = Prepared { strut: (asc, desc, ctx.size * ctx.lh), ..Default::default() };
        let mut runs = Vec::new();
        self.flatten(content, RunStyle::from_ctx(ctx), &mut runs);
        self.shape_runs(&runs, &mut p);
        p.byte0 = runs.first().map_or(self.c.text.len() as u32, |r| r.start());
        p.byte1 = runs.iter().rev().find_map(|r| r.end()).unwrap_or(p.byte0);
        p
    }

    fn flatten(&self, content: &[Inline], st: RunStyle, out: &mut Vec<Run>) {
        for i in content {
            match i {
                Inline::Text(s) => out.push(Run::Text(*s, st)),
                Inline::Code(s) => out.push(Run::Text(*s, code_style(st, self.p))),
                Inline::Break => out.push(Run::Break),
                Inline::Styled { style, content } => {
                    let mut n = st;
                    match style {
                        InlineStyle::Emphasis => n.italic = true,
                        InlineStyle::Strong => {
                            n.bold = true;
                            n.color = self.p.text_strong;
                            if st.link.is_some() {
                                n.color = st.color;
                            }
                        }
                        InlineStyle::Strikethrough => n.strike = true,
                        InlineStyle::Underline => n.underline = true,
                        InlineStyle::Kbd => {
                            n.family = Family::Mono;
                            n.bold = false;
                            n.size = st.size * 0.87;
                            n.decor = Decor::Kbd;
                            n.pad_l = 0.4 * n.size + 1.0;
                            n.pad_r = n.pad_l;
                        }
                        InlineStyle::Sub | InlineStyle::Sup | InlineStyle::Small => {
                            n.size = st.size / 1.2;
                            match style {
                                InlineStyle::Sub => n.shift = st.shift + (st.size / 5.0 + 1.0).floor(),
                                InlineStyle::Sup => n.shift = st.shift - (st.size / 3.0 + 1.0).floor(),
                                _ => {}
                            }
                            n.lh_px = n.size * st.lh_ratio;
                        }
                        InlineStyle::Mark => n.decor = Decor::Mark,
                        InlineStyle::Mono => n.family = Family::Mono,
                        InlineStyle::Code => n = code_style(st, self.p),
                    }
                    self.flatten(content, n, out);
                }
                Inline::Link { target, content, .. } => {
                    let mut n = st;
                    n.color = self.p.accent;
                    n.link = Some(LinkRef::Target(target as *const LinkTarget));
                    self.flatten(content, n, out);
                }
                Inline::Image(img) => self.flatten_image(img, st, out),
                Inline::Math { src, text } => match self.assets.math(src, false, st.size) {
                    Some(a) => out.push(Run::Atom(AtomDraw::Asset { key: a.key, w: a.width, h: a.height }, a.height, a.height - a.baseline, text.start)),
                    None => {
                        let mut n = st;
                        n.family = Family::Mono;
                        n.size = st.size * 0.87;
                        n.color = self.p.muted;
                        n.decor = Decor::MathPlaceholder;
                        n.pad_l = 0.2 * n.size;
                        n.pad_r = n.pad_l;
                        out.push(Run::Text(*text, n));
                    }
                },
                Inline::FootnoteRef { target, label, .. } => {
                    let mut n = st;
                    n.size = st.size / 1.2;
                    n.shift = st.shift - (st.size / 3.0 + 1.0).floor();
                    n.lh_px = n.size * st.lh_ratio;
                    n.color = self.p.accent;
                    n.link = Some(LinkRef::Fragment(target.as_str() as *const str));
                    out.push(Run::Text(*label, n));
                }
                Inline::FootnoteBack { target, label } => {
                    let mut n = st;
                    n.color = self.p.accent;
                    n.link = Some(LinkRef::Fragment(target.as_str() as *const str));
                    out.push(Run::Text(*label, n));
                }
            }
        }
    }

    fn flatten_image(&self, img: &Image, st: RunStyle, out: &mut Vec<Run>) {
        let held = |this: &Self, out: &mut Vec<Run>| {
            let mut n = st;
            n.size = st.size * 0.85;
            n.color = this.p.muted;
            n.decor = Decor::HeldImage;
            n.pad_l = 7.0;
            n.pad_r = 7.0;
            n.link = None;
            match img.alt_text.filter(|a| !a.is_empty()) {
                Some(a) => out.push(Run::Text(a, n)),
                None => {
                    let (w, h) = (n.size * 2.0, n.size * 1.4);
                    out.push(Run::Atom(AtomDraw::EmptyBox { w, h }, h, 0.0, img.alt_text.map_or(0, |a| a.start)));
                }
            }
        };
        match self.assets.image(&img.source) {
            ImageState::Ready { width, height, key } if !matches!(img.source, ImageSource::Web(_)) => {
                // `max-width: 100%; height: auto`: a width attribute sets the box, the height
                // follows the image's own proportions.
                let w = img.width.unwrap_or(width).min(st.max_w).max(1.0);
                let h = if width > 0.0 { w * height / width } else { height };
                out.push(Run::Atom(AtomDraw::Image { key, w, h }, h, 0.0, st.offset_hint));
            }
            ImageState::Pending if !matches!(img.source, ImageSource::Web(_)) => {
                // Reserve what is known: the attributes, or nothing yet.
                let w = img.width.unwrap_or(1.0).min(st.max_w);
                let h = img.height.unwrap_or(1.0);
                out.push(Run::Atom(AtomDraw::EmptyBox { w: 0.0, h: 0.0 }, h, 0.0, st.offset_hint));
                let _ = w;
            }
            _ => held(self, out),
        }
    }

    fn shape_runs(&mut self, runs: &[Run], p: &mut Prepared) {
        let c = self.c;
        let mut seg_start = 0usize;
        let mut cursor = 0.0f32;
        let mut i = 0;
        while i < runs.len() {
            match &runs[i] {
                Run::Break => {
                    p.segments.push(seg_start..p.glyphs.len());
                    seg_start = p.glyphs.len();
                    cursor = 0.0;
                    i += 1;
                }
                Run::Atom(draw, up, down, offset) => {
                    let w = match draw {
                        AtomDraw::Image { w, .. } | AtomDraw::Asset { w, .. } | AtomDraw::EmptyBox { w, .. } => *w,
                    };
                    let span = p.spans.len() as u32;
                    let font = FontSpec { family: Family::Sans, bold: false, italic: false };
                    let _ = font;
                    p.spans.push(SpanInfo { ascent: *up, descent: *down, line_height: up + down, size: *up, color: self.p.text, ..Default::default() });
                    let id = p.atoms.len() as u32;
                    p.atoms.push(draw.clone());
                    p.glyphs.push(Glyph { x0: cursor, x1: cursor + w, byte: *offset, id, font: 0, span, flags: FLAG_ATOM });
                    cursor += w;
                    i += 1;
                }
                Run::Text(s0, st0) => {
                    // Shape neighbours in one call when only their paint differs, for kerning.
                    let font = st0.font();
                    let mut j = i;
                    let mut end = s0.end;
                    let mut pad_inside = st0.pad_r > 0.0;
                    while !pad_inside {
                        let Some(Run::Text(sn, stn)) = runs.get(j + 1) else { break };
                        if stn.font() != font || stn.size != st0.size || sn.start != end || stn.pad_l > 0.0 {
                            break;
                        }
                        end = sn.end;
                        pad_inside = stn.pad_r > 0.0;
                        j += 1;
                    }
                    let start = s0.start;
                    let text = &c.text[start as usize..end as usize];
                    let run_x = cursor + st0.pad_l;
                    self.glyph_buf.clear();
                    let width = self.shaper.shape(text, font, st0.size, &mut self.glyph_buf);
                    let fake = st0.bold && self.shaper.fake_bold(font);
                    // One SpanInfo per source run; the glyph loop advances through them.
                    let mut span_ix = p.spans.len() as u32;
                    let mut run_ix = i;
                    let (ascent, descent) = self.shaper.metrics(font, st0.size);
                    for k in i..=j {
                        if let Run::Text(_, st) = &runs[k] {
                            p.spans.push(st.span_info(ascent, descent, self.link_index(st.link)));
                        }
                    }
                    let n = self.glyph_buf.len();
                    let bytes = c.text.as_bytes();
                    let mut k = 0;
                    while k < n {
                        let g = self.glyph_buf[k];
                        let byte = start + g.byte;
                        while let Some(Run::Text(s, _)) = runs.get(run_ix) {
                            if byte >= s.end && run_ix < j {
                                run_ix += 1;
                                span_ix += 1;
                            } else {
                                break;
                            }
                        }
                        let next_x = if k + 1 < n { self.glyph_buf[k + 1].x } else { width };
                        let mut flags = 0;
                        if g.emoji {
                            flags |= FLAG_EMOJI;
                        }
                        if fake {
                            flags |= FLAG_FAKE_BOLD;
                        }
                        if matches!(bytes.get(byte as usize), Some(b' ' | b'\t' | b'\n')) {
                            flags |= FLAG_SPACE;
                        }
                        p.glyphs.push(Glyph { x0: run_x + g.x, x1: run_x + next_x, byte, id: g.id, font: g.font, span: span_ix, flags });
                        k += 1;
                    }
                    let last_pad = if let Run::Text(_, st) = &runs[j] { st.pad_r } else { 0.0 };
                    cursor = run_x + width + last_pad;
                    i = j + 1;
                }
            }
        }
        p.segments.push(seg_start..p.glyphs.len());
        p.total = cursor;
    }

    fn link_index(&mut self, link: Option<LinkRef>) -> Option<u32> {
        link.map(|l| {
            let target = match l {
                // Safety: the pointer is into the chunk borrowed for this whole layout.
                LinkRef::Target(t) => unsafe { (*t).clone() },
                LinkRef::Fragment(f) => LinkTarget::Fragment(unsafe { (*f).to_string() }),
            };
            if let Some(i) = self.out.links.iter().rposition(|x| *x == target) {
                return i as u32;
            }
            self.out.links.push(target);
            self.out.links.len() as u32 - 1
        })
    }

    /// Positions broken lines. Returns the box and its height.
    #[allow(clippy::too_many_arguments)]
    fn place(&mut self, p: Prepared, lines: &[LineRange], ctx: Ctx, x: f32, y: f32, w: f32, selectable: bool) -> (TextBox, f32) {
        let mut out_lines = Vec::with_capacity(lines.len());
        let mut cy = y;
        let (sa, sd, sl) = p.strut;
        let half = (sl - (sa + sd)) / 2.0;
        for l in lines {
            let (mut top, mut bot) = (sa + half, sd + half);
            let mut last_span = u32::MAX;
            for g in &p.glyphs[l.g0..l.g1] {
                if g.span != last_span {
                    last_span = g.span;
                    let s = &p.spans[g.span as usize];
                    if g.flags & FLAG_ATOM != 0 {
                        top = top.max(s.ascent);
                        bot = bot.max(s.descent);
                    } else {
                        let h = (s.line_height - (s.ascent + s.descent)) / 2.0;
                        top = top.max(s.ascent + h - s.shift);
                        bot = bot.max(s.descent + h + s.shift);
                    }
                }
            }
            let height = top + bot;
            let baseline = cy + top;
            let lx = match ctx.align {
                Align::Centre => x + (w - l.width) / 2.0,
                Align::Right => x + w - l.width,
                _ => x,
            };
            let lx = lx.max(x);
            out_lines.push(Line {
                x: lx,
                x_shift: l.x_start,
                top: cy,
                height,
                baseline,
                g0: l.g0 as u32,
                g1: l.g1 as u32,
                byte0: l.byte0,
                byte1: l.byte1,
                width: l.width,
            });
            cy += height;
        }
        let h = cy - y;
        let tb = TextBox {
            lines: out_lines,
            glyphs: p.glyphs,
            spans: p.spans,
            atoms: p.atoms,
            byte0: p.byte0,
            byte1: p.byte1,
            rect: Rect::new(x, y, w, h),
            selectable,
            code: false,
        };
        (tb, h)
    }

    /// Adds inline backgrounds and borders, underlines and strikes, and link hit areas.
    fn emit_decor(&mut self, tb: &TextBox) {
        let layer = self.layer as u32;
        for l in &tb.lines {
            let mut i = l.g0 as usize;
            let end = l.g1 as usize;
            while i < end {
                let sp = tb.glyphs[i].span;
                let mut j = i + 1;
                while j < end && tb.glyphs[j].span == sp {
                    j += 1;
                }
                let s = tb.spans[sp as usize];
                if tb.glyphs[i].flags & FLAG_ATOM == 0 {
                    let first_of_span = i == 0 || tb.glyphs[i - 1].span != sp;
                    let last_of_span = j == tb.glyphs.len() || tb.glyphs[j].span != sp;
                    let gx0 = l.x + tb.glyphs[i].x0 - l.x_shift;
                    let gx1 = l.x + tb.glyphs[j - 1].x1 - l.x_shift;
                    let x0 = gx0 - if first_of_span { s.pad_l } else { 0.0 };
                    let x1 = gx1 + if last_of_span { s.pad_r } else { 0.0 };
                    let base = l.baseline + s.shift;
                    self.decor(&s, x0, x1, base);
                    if let Some(link) = s.link {
                        let rect = Rect::new(gx0, l.top, gx1 - gx0, l.height);
                        let ul = base + s.size * 0.12;
                        self.out.hits.push(Hit { rect, kind: HitKind::Link(link), layer, underline: Some((ul, s.color)) });
                    }
                }
                i = j;
            }
        }
    }

    fn decor(&mut self, s: &SpanInfo, x0: f32, x1: f32, base: f32) {
        let p = self.p;
        let (top, bottom) = (base - s.ascent, base + s.descent);
        match s.decor {
            Decor::None => {}
            Decor::Code => {
                let v = 0.12 * s.size;
                self.rect(RectItem { rect: Rect::new(x0, top - v - 1.0, x1 - x0, bottom - top + 2.0 * v + 2.0), fill: Some(p.bg_code), border: Some(Border::all(p.border, 1.0)), radius: 0.0 });
            }
            Decor::Kbd => {
                let v = 0.1 * s.size;
                let mut b = Border::all(p.border, 1.0);
                b.bottom = 2.0;
                self.rect(RectItem { rect: Rect::new(x0, top - v - 1.0, x1 - x0, bottom - top + 2.0 * v + 3.0), fill: Some(p.bg_soft), border: Some(b), radius: 0.0 });
            }
            Decor::Mark => self.rect(RectItem { rect: Rect::new(x0, top, x1 - x0, bottom - top), fill: Some(p.mark), border: None, radius: 0.0 }),
            Decor::HeldImage => {
                let mut b = Border::all(p.border, 1.0);
                b.dashed = true;
                self.rect(RectItem { rect: Rect::new(x0, top - 2.0, x1 - x0, bottom - top + 4.0), fill: None, border: Some(b), radius: 0.0 });
            }
            Decor::MathPlaceholder => {
                let mut b = Border::all(p.border, 1.0);
                b.dashed = true;
                self.rect(RectItem { rect: Rect::new(x0, top - 1.0, x1 - x0, bottom - top + 2.0), fill: None, border: Some(b), radius: 0.0 });
            }
        }
        if s.underline {
            self.rect(RectItem { rect: Rect::new(x0, base + s.size * 0.12, x1 - x0, 1.0), fill: Some(s.color), border: None, radius: 0.0 });
        }
        if s.strike {
            self.rect(RectItem { rect: Rect::new(x0, base - s.size * 0.3, x1 - x0, 1.0), fill: Some(s.color), border: None, radius: 0.0 });
        }
    }

    // ---- code and placeholders --------------------------------------------------------------

    fn code(&mut self, cb: &CodeBlock, ctx: Ctx, no_bottom: bool) {
        self.flush();
        let size = ctx.size * 0.87;
        let top_pad = if cb.copyable { 2.8 * size } else { 0.9 * size };
        self.pre_box(cb.text, &cb.tokens, cb.span.text.start, cb.copyable, None, size, top_pad, ctx);
        if !no_bottom {
            self.margin(ctx.size * 0.87);
        }
    }

    /// Mermaid before its asset arrives: `pre.mermaid`, in muted text.
    fn mermaid(&mut self, src: &str, span: Span, ctx: Ctx, no_bottom: bool) {
        self.flush();
        if let Some(a) = self.assets.mermaid(src) {
            self.asset_block(a, ctx);
        } else {
            let size = ctx.size * 0.87;
            let text = ByteSpan::new(span.text.start, span.text.start + src.len() as u32);
            let muted = self.p.muted;
            self.pre_box(text, &[], span.text.start, false, Some(muted), size, 0.9 * size, ctx);
        }
        if !no_bottom {
            self.margin(ctx.size * 0.87);
        }
    }

    fn math_block(&mut self, src: &str, display: bool, span: Span, ctx: Ctx, no_bottom: bool) {
        self.flush();
        if let Some(a) = self.assets.math(src, display, ctx.size) {
            self.asset_block(a, ctx);
        } else {
            // The placeholder: its source in mono in a dashed box, centred like the real thing.
            let mut c = ctx;
            c.family = Family::Mono;
            c.size = ctx.size * 0.87;
            c.color = self.p.muted;
            c.lh = 1.5;
            c.align = Align::Centre;
            let pad = 0.4 * ctx.size;
            let top = self.y;
            c.x += pad + 1.0;
            c.w = (ctx.w - 2.0 * pad - 2.0).max(1.0);
            self.y += pad + 1.0;
            let text = ByteSpan::new(span.text.start, span.text.start + src.len() as u32);
            let (tb, h) = self.code_lines(text, &[], c, Some(c.w), Some(c.color), Align::Centre);
            self.push_box(tb);
            self.y += h + pad + 1.0;
            let mut b = Border::all(self.p.border, 1.0);
            b.dashed = true;
            self.rect(RectItem { rect: Rect::new(ctx.x, top, ctx.w, self.y - top), fill: None, border: Some(b), radius: 0.0 });
        }
        if !no_bottom {
            self.margin(ctx.size);
        }
    }

    fn asset_block(&mut self, a: AssetBox, ctx: Ctx) {
        let w = a.width.min(ctx.w);
        let h = if a.width > 0.0 { a.height * w / a.width } else { a.height };
        let x = ctx.x + (ctx.w - w) / 2.0;
        let draw = AtomDraw::Asset { key: a.key, w, h };
        let tb = TextBox {
            lines: vec![Line { x, x_shift: 0.0, top: self.y, height: h, baseline: self.y + h, g0: 0, g1: 1, byte0: 0, byte1: 0, width: w }],
            glyphs: vec![Glyph { x0: 0.0, x1: w, byte: 0, id: 0, font: 0, span: 0, flags: FLAG_ATOM }],
            spans: vec![SpanInfo::default()],
            atoms: vec![draw],
            rect: Rect::new(x, self.y, w, h),
            ..Default::default()
        };
        self.out.layers[self.layer].boxes.push(tb);
        self.y += h;
    }

    /// `.md pre`: the bordered box, its text and, for code, the copy button.
    #[allow(clippy::too_many_arguments)]
    fn pre_box(&mut self, text: ByteSpan, tokens: &[CodeToken], key: u32, copy: bool, color: Option<u32>, size: f32, top_pad: f32, ctx: Ctx) {
        let pad_x = 1.1 * size;
        let bottom_pad = 0.9 * size;
        let top = self.y;
        let box_x = ctx.x;
        let box_w = ctx.w;
        let inner_w = (box_w - 2.0 - pad_x * 2.0).max(1.0);
        let mut c = ctx;
        c.family = Family::Mono;
        c.bold = false;
        c.italic = false;
        c.size = size;
        c.lh = 1.5;
        c.color = self.p.code_text;
        c.x = box_x + 1.0 + pad_x;
        c.w = inner_w;
        c.align = Align::Default;
        self.y += 1.0 + top_pad;
        let wrap_w = self.o.wrap.then_some(inner_w);
        let (tb, h) = self.code_lines(text, tokens, c, wrap_w, color, Align::Default);
        let content_w = tb.lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
        let overflow = !self.o.wrap && content_w > inner_w + 0.5;
        // The box background and border come first, in the chunk's own layer.
        let scrollbar = if overflow { 12.0 } else { 0.0 };
        let full_h = 1.0 + top_pad + h + bottom_pad + scrollbar + 1.0;
        self.rect(RectItem { rect: Rect::new(box_x, top, box_w, full_h), fill: Some(self.p.bg_code), border: Some(Border::all(self.p.border, 1.0)), radius: 0.0 });
        if overflow {
            self.out.layers.push(Layer::default());
            let view = Rect::new(box_x + 1.0, top + 1.0, box_w - 2.0, full_h - 2.0 - scrollbar);
            self.out.areas.push(Area { key, view, content_w: content_w + 2.0 * pad_x, scrollbar_y: view.bottom() });
            self.out.layers.last_mut().expect("layer").boxes.push(tb);
        } else {
            self.push_box(tb);
        }
        self.y = top + full_h;
        if copy {
            self.copy_button(key, text, box_x + box_w, top);
        }
    }

    fn copy_button(&mut self, key: u32, code: ByteSpan, right: f32, top: f32) {
        let done = self.st.copied == Some(key);
        let color = if done { self.p.accent } else { self.p.muted };
        let label = if done { "COPIED" } else { "COPY" };
        let family = if self.o.serif { Family::Serif } else { Family::Sans };
        let font = FontSpec { family, bold: false, italic: false };
        let size = self.o.font_size;
        let t = self.chrome_text(label, font, size, color, 0.0, 0.0);
        let w = 8.0 + 15.0 + 6.0 + t.2 + 8.0 + 2.0;
        let (x, y) = (right - 6.0 - w, top + 6.0);
        self.rect(RectItem { rect: Rect::new(x, y, w, 28.0), fill: Some(self.p.bg_soft), border: Some(Border::all(self.p.border, 1.0)), radius: 0.0 });
        let ix = x + 1.0 + 8.0;
        let iy = y + (28.0 - 15.0) / 2.0;
        if done {
            self.out.layers[self.layer].polys.extend(check_mark(ix, iy, 15.0, color));
        } else {
            self.copy_icon(ix, iy, color);
        }
        let (asc, desc) = self.shaper.metrics(font, size);
        let baseline = y + (28.0 - (asc + desc)) / 2.0 + asc;
        self.push_chrome(t, ix + 15.0 + 6.0, baseline);
        self.out.hits.push(Hit { rect: Rect::new(x, y, w, 28.0), kind: HitKind::Copy { key, text: code }, layer: 0, underline: None });
    }

    /// The copy icon from `extras.js`: a rounded square in front of another, on a 24-unit grid.
    fn copy_icon(&mut self, x: f32, y: f32, color: u32) {
        let u = 15.0 / 24.0;
        let stroke = 1.8 * u;
        let bg = self.p.bg_soft;
        // The back square, then the front one with an opaque fill hiding the overlap.
        self.rect(RectItem { rect: Rect::new(x + 4.0 * u - stroke / 2.0, y + 4.0 * u - stroke / 2.0, 12.0 * u + stroke, 12.0 * u + stroke), fill: None, border: Some(Border::all(color, stroke)), radius: 2.0 * u });
        self.rect(RectItem { rect: Rect::new(x + 8.0 * u - stroke / 2.0, y + 8.0 * u - stroke / 2.0, 12.0 * u + stroke, 12.0 * u + stroke), fill: Some(bg), border: Some(Border::all(color, stroke)), radius: 2.0 * u });
    }

    /// Text in code form: logical lines, tabs expanded, coloured by token.
    fn code_lines(&mut self, text: ByteSpan, tokens: &[CodeToken], c: Ctx, wrap: Option<f32>, color: Option<u32>, align: Align) -> (TextBox, f32) {
        let font = FontSpec { family: Family::Mono, bold: false, italic: false };
        let (asc, desc) = self.shaper.metrics(font, c.size);
        let line_h = c.size * c.lh;
        let mode = self.o.mode;
        let default = color.unwrap_or(c.color);
        let mut p = Prepared { strut: (asc, desc, line_h), ..Default::default() };
        p.byte0 = text.start;
        p.byte1 = text.end;
        // Span 0 is plain code; each token class that appears gets one more.
        let plain = SpanInfo { color: default, size: c.size, ascent: asc, descent: desc, line_height: line_h, ..Default::default() };
        p.spans.push(plain);
        let mut class_spans: HashMap<(u32, bool, bool), u32> = HashMap::new();
        let body = &self.c.text[text.range()];
        let mut tok_i = 0usize;
        let mut pos = text.start;
        let mut space_w = None;
        for line in body.split('\n') {
            let line_start = pos;
            let line_end = line_start + line.len() as u32;
            let seg_start = p.glyphs.len();
            let mut cursor = 0.0f32;
            let mut at = line_start;
            // Shape stretches of one face together; only the rest of a token's style varies.
            while at < line_end {
                let (sp, stretch_end, spec) = {
                    while tok_i < tokens.len() && tokens[tok_i].span.end <= at {
                        tok_i += 1;
                    }
                    let tok = tokens.get(tok_i);
                    match tok {
                        Some(t) if t.span.start <= at => {
                            let (col, b, i) = token_style(t.class, mode);
                            let col = col.unwrap_or(default);
                            let spec = FontSpec { family: Family::Mono, bold: b, italic: i };
                            let id = *class_spans.entry((col, b, i)).or_insert_with(|| {
                                p.spans.push(SpanInfo { color: col, ..plain });
                                p.spans.len() as u32 - 1
                            });
                            (id, t.span.end.min(line_end), spec)
                        }
                        Some(t) => (0, t.span.start.min(line_end), font),
                        None => (0, line_end, font),
                    }
                };
                // Extend over following stretches in the same face.
                let mut spans_here: Vec<(u32, u32)> = vec![(stretch_end, sp)];
                let mut end = stretch_end;
                let mut ti = tok_i;
                while end < line_end {
                    while ti < tokens.len() && tokens[ti].span.end <= end {
                        ti += 1;
                    }
                    let (nsp, nend, nspec) = match tokens.get(ti) {
                        Some(t) if t.span.start <= end => {
                            let (col, b, i) = token_style(t.class, mode);
                            let col = col.unwrap_or(default);
                            let sp2 = FontSpec { family: Family::Mono, bold: b, italic: i };
                            let id = *class_spans.entry((col, b, i)).or_insert_with(|| {
                                p.spans.push(SpanInfo { color: col, ..plain });
                                p.spans.len() as u32 - 1
                            });
                            (id, t.span.end.min(line_end), sp2)
                        }
                        Some(t) => (0, t.span.start.min(line_end), font),
                        None => (0, line_end, font),
                    };
                    if nspec != spec {
                        break;
                    }
                    spans_here.push((nend, nsp));
                    end = nend;
                }
                let stretch = &self.c.text[at as usize..end as usize];
                self.glyph_buf.clear();
                let has_tab = stretch.contains('\t');
                let width = if has_tab {
                    let replaced = stretch.replace('\t', " ");
                    self.shaper.shape(&replaced, spec, c.size, &mut self.glyph_buf)
                } else {
                    self.shaper.shape(stretch, spec, c.size, &mut self.glyph_buf)
                };
                let fake = spec.bold && self.shaper.fake_bold(spec);
                let mut extra = 0.0f32;
                let n = self.glyph_buf.len();
                let mut si = 0usize;
                for k in 0..n {
                    let g = self.glyph_buf[k];
                    let byte = at + g.byte;
                    while si + 1 < spans_here.len() && byte >= spans_here[si].0 {
                        si += 1;
                    }
                    let next = if k + 1 < n { self.glyph_buf[k + 1].x } else { width };
                    let is_tab = self.c.text.as_bytes()[byte as usize] == b'\t';
                    let mut x1 = cursor + next + extra;
                    if is_tab {
                        // `tab-size: 4`: to the next stop of four space widths.
                        let sw = *space_w.get_or_insert(next - g.x);
                        let stop = sw * 4.0;
                        let x0 = cursor + g.x + extra;
                        let target = ((x0 + 0.01) / stop).floor() * stop + stop;
                        extra += target - x1;
                        x1 = target;
                    }
                    let mut flags = 0;
                    if g.emoji {
                        flags |= FLAG_EMOJI;
                    }
                    if fake {
                        flags |= FLAG_FAKE_BOLD;
                    }
                    if is_tab || self.c.text.as_bytes()[byte as usize] == b' ' {
                        flags |= FLAG_SPACE;
                    }
                    p.glyphs.push(Glyph { x0: cursor + g.x + if is_tab { extra - (x1 - (cursor + next + extra - (x1 - (cursor + next + extra)))) * 0.0 } else { extra }, x1, byte, id: g.id, font: g.font, span: spans_here[si].1, flags });
                }
                cursor += width + extra;
                at = end;
            }
            p.segments.push(seg_start..p.glyphs.len());
            p.seg_bytes.push((line_start, line_end));
            pos = line_end + 1;
        }
        p.total = 0.0;
        let lines = break_lines(&p, wrap, self.c, false);
        let mut cc = c;
        cc.align = align;
        let (mut tb, h) = self.place(p, &lines, cc, c.x, self.y, c.w, true);
        tb.code = true;
        (tb, h)
    }

    // ---- tables -----------------------------------------------------------------------------

    fn table(&mut self, t: &Table, ctx: Ctx, no_bottom: bool) {
        self.flush();
        let key = t.span.text.start;
        let size = ctx.size * 0.94;
        let (pad_y, pad_x) = (0.4 * size, 0.8 * size);
        let cols = t.head.iter().chain(&t.rows).map(|r| r.cells.len()).max().unwrap_or(0);
        if cols == 0 {
            return;
        }
        let wrap = self.o.wrap;
        let sort = self.st.sorts.get(&key).copied();
        let mut head_ctx = ctx;
        head_ctx.size = size;
        head_ctx.lh = 1.5;
        head_ctx.bold = true;
        head_ctx.color = self.p.text_strong;
        let mut body_ctx = head_ctx;
        body_ctx.bold = false;
        body_ctx.color = ctx.color;
        // The display order of body rows.
        let order: Vec<usize> = match sort {
            Some((col, dir)) => {
                let cells: Vec<String> = t.rows.iter().map(|r| r.cells.get(col).map_or(String::new(), |c| self.c.slice(c.text).trim().to_string())).collect();
                sort::sorted_order(&cells, dir)
            }
            None => (0..t.rows.len()).collect(),
        };
        struct Cell {
            prep: Prepared,
            min: f32,
            max: f32,
            ctx: Ctx,
        }
        // Prepare every cell and find each column's minimum and maximum widths.
        let mut grid: Vec<Vec<Option<Cell>>> = Vec::with_capacity(t.head.len() + t.rows.len());
        let mut min_w = vec![0.0f32; cols];
        let mut max_w = vec![0.0f32; cols];
        for (ri, row) in t.head.iter().map(|r| (true, r)).chain(order.iter().map(|&i| (false, &t.rows[i]))).enumerate() {
            let (is_head, row) = row;
            let _ = ri;
            let mut cells = Vec::with_capacity(cols);
            for col in 0..cols {
                let cell = row.cells.get(col);
                let mut c = if is_head { head_ctx } else { body_ctx };
                // Browsers centre `th` by default. The sanitiser removes the `style` attribute that
                // holds a delimiter row's alignment, so only a raw `align` attribute changes this.
                c.align = match cell.map_or(Align::Default, |x| x.align) {
                    Align::Default => {
                        if is_head {
                            Align::Centre
                        } else {
                            ctx.align
                        }
                    }
                    a => a,
                };
                let mut prep = match cell {
                    Some(x) => self.prepare(&x.content, c),
                    None => Prepared { strut: (0.0, 0.0, size * 1.5), ..Default::default() },
                };
                if is_head {
                    if let Some((_, dir)) = sort.filter(|s| s.0 == col) {
                        self.append_sort_arrow(&mut prep, dir, c);
                    }
                }
                let (mn, mx) = cell_widths(&prep, wrap);
                min_w[col] = min_w[col].max(mn);
                max_w[col] = max_w[col].max(mx);
                cells.push(Some(Cell { prep, min: mn, max: mx, ctx: c }));
            }
            grid.push(cells);
        }
        // Column widths: CSS 2.1's automatic table layout. Width includes padding and one border.
        let extra = 2.0 * pad_x + 1.0;
        let avail = ctx.w - 1.0;
        let sum_max: f32 = max_w.iter().map(|w| w + extra).sum();
        let sum_min: f32 = min_w.iter().map(|w| w + extra).sum();
        let widths: Vec<f32> = if sum_max <= avail {
            max_w.iter().map(|w| w + extra).collect()
        } else if sum_min >= avail {
            min_w.iter().map(|w| w + extra).collect()
        } else {
            let room = avail - sum_min;
            let spread = (sum_max - sum_min).max(1.0);
            min_w.iter().zip(&max_w).map(|(mn, mx)| mn + extra + (mx - mn) * room / spread).collect()
        };
        let total_w: f32 = widths.iter().sum::<f32>() + 1.0;
        let overflow = total_w > ctx.w + 0.5;
        let top = self.y;
        if overflow {
            self.out.layers.push(Layer::default());
            self.layer = self.out.layers.len() - 1;
        }
        let mut y = top;
        let head_rows = t.head.len();
        for (ri, row) in grid.into_iter().enumerate() {
            // Break every cell at its column's width to find the row's height.
            let mut placed = Vec::with_capacity(cols);
            let mut row_h = 0.0f32;
            for (col, cell) in row.into_iter().enumerate() {
                let cell = cell.expect("cell");
                let inner = (widths[col] - extra).max(1.0);
                let lines = break_lines(&cell.prep, if wrap { Some(inner) } else { None }, self.c, true);
                let mut h = 0.0;
                {
                    let (sa, sd, sl) = cell.prep.strut;
                    let half = (sl - (sa + sd)) / 2.0;
                    for l in &lines {
                        let (mut t2, mut b2) = (sa + half, sd + half);
                        let mut last = u32::MAX;
                        for g in &cell.prep.glyphs[l.g0..l.g1] {
                            if g.span != last {
                                last = g.span;
                                let s = &cell.prep.spans[g.span as usize];
                                if g.flags & FLAG_ATOM != 0 {
                                    t2 = t2.max(s.ascent);
                                    b2 = b2.max(s.descent);
                                } else {
                                    let hh = (s.line_height - (s.ascent + s.descent)) / 2.0;
                                    t2 = t2.max(s.ascent + hh - s.shift);
                                    b2 = b2.max(s.descent + hh + s.shift);
                                }
                            }
                        }
                        h += t2 + b2;
                    }
                }
                row_h = row_h.max(h);
                placed.push((cell, lines, h, inner));
                let _ = (placed.last().map(|p: &(Cell, Vec<LineRange>, f32, f32)| p.0.min), placed.last().map(|p: &(Cell, Vec<LineRange>, f32, f32)| p.0.max));
            }
            let full_row = row_h + 2.0 * pad_y + 1.0;
            let is_head = ri < head_rows;
            let mut x = ctx.x;
            for (col, (cell, lines, h, inner)) in placed.into_iter().enumerate() {
                let w = widths[col];
                let fill = if is_head { self.p.bg_soft } else { self.p.bg };
                self.rect(RectItem { rect: Rect::new(x, y, w + 1.0, full_row + 1.0), fill: Some(fill), border: Some(Border::all(self.p.border, 1.0)), radius: 0.0 });
                // `vertical-align: middle` is the default for table cells.
                let ty = y + 1.0 + pad_y + (row_h - h) / 2.0;
                let (tb, _) = self.place(cell.prep, &lines, cell.ctx, x + 1.0 + pad_x, ty, inner, true);
                self.push_box(tb);
                if is_head && !t.rows.is_empty() {
                    let layer = self.layer as u32;
                    self.out.hits.push(Hit { rect: Rect::new(x, y, w, full_row), kind: HitKind::Sort { key, col }, layer, underline: None });
                }
                x += w;
            }
            y += full_row;
        }
        y += 1.0;
        let mut full = y - top;
        if overflow {
            let view = Rect::new(ctx.x, top, ctx.w, full);
            self.out.areas.push(Area { key, view, content_w: total_w, scrollbar_y: view.bottom() });
            self.layer = 0;
            full += 12.0;
        }
        self.y = top + full;
        if !no_bottom {
            self.margin(size);
        }
    }

    /// `th[aria-sort]::after`: the arrow, smaller and in the accent colour.
    fn append_sort_arrow(&mut self, p: &mut Prepared, dir: SortDir, c: Ctx) {
        let size = c.size * 0.7;
        let font = FontSpec { family: c.family, bold: c.bold, italic: false };
        let (ascent, descent) = self.shaper.metrics(font, size);
        let span = p.spans.len() as u32;
        p.spans.push(SpanInfo { color: self.p.accent, size, shift: -0.15 * size, ascent, descent, line_height: c.size * c.lh * 0.7, chrome: true, ..Default::default() });
        let label = if dir == SortDir::Ascending { " \u{25b2}" } else { " \u{25bc}" };
        self.glyph_buf.clear();
        let w = self.shaper.shape(label, font, size, &mut self.glyph_buf);
        let start = p.glyphs.last().map_or(0.0, |g| g.x1);
        let n = self.glyph_buf.len();
        let byte = p.byte1;
        for k in 0..n {
            let g = self.glyph_buf[k];
            let next = if k + 1 < n { self.glyph_buf[k + 1].x } else { w };
            let flags = if g.emoji { FLAG_EMOJI } else { 0 };
            p.glyphs.push(Glyph { x0: start + g.x, x1: start + next, byte, id: g.id, font: g.font, span, flags });
        }
        if let Some(seg) = p.segments.last_mut() {
            seg.end = p.glyphs.len();
        } else {
            p.segments.push(0..p.glyphs.len());
        }
    }

    // ---- chrome text ------------------------------------------------------------------------

    fn chrome_text(&mut self, label: &str, font: FontSpec, size: f32, color: u32, shift: f32, x: f32) -> (Vec<Glyph>, SpanInfo, f32) {
        self.glyph_buf.clear();
        let w = self.shaper.shape(label, font, size, &mut self.glyph_buf);
        let (ascent, descent) = self.shaper.metrics(font, size);
        let span = SpanInfo { color, size, shift, ascent, descent, line_height: size, chrome: true, ..Default::default() };
        let n = self.glyph_buf.len();
        let mut glyphs = Vec::with_capacity(n);
        for k in 0..n {
            let g = self.glyph_buf[k];
            let next = if k + 1 < n { self.glyph_buf[k + 1].x } else { w };
            let flags = if g.emoji { FLAG_EMOJI } else { 0 };
            glyphs.push(Glyph { x0: x + g.x, x1: x + next, byte: 0, id: g.id, font: g.font, span: 0, flags });
        }
        (glyphs, span, w)
    }

    fn push_chrome(&mut self, t: (Vec<Glyph>, SpanInfo, f32), x: f32, baseline: f32) {
        let (glyphs, span, w) = t;
        let n = glyphs.len() as u32;
        let h = span.ascent + span.descent;
        self.out.layers[self.layer].boxes.push(TextBox {
            lines: vec![Line { x, x_shift: 0.0, top: baseline - span.ascent, height: h, baseline, g0: 0, g1: n, byte0: 0, byte1: 0, width: w }],
            glyphs,
            spans: vec![span],
            atoms: Vec::new(),
            byte0: 0,
            byte1: 0,
            rect: Rect::new(x, baseline - span.ascent, w, h),
            selectable: false,
            code: false,
        });
    }
}

#[derive(PartialEq, Clone, Copy)]
enum ListPlace {
    Block,
    InItem,
}

fn check_mark(x: f32, y: f32, size: f32, color: u32) -> Vec<Poly> {
    // The tick from extras.js (`m5 12.5 4.5 4.5L19 7.5`) as two stroked segments.
    let u = size / 24.0;
    let stroke = if size > 13.5 { 1.8 * u } else { 3.0 * u };
    let seg = |a: (f32, f32), b: (f32, f32)| {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = (-dy / len * stroke / 2.0, dx / len * stroke / 2.0);
        let p = |q: (f32, f32), s: f32| (x + q.0 * u + nx * s, y + q.1 * u + ny * s);
        Poly { points: vec![p(a, 1.0), p(b, 1.0), p(b, -1.0), p(a, -1.0)], color }
    };
    vec![seg((5.0, 12.5), (9.5, 17.0)), seg((9.5, 17.0), (19.0, 7.5))]
}

/// (colour, bold, italic) for a token class: chroma's `github-dark` and `github`, as dumped from
/// `HighlightCSS()`. `None` means the class has no rule in that style and `--code-text` shows.
pub fn token_style(c: TokenClass, mode: Mode) -> (Option<u32>, bool, bool) {
    use TokenClass as T;
    match mode {
        Mode::Dark => match c {
            T::Keyword | T::KeywordDeclaration | T::KeywordNamespace | T::KeywordType => (Some(0xff7b72), false, false),
            T::KeywordConstant | T::KeywordPseudo | T::NameProperty | T::NameVariable | T::StringEscape | T::StringRegex | T::GenericSubheading => (Some(0x79c0ff), false, false),
            T::Name | T::NameAttribute | T::NameBuiltin | T::NameBuiltinPseudo | T::Punctuation => (Some(0xe6edf3), false, false),
            T::NameClass | T::NameException => (Some(0xf0883e), true, false),
            T::NameConstant | T::NameLabel | T::GenericHeading => (Some(0x79c0ff), true, false),
            T::NameDecorator | T::NameFunction => (Some(0xd2a8ff), true, false),
            T::NameEntity => (Some(0xffa657), false, false),
            T::NameNamespace => (Some(0xff7b72), false, false),
            T::NameTag => (Some(0x7ee787), false, false),
            T::Literal | T::String | T::StringSymbol | T::Number => (Some(0xa5d6ff), false, false),
            T::Operator => (Some(0xff7b72), true, false),
            T::Comment => (Some(0x8b949e), false, true),
            T::CommentPreproc => (Some(0x8b949e), true, true),
            T::GenericDeleted => (Some(0xffa198), false, false),
            T::GenericInserted => (Some(0x56d364), false, false),
            T::GenericEmph => (Some(0xe6edf3), false, true),
            T::GenericStrong => (Some(0xe6edf3), true, false),
            T::Error => (Some(0xf85149), false, false),
        },
        Mode::Light => match c {
            T::Keyword | T::KeywordConstant | T::KeywordDeclaration | T::KeywordNamespace | T::KeywordPseudo | T::KeywordType => (Some(0xcf222e), false, false),
            T::NameAttribute | T::NameClass | T::Punctuation | T::GenericEmph => (Some(0x1f2328), false, false),
            T::NameBuiltin | T::NameEntity | T::NameFunction => (Some(0x6639ba), false, false),
            T::NameBuiltinPseudo => (Some(0x6a737d), false, false),
            T::NameConstant | T::NameDecorator | T::NameTag | T::Number | T::Operator => (Some(0x0550ae), false, false),
            T::NameLabel => (Some(0x990000), true, false),
            T::NameNamespace => (Some(0x24292e), false, false),
            T::NameVariable => (Some(0x953800), false, false),
            T::String | T::StringEscape | T::StringRegex => (Some(0x0a3069), false, false),
            T::StringSymbol => (Some(0x032f62), false, false),
            T::Comment | T::CommentPreproc => (Some(0x57606a), false, false),
            T::GenericDeleted => (Some(0x82071e), false, false),
            T::GenericInserted => (Some(0x116329), false, false),
            T::Error => (Some(0xf6f8fa), false, false),
            T::Name | T::NameException | T::NameProperty | T::Literal | T::GenericHeading | T::GenericSubheading | T::GenericStrong => (None, false, false),
        },
    }
}

#[derive(Clone, Copy)]
enum LinkRef {
    Target(*const LinkTarget),
    Fragment(*const str),
}

#[derive(Clone, Copy)]
struct RunStyle {
    family: Family,
    bold: bool,
    italic: bool,
    size: f32,
    color: u32,
    shift: f32,
    underline: bool,
    strike: bool,
    link: Option<LinkRef>,
    decor: Decor,
    pad_l: f32,
    pad_r: f32,
    lh_px: f32,
    lh_ratio: f32,
    max_w: f32,
    offset_hint: u32,
}

impl RunStyle {
    fn from_ctx(c: Ctx) -> Self {
        Self {
            family: c.family,
            bold: c.bold,
            italic: c.italic,
            size: c.size,
            color: c.color,
            shift: 0.0,
            underline: false,
            strike: false,
            link: None,
            decor: Decor::None,
            pad_l: 0.0,
            pad_r: 0.0,
            lh_px: c.size * c.lh,
            lh_ratio: c.lh,
            max_w: c.w,
            offset_hint: 0,
        }
    }

    fn font(&self) -> FontSpec {
        FontSpec { family: self.family, bold: self.bold, italic: self.italic }
    }

    fn span_info(&self, ascent: f32, descent: f32, link: Option<u32>) -> SpanInfo {
        SpanInfo {
            color: self.color,
            size: self.size,
            shift: self.shift,
            ascent,
            descent,
            line_height: self.lh_px,
            underline: self.underline,
            strike: self.strike,
            link,
            decor: self.decor,
            pad_l: self.pad_l,
            pad_r: self.pad_r,
            chrome: false,
        }
    }
}

fn code_style(st: RunStyle, _p: Palette) -> RunStyle {
    let mut n = st;
    n.family = Family::Mono;
    n.bold = false;
    n.size = st.size * 0.87;
    n.decor = Decor::Code;
    n.pad_l = 0.38 * n.size + 1.0;
    n.pad_r = n.pad_l;
    n.lh_px = n.size * st.lh_ratio;
    n
}

enum Run {
    Text(ByteSpan, RunStyle),
    /// (what to draw, height above the baseline, depth below it, text offset)
    Atom(AtomDraw, f32, f32, u32),
    Break,
}

impl Run {
    fn start(&self) -> u32 {
        match self {
            Run::Text(s, _) => s.start,
            Run::Atom(_, _, _, o) => *o,
            Run::Break => u32::MAX,
        }
    }
    fn end(&self) -> Option<u32> {
        match self {
            Run::Text(s, _) => Some(s.end),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Prepared {
    glyphs: Vec<Glyph>,
    spans: Vec<SpanInfo>,
    atoms: Vec<AtomDraw>,
    segments: Vec<std::ops::Range<usize>>,
    seg_bytes: Vec<(u32, u32)>,
    strut: (f32, f32, f32),
    byte0: u32,
    byte1: u32,
    total: f32,
}

pub struct LineRange {
    g0: usize,
    g1: usize,
    x_start: f32,
    width: f32,
    byte0: u32,
    byte1: u32,
}

fn seg_end_x(p: &Prepared, r: &std::ops::Range<usize>, upto: usize) -> f32 {
    // The end of the last glyph that is not trailing white space.
    let mut k = upto;
    while k > r.start && p.glyphs[k - 1].flags & FLAG_SPACE != 0 {
        k -= 1;
    }
    if k > r.start { p.glyphs[k - 1].x1 } else { 0.0 }
}

/// Breaks prepared text into lines. `prefer_words` applies `overflow-wrap: break-word` only as a
/// last resort; `false` is code's `anywhere`, which has the same greedy effect here.
pub fn break_lines(p: &Prepared, width: Option<f32>, c: &ChunkContent, _prefer_words: bool) -> Vec<LineRange> {
    let mut out = Vec::new();
    for (si, seg) in p.segments.iter().enumerate() {
        let fallback = p.seg_bytes.get(si).copied();
        if seg.is_empty() {
            let (b0, b1) = fallback.unwrap_or((p.byte0, p.byte0));
            out.push(LineRange { g0: seg.start, g1: seg.start, x_start: 0.0, width: 0.0, byte0: b0, byte1: b1 });
            continue;
        }
        let seg_b0 = fallback.map_or_else(|| p.glyphs[seg.start].byte, |f| f.0);
        let seg_b1 = fallback.map_or_else(
            || {
                let last = &p.glyphs[seg.end - 1];
                if last.flags & FLAG_ATOM != 0 {
                    last.byte
                } else {
                    let b = last.byte as usize;
                    (b + c.text[b..].chars().next().map_or(0, char::len_utf8)) as u32
                }
            },
            |f| f.1,
        );
        let emit = |out: &mut Vec<LineRange>, g0: usize, g1: usize, b1: u32| {
            let x_start = p.glyphs[g0].x0 - p.spans[p.glyphs[g0].span as usize].pad_l.min(if g0 == 0 || p.glyphs[g0 - 1].span != p.glyphs[g0].span { f32::MAX } else { 0.0 });
            let w = seg_end_x(p, &(g0..g1), g1) - x_start + {
                let last = g1 - 1;
                let sp = p.glyphs[last].span;
                if last + 1 == p.glyphs.len() || p.glyphs[last + 1].span != sp {
                    p.spans[sp as usize].pad_r
                } else {
                    0.0
                }
            };
            out.push(LineRange { g0, g1, x_start, width: w.max(0.0), byte0: p.glyphs[g0].byte, byte1: b1 });
        };
        let Some(limit) = width else {
            emit(&mut out, seg.start, seg.end, seg_b1);
            if let Some(l) = out.last_mut() {
                l.byte0 = seg_b0;
            }
            continue;
        };
        // Whole segment fits: the common case, without computing break opportunities.
        let x0 = p.glyphs[seg.start].x0;
        if seg_end_x(p, seg, seg.end) - x0 <= limit {
            emit(&mut out, seg.start, seg.end, seg_b1);
            if let Some(l) = out.last_mut() {
                l.byte0 = seg_b0;
            }
            continue;
        }
        // Allowed break positions, in glyph indices.
        let text = &c.text[seg_b0 as usize..seg_b1 as usize];
        let mut breaks: Vec<usize> = Vec::new();
        {
            let mut gi = seg.start;
            for (b, _) in unicode_linebreak::linebreaks(text) {
                let abs = seg_b0 + b as u32;
                while gi < seg.end && p.glyphs[gi].byte < abs {
                    gi += 1;
                }
                if gi > seg.start && gi <= seg.end && breaks.last() != Some(&gi) {
                    breaks.push(gi);
                }
            }
            for (i, g) in p.glyphs[seg.clone()].iter().enumerate() {
                if g.flags & FLAG_ATOM != 0 {
                    breaks.push(seg.start + i);
                    breaks.push(seg.start + i + 1);
                }
            }
            breaks.sort_unstable();
            breaks.dedup();
        }
        if breaks.last() != Some(&seg.end) {
            breaks.push(seg.end);
        }
        let mut start = seg.start;
        let mut bi = 0;
        let mut first = true;
        while start < seg.end {
            let lx = p.glyphs[start].x0;
            let mut fit: Option<usize> = None;
            while bi < breaks.len() {
                let b = breaks[bi];
                if b <= start {
                    bi += 1;
                    continue;
                }
                if seg_end_x(p, &(start..b), b) - lx <= limit {
                    fit = Some(b);
                    bi += 1;
                } else {
                    break;
                }
            }
            let end = match fit {
                Some(e) => e,
                None => {
                    // `overflow-wrap`: no break fits, so cut at the last glyph that does.
                    let mut e = start + 1;
                    while e < seg.end && p.glyphs[e].x1 - lx <= limit {
                        e += 1;
                    }
                    // Keep a character's combining marks and clusters together.
                    while e < seg.end && p.glyphs[e].byte == p.glyphs[e - 1].byte {
                        e += 1;
                    }
                    e
                }
            };
            let b1 = if end < seg.end { p.glyphs[end].byte } else { seg_b1 };
            emit(&mut out, start, end, b1);
            if first {
                if let Some(l) = out.last_mut() {
                    l.byte0 = seg_b0;
                }
                first = false;
            }
            start = end;
        }
    }
    out
}

/// `text-wrap: balance`: the narrowest width that keeps the same number of lines.
fn balance(p: &Prepared, width: f32, c: &ChunkContent) -> Vec<LineRange> {
    let normal = break_lines(p, Some(width), c, true);
    if normal.len() < 2 || normal.len() > 6 {
        return normal;
    }
    let n = normal.len();
    let (mut lo, mut hi) = (width / n as f32, width);
    let mut best = normal;
    for _ in 0..12 {
        if hi - lo < 1.0 {
            break;
        }
        let mid = (lo + hi) / 2.0;
        let t = break_lines(p, Some(mid), c, true);
        if t.len() <= n {
            hi = mid;
            best = t;
        } else {
            lo = mid;
        }
    }
    best
}

/// (min-content, max-content) of a cell. With wrap on, `overflow-wrap: anywhere` lets the minimum
/// shrink to one character. With wrap off, `white-space: nowrap` makes both the full width.
fn cell_widths(p: &Prepared, wrap: bool) -> (f32, f32) {
    let mut mx = 0.0f32;
    let mut mn = 0.0f32;
    for seg in &p.segments {
        if seg.is_empty() {
            continue;
        }
        let x0 = p.glyphs[seg.start].x0;
        let w = seg_end_x(p, seg, seg.end) - x0;
        let sp = p.glyphs[seg.end - 1].span as usize;
        let total = w + p.spans[p.glyphs[seg.start].span as usize].pad_l + p.spans[sp].pad_r;
        mx = mx.max(total);
        for g in &p.glyphs[seg.clone()] {
            mn = mn.max(g.x1 - g.x0);
        }
    }
    if wrap { (mn, mx) } else { (mx, mx) }
}

// ---- word and line selection ------------------------------------------------------------------

/// The word around `offset` in `text`, for double-click.
pub fn word_at(text: &str, offset: u32) -> ByteSpan {
    let at = (offset as usize).min(text.len());
    // Search a window around the point, ending at line breaks, so a large chunk stays quick.
    let from = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let to = text[at..].find('\n').map_or(text.len(), |i| at + i);
    for (i, w) in text[from..to].split_word_bound_indices() {
        let (s, e) = (from + i, from + i + w.len());
        if at >= s && at < e {
            return ByteSpan::new(s as u32, e as u32);
        }
    }
    ByteSpan::new(at as u32, at as u32)
}

/// The paragraph around `offset`: the whole text box, or one line of code.
pub fn paragraph_at(text: &str, layout: &ChunkLayout, offset: u32) -> ByteSpan {
    match layout.box_at(offset) {
        Some(b) if !b.code => ByteSpan::new(b.byte0, b.byte1),
        Some(b) => {
            let at = offset as usize;
            let from = text[b.byte0 as usize..at].rfind('\n').map_or(b.byte0 as usize, |i| b.byte0 as usize + i + 1);
            let to = text[at..b.byte1 as usize].find('\n').map_or(b.byte1 as usize, |i| at + i);
            ByteSpan::new(from as u32, to as u32)
        }
        None => ByteSpan::new(offset, offset),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_whole;

    /// Every character is half an em wide, ascent 0.8 em, descent 0.2 em.
    struct Fake;
    impl Shaper for Fake {
        fn shape(&mut self, text: &str, _f: FontSpec, size: f32, out: &mut Vec<RawGlyph>) -> f32 {
            let mut x = 0.0;
            for (i, _) in text.char_indices() {
                out.push(RawGlyph { x, byte: i as u32, id: 1, font: 1, emoji: false });
                x += size * 0.5;
            }
            x
        }
        fn metrics(&mut self, _f: FontSpec, size: f32) -> (f32, f32) {
            ((size * 0.8).round(), (size * 0.2).round())
        }
    }

    const OPTS: Options = Options { width: 800.0, padding: 20.0, font_size: 20.0, line_height: 1.5, serif: false, wrap: true, mode: Mode::Dark };

    fn lay(src: &str, o: Options) -> (ChunkContent, ChunkLayout) {
        let w = parse_whole(src, 1 << 30, false);
        let c = w.chunks.into_iter().next().unwrap();
        let l = layout_chunk(&c, &o, &ChunkState::default(), true, true, &mut Fake, &NoAssets);
        (c, l)
    }

    fn boxes(l: &ChunkLayout) -> Vec<&TextBox> {
        l.layers.iter().flat_map(|x| &x.boxes).filter(|b| b.selectable).collect()
    }

    #[test]
    fn margins_collapse_between_paragraph_and_heading() {
        // Paragraph margin-bottom is 1em = 20. H2 margin-top is 1.6 * 29 = 46.4: the larger wins.
        let (_, l) = lay("one\n\n## two\n", OPTS);
        let b = boxes(&l);
        let gap = b[1].rect.y - b[0].rect.bottom();
        assert!((gap - 46.4).abs() < 0.01, "{gap}");
    }

    #[test]
    fn first_block_of_the_document_has_no_top_margin() {
        let (_, l) = lay("# one\n", OPTS);
        assert_eq!(boxes(&l)[0].rect.y, OPTS.padding);
    }

    #[test]
    fn h1_is_centred_and_h2_is_not() {
        let (_, l) = lay("# abcd\n\n## abcd\n", OPTS);
        let b = boxes(&l);
        let w = OPTS.width - 40.0;
        let text_w = 4.0 * (20.0 * 1.9 * 0.5);
        assert!((b[0].lines[0].x - (20.0 + (w - text_w) / 2.0)).abs() < 0.01, "{}", b[0].lines[0].x);
        assert_eq!(b[1].lines[0].x, 20.0);
        // Each has a rule under it.
        assert_eq!(l.layers[0].rects.iter().filter(|r| r.rect.h == 1.0 && r.rect.w == w).count(), 2);
    }

    #[test]
    fn text_wraps_at_the_width_and_lines_map_back() {
        let t = "word ".repeat(40);
        let (c, l) = lay(&t, OPTS);
        let b = boxes(&l)[0];
        assert!(b.lines.len() > 2);
        for line in &b.lines {
            assert!(line.width <= OPTS.width - 40.0 + 0.01, "{}", line.width);
        }
        // Lines partition the text.
        let mut at = b.byte0;
        for line in &b.lines {
            assert_eq!(line.byte0, at);
            at = line.byte1;
        }
        assert_eq!(at as usize, c.text.len());
    }

    #[test]
    fn long_word_breaks_anywhere() {
        let (_, l) = lay(&"x".repeat(500), OPTS);
        let b = boxes(&l)[0];
        assert!(b.lines.len() >= 6);
        assert!(b.lines.iter().all(|x| x.width <= 760.01));
    }

    #[test]
    fn quote_has_a_four_pixel_accent_rule() {
        let (_, l) = lay("> quoted\n", OPTS);
        let r = l.layers[0].rects.iter().find(|r| r.rect.w == 4.0).expect("rule");
        assert_eq!(r.fill, Some(OPTS.palette().accent));
        assert_eq!(boxes(&l)[0].rect.x, 20.0 + 4.0 + 20.0);
    }

    #[test]
    fn table_narrower_than_the_pane_keeps_its_content_width() {
        let (_, l) = lay("| ab | cdef |\n|---|---|\n| 1 | 2 |\n", OPTS);
        assert!(l.areas.is_empty());
        let size = 20.0 * 0.94;
        let cell = |n: f32| n * size * 0.5 + 2.0 * 0.8 * size + 1.0;
        let total = cell(2.0) + cell(4.0) + 1.0;
        let outer = l.layers[0].rects.iter().filter(|r| r.border.is_some()).map(|r| r.rect.x + r.rect.w).fold(0.0f32, f32::max);
        assert!((outer - (20.0 + total)).abs() < 0.01, "{outer} vs {}", 20.0 + total);
    }

    #[test]
    fn wide_table_wraps_inside_the_pane_with_wrap_on() {
        let row = "| aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbb ".repeat(6);
        let src = format!("{row}|\n{}|\n{row}|\n", "|---".repeat(6));
        let (_, l) = lay(&src, OPTS);
        assert!(l.areas.is_empty(), "no horizontal scrolling");
        let right = l.layers[0].rects.iter().map(|r| r.rect.x + r.rect.w).fold(0.0f32, f32::max);
        assert!(right <= 20.0 + 760.0 + 0.5, "{right}");
    }

    #[test]
    fn wide_table_scrolls_with_wrap_off() {
        let row = "| aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbbbbbbbbbbbbb ".repeat(6);
        let src = format!("{row}|\n{}|\n{row}|\n", "|---".repeat(6));
        let mut o = OPTS;
        o.wrap = false;
        let (_, l) = lay(&src, o);
        assert_eq!(l.areas.len(), 1);
        assert!(l.areas[0].content_w > 760.0);
        assert_eq!(l.areas[0].view.w, 760.0);
    }

    #[test]
    fn code_wraps_or_scrolls() {
        let src = format!("```\n{}\n```\n", "x".repeat(300));
        let (_, on) = lay(&src, OPTS);
        assert!(on.areas.is_empty());
        assert!(boxes(&on)[0].lines.len() > 1);
        let mut o = OPTS;
        o.wrap = false;
        let (_, off) = lay(&src, o);
        assert_eq!(off.areas.len(), 1);
        assert_eq!(off.layers[1].boxes[0].lines.len(), 1);
    }

    #[test]
    fn code_tabs_go_to_stops_of_four() {
        let (_, l) = lay("```\na\tb\n```\n", OPTS);
        let b = boxes(&l)[0];
        let cw = 20.0 * 0.87 * 0.5;
        let last = b.glyphs.last().unwrap();
        assert!((last.x0 - 4.0 * cw).abs() < 0.01, "{} vs {}", last.x0, 4.0 * cw);
    }

    #[test]
    fn copy_button_is_a_hit_and_not_text() {
        let (c, l) = lay("```js\nlet a;\n```\n", OPTS);
        assert!(l.hits.iter().any(|h| matches!(&h.kind, HitKind::Copy { text, .. } if c.slice(*text) == "let a;")));
        assert_eq!(boxes(&l).len(), 1);
    }

    #[test]
    fn hit_test_finds_text_across_blocks() {
        let (c, l) = lay("first para\n\nsecond para\n", OPTS);
        let b = boxes(&l);
        let (x, y) = (b[1].lines[0].x + 35.0, b[1].lines[0].top + 2.0);
        let at = l.hit_test(x, y, &|_| 0.0).unwrap();
        assert!(c.text[at as usize..].starts_with("ond para") || c.text[at as usize..].starts_with("nd para"), "{}", &c.text[at as usize..]);
        // A point in the gap selects the nearest box.
        let gap = b[0].rect.bottom() + 1.0;
        assert!(l.hit_test(25.0, gap, &|_| 0.0).is_some());
    }

    #[test]
    fn x_range_covers_a_selection_on_one_line() {
        let (_, l) = lay("abcdef\n", OPTS);
        let b = boxes(&l)[0];
        let (x0, x1) = b.x_range(&b.lines[0], 2, 4).unwrap();
        assert_eq!((x0, x1), (20.0 + 20.0, 20.0 + 40.0));
        assert!(b.x_range(&b.lines[0], 6, 8).is_none());
    }

    #[test]
    fn y_of_finds_any_offset_and_text_at_y_inverts_it() {
        let t = "word ".repeat(400);
        let (_, l) = lay(&t, OPTS);
        let y = l.y_of(1000).unwrap();
        assert!(y > 100.0);
        let (byte, top) = l.text_at_y(y + 1.0).unwrap();
        assert_eq!(top, y);
        assert!(byte <= 1000 && byte + 200 > 1000, "{byte}");
    }

    #[test]
    fn closed_details_hide_their_blocks_and_toggle() {
        let src = "<details>\n<summary>S</summary>\n\nHidden.\n\n</details>\n";
        let (c, l) = lay(src, OPTS);
        assert_eq!(boxes(&l).len(), 1);
        let Block::Details(d) = &c.blocks[0] else { panic!() };
        let mut st = ChunkState::default();
        st.details.insert(d.span.text.start, true);
        let open = layout_chunk(&c, &OPTS, &st, true, true, &mut Fake, &NoAssets);
        assert_eq!(boxes(&open).len(), 2);
        assert!(open.height > l.height);
    }

    #[test]
    fn sorting_moves_rows_but_not_text_offsets() {
        let src = "| n |\n|---|\n| 2 |\n| 10 |\n| 1 |\n";
        let (c, base) = lay(src, OPTS);
        let Block::Table(t) = &c.blocks[0] else { panic!() };
        let mut st = ChunkState::default();
        st.sorts.insert(t.span.text.start, (0, SortDir::Ascending));
        let sorted = layout_chunk(&c, &OPTS, &st, true, true, &mut Fake, &NoAssets);
        let rows = |l: &ChunkLayout| {
            let mut b: Vec<(f32, u32)> = boxes(l).iter().skip(1).map(|b| (b.rect.y, b.byte0)).collect();
            b.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            b.iter().map(|x| x.1).collect::<Vec<_>>()
        };
        let (a, b) = (rows(&base), rows(&sorted));
        assert_ne!(a, b);
        let mut c1 = a.clone();
        let mut c2 = b.clone();
        c1.sort_unstable();
        c2.sort_unstable();
        assert_eq!(c1, c2, "same text, in another order");
    }

    /// An asset arriving late changes height only by what it reserves, and anchoring by text stays
    /// correct: `y_of` the same offset is what the view scrolls back to.
    #[test]
    fn asset_arrival_moves_only_what_follows() {
        struct Sized;
        impl Assets for Sized {
            fn math(&self, _s: &str, _d: bool, _f: f32) -> Option<AssetBox> {
                Some(AssetBox { width: 100.0, height: 400.0, baseline: 300.0, key: 1 })
            }
        }
        let src = "before\n\n$$\nx\n$$\n\nafter\n";
        let w = parse_whole(src, 1 << 30, false);
        let c = &w.chunks[0];
        let st = ChunkState::default();
        let a = layout_chunk(c, &OPTS, &st, true, true, &mut Fake, &NoAssets);
        let b = layout_chunk(c, &OPTS, &st, true, true, &mut Fake, &Sized);
        let before = c.text.find("before").unwrap() as u32;
        let after = c.text.find("after").unwrap() as u32;
        assert_eq!(a.y_of(before), b.y_of(before));
        assert!(b.y_of(after).unwrap() > a.y_of(after).unwrap() + 300.0);
    }

    #[test]
    fn word_and_paragraph_selection() {
        let t = "one two-three four";
        assert_eq!(&t[word_at(t, 5).range()], "two");
        let (c, l) = lay("alpha beta\n\ngamma\n", OPTS);
        let p = paragraph_at(&c.text, &l, 2);
        assert_eq!(&c.text[p.range()], "alpha beta");
        let (c, l) = lay("```\nline one\nline two\n```\n", OPTS);
        let p = paragraph_at(&c.text, &l, 12);
        assert_eq!(&c.text[p.range()], "line two");
    }

    #[test]
    fn empty_code_lines_keep_their_height() {
        let (_, l) = lay("```\na\n\nb\n```\n", OPTS);
        assert_eq!(boxes(&l)[0].lines.len(), 3);
        assert!(boxes(&l)[0].lines[1].height > 0.0);
    }
}
