//! What assistive technology is told about the document: its structure and text, for the part of
//! it near the view. No GPUI types here, so the tree can be tested without a window.
//!
//! The reader paints glyphs, so nothing is exposed unless it is built here. As in the web viewer,
//! only what is mounted is exposed: a 25 MB document costs no more than a short one. Text is in
//! runs, one for each line or for each stretch of a line inside or outside a link, which is what
//! AccessKit builds a Text interface from. A block also has its text as a label, because a screen
//! reader walking the tree rather than the Text interface reads names.

use crate::layout::{ChunkLayout, ChunkState, HitKind, Rect, TextBox};
use crate::model::*;

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Heading(u8),
    Paragraph,
    Quote,
    List,
    ListItem { position: usize, size: usize, checked: Option<bool> },
    Code,
    Button(&'static str),
    Table { rows: usize, columns: usize },
    Row(usize),
    Cell { row: usize, column: usize, header: bool },
    Rule,
    Image,
    Math,
    Diagram,
    Details { open: bool },
    Summary { open: bool },
    Term,
    Definition,
    Footnotes,
    Footnote,
    Group,
    Link(LinkTarget),
    Run,
}

/// One node of the tree. `key` identifies it from one frame to the next within its chunk.
#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub key: (u32, u32),
    /// A block's text, or a run's. For a block it is only what is in the exposed range.
    pub text: String,
    /// In chunk coordinates. A scrolled block's horizontal scroll is not allowed for.
    pub rect: Option<Rect>,
    pub children: Vec<Item>,
    /// For a run: the flat text offset it starts at, and whether the next run is on the same line.
    pub offset: u32,
    pub continues_line: bool,
    /// What a screen reader's "activate" does: the same as clicking in the view.
    pub hit: Option<HitKind>,
}

impl Item {
    fn new(kind: Kind, key: (u32, u32)) -> Self {
        Self { kind, key, text: String::new(), rect: None, children: Vec::new(), offset: key.0, continues_line: false, hit: None }
    }

    /// This item and everything under it, in reading order.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Item::count).sum::<usize>()
    }
}

/// The text boxes of one chunk that are in range, in text order.
struct Boxes<'a> {
    list: Vec<&'a TextBox>,
    y0: f32,
    y1: f32,
}

impl<'a> Boxes<'a> {
    fn new(layout: &'a ChunkLayout, y0: f32, y1: f32) -> Self {
        let mut list: Vec<&TextBox> = layout
            .layers
            .iter()
            .flat_map(|l| &l.boxes)
            .filter(|b| b.selectable && b.rect.y < y1 && b.rect.bottom() > y0)
            .collect();
        list.sort_by_key(|b| b.byte0);
        Self { list, y0, y1 }
    }

    /// The boxes holding any of `span`.
    fn within(&self, span: ByteSpan) -> &[&'a TextBox] {
        let from = self.list.partition_point(|b| b.byte1 <= span.start);
        let to = from + self.list[from..].partition_point(|b| b.byte0 < span.end);
        &self.list[from..to]
    }

    fn range(&self) -> Option<ByteSpan> {
        Some(ByteSpan::new(self.list.first()?.byte0, self.list.iter().map(|b| b.byte1).max()?))
    }
}

struct Walk<'a> {
    c: &'a ChunkContent,
    layout: &'a ChunkLayout,
    st: &'a ChunkState,
    boxes: Boxes<'a>,
    /// Items that may still be added.
    budget: usize,
}

/// The blocks of a chunk that have text between `y0` and `y1`, measured from its top. Blocks with
/// no text (pictures, rules) are included when the blocks around them are. Stops at `budget`
/// items, and returns how many it used.
pub fn outline(c: &ChunkContent, layout: &ChunkLayout, st: &ChunkState, y0: f32, y1: f32, budget: usize) -> (Vec<Item>, usize) {
    let mut w = Walk { c, layout, st, boxes: Boxes::new(layout, y0, y1), budget };
    let Some(range) = w.boxes.range() else { return (Vec::new(), 0) };
    let mut out = Vec::new();
    w.blocks(&c.blocks, range, &mut out);
    (out, budget - w.budget)
}

fn overlaps(s: ByteSpan, range: ByteSpan) -> bool {
    s.start < range.end && s.end > range.start || (s.is_empty() && s.start >= range.start && s.start <= range.end)
}

impl Walk<'_> {
    fn take(&mut self) -> bool {
        if self.budget == 0 {
            return false;
        }
        self.budget -= 1;
        true
    }

    fn blocks(&mut self, blocks: &[Block], range: ByteSpan, out: &mut Vec<Item>) {
        for b in blocks {
            if !overlaps(b.span().text, range) {
                continue;
            }
            if let Some(i) = self.block(b, range) {
                out.push(i);
            }
        }
    }

    fn container(&mut self, kind: Kind, span: Span, blocks: &[Block], range: ByteSpan) -> Option<Item> {
        if !self.take() {
            return None;
        }
        let mut item = Item::new(kind, (span.text.start, 1));
        self.blocks(blocks, range, &mut item.children);
        fit_rect(&mut item);
        Some(item)
    }

    fn block(&mut self, b: &Block, range: ByteSpan) -> Option<Item> {
        match b {
            Block::Heading { level, content, span, .. } => self.text_block(Kind::Heading(*level), content, span.text),
            Block::Paragraph { content, span, .. } => {
                // A paragraph that is only a picture is the picture.
                self.text_block(Kind::Paragraph, content, span.text)
            }
            Block::Quote { blocks, span } => self.container(Kind::Quote, *span, blocks, range),
            Block::Aligned { blocks, span, .. } => self.container(Kind::Group, *span, blocks, range),
            Block::List(l) => {
                if !self.take() {
                    return None;
                }
                let mut list = Item::new(Kind::List, (l.span.text.start, 1));
                let size = l.items.len();
                for (i, it) in l.items.iter().enumerate() {
                    if !overlaps(it.span.text, range) {
                        continue;
                    }
                    let kind = Kind::ListItem { position: i + 1, size, checked: it.checked };
                    if let Some(n) = self.container(kind, it.span, &it.blocks, range) {
                        list.children.push(n);
                    }
                }
                fit_rect(&mut list);
                Some(list)
            }
            Block::Code(cb) => self.code(cb),
            Block::Table(t) => self.table(t, range),
            Block::Rule { span } => {
                self.take().then(|| Item::new(Kind::Rule, (span.text.start, 1)))
            }
            Block::Details(d) => {
                if !self.take() {
                    return None;
                }
                let key = d.span.text.start;
                let open = self.st.details.get(&key).copied().unwrap_or(d.open);
                let mut item = Item::new(Kind::Details { open }, (key, 1));
                if let Some(mut s) = self.text_block(Kind::Summary { open }, &d.summary, d.summary_text) {
                    s.hit = Some(HitKind::Details { key });
                    item.children.push(s);
                }
                self.blocks(&d.blocks, range, &mut item.children);
                fit_rect(&mut item);
                Some(item)
            }
            Block::DefinitionList { items, span } => {
                if !self.take() {
                    return None;
                }
                let mut list = Item::new(Kind::Group, (span.text.start, 1));
                for it in items {
                    if overlaps(it.term_text, range) {
                        if let Some(t) = self.text_block(Kind::Term, &it.term, it.term_text) {
                            list.children.push(t);
                        }
                    }
                    for def in &it.definitions {
                        let Some(first) = def.first() else { continue };
                        let s = Span { source: first.span().source, text: first.span().text };
                        if let Some(n) = self.container(Kind::Definition, s, def, range) {
                            list.children.push(n);
                        }
                    }
                }
                fit_rect(&mut list);
                Some(list)
            }
            Block::Footnotes { items, span } => {
                if !self.take() {
                    return None;
                }
                let mut list = Item::new(Kind::Footnotes, (span.text.start, 1));
                for f in items {
                    let Some(first) = f.blocks.first() else { continue };
                    if let Some(n) = self.container(Kind::Footnote, Span { source: first.span().source, text: first.span().text }, &f.blocks, range) {
                        list.children.push(n);
                    }
                }
                fit_rect(&mut list);
                Some(list)
            }
            Block::Math { src, span, .. } => self.labelled(Kind::Math, span.text, src),
            Block::Mermaid { span, .. } => self.labelled(Kind::Diagram, span.text, "Diagram"),
        }
    }

    fn labelled(&mut self, kind: Kind, span: ByteSpan, label: &str) -> Option<Item> {
        if !self.take() {
            return None;
        }
        let mut item = Item::new(kind, (span.start, 1));
        item.text = bounded(label).to_string();
        Some(item)
    }

    /// A block of running text: its lines in range become runs, split around links.
    fn text_block(&mut self, kind: Kind, content: &[Inline], span: ByteSpan) -> Option<Item> {
        if !self.take() {
            return None;
        }
        let mut item = Item::new(kind, (span.start, 1));
        let mut marks = Vec::new();
        collect_marks(content, &mut marks);
        let images: Vec<String> = marks.iter().filter_map(|m| m.image.clone()).collect();
        if images.len() == 1 && marks.len() == 1 {
            item.kind = Kind::Image;
            item.text = bounded(&images[0]).to_string();
            item.rect = self.boxes.list.iter().find(|b| b.byte0 >= span.start && b.byte1 <= span.end).map(|b| b.rect);
            return Some(item);
        }
        marks.retain(|m| m.link.is_some() && !m.span.is_empty());
        let boxes: Vec<&TextBox> = self.boxes.within(span).to_vec();
        for b in boxes {
            self.box_runs(b, span, &marks, &mut item);
        }
        // Each picture among other text has its alternative text as a node of its own, after them.
        for alt in images {
            if self.take() {
                let n = item.children.len() as u32;
                let mut i = Item::new(Kind::Image, (span.start + n, 5));
                i.text = bounded(&alt).to_string();
                item.children.push(i);
            }
        }
        finish_text(&mut item);
        Some(item)
    }

    fn box_runs(&mut self, b: &TextBox, span: ByteSpan, links: &[Mark], item: &mut Item) {
        let (y0, y1) = (self.boxes.y0, self.boxes.y1);
        let text = &self.c.text;
        for line in b.lines.iter().filter(|l| l.top < y1 && l.top + l.height > y0) {
            let (from, to) = (line.byte0.max(span.start), line.byte1.min(span.end));
            let mut at = from;
            let mut pieces: Vec<(u32, u32, Option<usize>)> = Vec::new();
            for (li, m) in links.iter().enumerate() {
                let (a, z) = (m.span.start.max(from), m.span.end.min(to));
                if a >= z {
                    continue;
                }
                if a > at {
                    pieces.push((at, a, None));
                }
                pieces.push((a, z, Some(li)));
                at = z;
            }
            if at < to {
                pieces.push((at, to, None));
            }
            let height = line.height;
            let n = pieces.len();
            for (pi, (a, z, link)) in pieces.into_iter().enumerate() {
                let Some(t) = text.get(a as usize..z as usize) else { continue };
                if t.is_empty() || self.budget == 0 {
                    continue;
                }
                self.budget -= 1;
                let (x0, x1) = b.x_range(line, a, z).unwrap_or((line.x, line.x + line.width));
                let mut run = Item::new(Kind::Run, (a, 2));
                run.text = t.to_string();
                run.rect = Some(Rect::new(x0, line.top, x1 - x0, height));
                run.continues_line = pi + 1 < n;
                match link {
                    None => item.children.push(run),
                    Some(li) => {
                        let same = matches!(item.children.last(), Some(Item { kind: Kind::Link(_), offset, .. }) if *offset == links[li].span.start);
                        if !same && self.take() {
                            let mut l = Item::new(Kind::Link(links[li].link.clone().unwrap_or(LinkTarget::Fragment(String::new()))), (links[li].span.start, 3));
                            l.offset = links[li].span.start;
                            item.children.push(l);
                        }
                        if let Some(l) = item.children.last_mut().filter(|l| matches!(l.kind, Kind::Link(_))) {
                            l.children.push(run);
                        }
                    }
                }
            }
        }
    }

    fn code(&mut self, cb: &CodeBlock) -> Option<Item> {
        if !self.take() {
            return None;
        }
        let mut item = Item::new(Kind::Code, (cb.span.text.start, 1));
        let boxes: Vec<&TextBox> = self.boxes.within(cb.text).to_vec();
        for b in boxes {
            self.box_runs(b, cb.text, &[], &mut item);
        }
        finish_text(&mut item);
        // Only where the view offers the button too, and only near the block's top.
        let copy = self.layout.hits.iter().find(|h| matches!(&h.kind, HitKind::Copy { key, .. } if *key == cb.span.text.start));
        if let Some(h) = copy.filter(|h| h.rect.y < self.boxes.y1 && h.rect.bottom() > self.boxes.y0) {
            if self.take() {
                let mut b = Item::new(Kind::Button("Copy code"), (cb.span.text.start, 6));
                b.rect = Some(h.rect);
                b.hit = Some(h.kind.clone());
                item.children.push(b);
            }
        }
        Some(item)
    }

    fn table(&mut self, t: &Table, range: ByteSpan) -> Option<Item> {
        if !self.take() {
            return None;
        }
        let columns = t.head.iter().chain(&t.rows).map(|r| r.cells.len()).max().unwrap_or(0);
        let mut table = Item::new(Kind::Table { rows: t.head.len() + t.rows.len(), columns }, (t.span.text.start, 1));
        let heads = t.head.iter().map(|r| (r, true));
        let rows = t.rows.iter().map(|r| (r, false));
        for (ri, (r, header)) in heads.chain(rows).enumerate() {
            let in_range = r.cells.iter().any(|c| overlaps(c.text, range));
            if !in_range {
                continue;
            }
            if !self.take() {
                break;
            }
            let start = r.cells.first().map_or(0, |c| c.text.start);
            let mut row = Item::new(Kind::Row(ri), (start, 4));
            for (ci, cell) in r.cells.iter().enumerate() {
                let kind = Kind::Cell { row: ri, column: ci, header };
                if let Some(mut c) = self.text_block(kind, &cell.content, cell.text) {
                    if header {
                        let key = t.span.text.start;
                        c.hit = self.layout.hits.iter().find(|h| matches!(h.kind, HitKind::Sort { key: k, col } if k == key && col == ci)).map(|h| h.kind.clone());
                    }
                    row.children.push(c);
                }
            }
            fit_rect(&mut row);
            table.children.push(row);
        }
        // A sorted table is read in the order it is shown.
        table.children.sort_by(|a, b| {
            let y = |i: &Item| i.rect.map_or(0.0, |r| r.y);
            y(a).partial_cmp(&y(b)).unwrap_or(std::cmp::Ordering::Equal)
        });
        fit_rect(&mut table);
        Some(table)
    }
}

/// Enough of a label for anything to be read aloud in one go.
fn bounded(s: &str) -> &str {
    let mut end = s.len().min(4096);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A block's label from its runs, its rectangle around them, and a line end on its last run, so
/// that text read across blocks does not run together.
fn finish_text(item: &mut Item) {
    fn gather(i: &Item, out: &mut String) {
        for c in &i.children {
            if c.kind == Kind::Run {
                out.push_str(&c.text);
            } else {
                gather(c, out);
            }
        }
    }
    let mut label = String::new();
    gather(item, &mut label);
    item.text = bounded(label.trim_end()).to_string();
    for c in item.children.iter_mut().filter(|c| matches!(c.kind, Kind::Link(_))) {
        let mut t = String::new();
        gather(c, &mut t);
        c.text = bounded(t.trim_end()).to_string();
    }
    fit_rect(item);
    let mut last = item;
    while let Some(next) = last.children.last_mut() {
        last = next;
    }
    if last.kind == Kind::Run && !last.text.ends_with('\n') {
        last.text.push('\n');
    }
}

fn fit_rect(item: &mut Item) {
    let mut r: Option<Rect> = None;
    for c in &item.children {
        if let Some(q) = c.rect {
            r = Some(match r {
                None => q,
                Some(p) => {
                    let (x0, y0) = (p.x.min(q.x), p.y.min(q.y));
                    let (x1, y1) = ((p.x + p.w).max(q.x + q.w), p.bottom().max(q.bottom()));
                    Rect::new(x0, y0, x1 - x0, y1 - y0)
                }
            });
        }
    }
    item.rect = r;
}

/// A stretch of inline content that matters: a link, or a picture.
struct Mark {
    span: ByteSpan,
    link: Option<LinkTarget>,
    image: Option<String>,
}

fn collect_marks(content: &[Inline], out: &mut Vec<Mark>) {
    for i in content {
        match i {
            Inline::Text(s) | Inline::Code(s) => out.push(Mark { span: *s, link: None, image: None }),
            Inline::Styled { content, .. } => collect_marks(content, out),
            Inline::Link { target, content, .. } => {
                let mut inner = Vec::new();
                collect_marks(content, &mut inner);
                let span = inner.iter().fold(None, |a: Option<ByteSpan>, m| match a {
                    None => Some(m.span),
                    Some(s) => Some(ByteSpan::new(s.start.min(m.span.start), s.end.max(m.span.end))),
                });
                out.push(Mark { span: span.unwrap_or_default(), link: Some(target.clone()), image: None });
            }
            Inline::Image(img) => {
                let alt = if img.alt.is_empty() { "Image".to_string() } else { img.alt.clone() };
                out.push(Mark { span: img.alt_text.unwrap_or_default(), link: None, image: Some(alt) });
            }
            Inline::Math { text, .. } => out.push(Mark { span: *text, link: None, image: None }),
            Inline::FootnoteRef { target, label, .. } | Inline::FootnoteBack { target, label } => {
                out.push(Mark { span: *label, link: Some(LinkTarget::Fragment(target.clone())), image: None });
            }
            Inline::Break => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::*;
    use crate::parse::parse_whole;

    struct Fake;
    impl Shaper for Fake {
        fn shape(&mut self, text: &str, _f: FontSpec, size: f32, out: &mut Vec<RawGlyph>) -> f32 {
            let mut x = 0.0;
            for (i, _) in text.char_indices() {
                out.push(RawGlyph { x, byte: i as u32, id: 1, font: 0, emoji: false });
                x += size / 2.0;
            }
            x
        }
        fn metrics(&mut self, _f: FontSpec, size: f32) -> (f32, f32) {
            (size * 0.8, size * 0.2)
        }
    }

    const OPTS: Options = Options { width: 800.0, padding: 20.0, font_size: 20.0, line_height: 1.5, serif: false, wrap: true, mode: mdv_theme::Mode::Dark };

    fn tree(src: &str, y0: f32, y1: f32, budget: usize) -> (Vec<Item>, usize) {
        let c = parse_whole(src, 1 << 30, false).chunks.into_iter().next().unwrap();
        let l = layout_chunk(&c, &OPTS, &ChunkState::default(), true, true, &mut Fake, &NoAssets);
        outline(&c, &l, &ChunkState::default(), y0, y1, budget)
    }

    fn all(src: &str) -> Vec<Item> {
        tree(src, f32::MIN, f32::MAX, usize::MAX).0
    }

    #[test]
    fn roles_follow_the_document() {
        let t = all("# Title\n\n## Sub\n\nText.\n\n> quoted\n\n- a\n- b\n\n```\ncode\n```\n\n| h |\n|---|\n| c |\n\n---\n");
        let kinds: Vec<&Kind> = t.iter().map(|i| &i.kind).collect();
        assert!(matches!(kinds[0], Kind::Heading(1)));
        assert!(matches!(kinds[1], Kind::Heading(2)));
        assert!(matches!(kinds[2], Kind::Paragraph));
        assert!(matches!(kinds[3], Kind::Quote));
        assert!(matches!(kinds[4], Kind::List));
        assert!(matches!(kinds[5], Kind::Code));
        assert!(matches!(kinds[6], Kind::Table { rows: 2, columns: 1 }));
        assert_eq!(t[0].text, "Title");
        assert_eq!(t[3].children[0].text, "quoted");
        assert!(matches!(t[4].children[1].kind, Kind::ListItem { position: 2, size: 2, .. }));
        let cell = &t[6].children[1].children[0];
        assert!(matches!(cell.kind, Kind::Cell { row: 1, column: 0, header: false }));
        assert_eq!(cell.text, "c");
    }

    #[test]
    fn links_hold_their_own_text() {
        let t = all("See [the site](https://example.org/x) now.\n");
        let p = &t[0];
        assert_eq!(p.text, "See the site now.");
        assert_eq!(p.children.len(), 3);
        assert_eq!(p.children[0].text, "See ");
        let Kind::Link(LinkTarget::Web(u)) = &p.children[1].kind else { panic!("{:?}", p.children[1].kind) };
        assert_eq!(u, "https://example.org/x");
        assert_eq!(p.children[1].children[0].text, "the site");
        assert_eq!(p.children[1].text, "the site");
        assert_eq!(p.children[2].text, " now.\n");
        assert!(p.children[0].continues_line && !p.children[2].continues_line);
    }

    #[test]
    fn a_link_that_wraps_is_one_link() {
        let words = "word ".repeat(100);
        let t = all(&format!("[{words}](https://example.org)\n"));
        let links: Vec<&Item> = t[0].children.iter().filter(|c| matches!(c.kind, Kind::Link(_))).collect();
        assert_eq!(links.len(), 1);
        assert!(links[0].children.len() > 1);
    }

    #[test]
    fn only_the_range_is_exposed() {
        let src: String = (0..200).map(|i| format!("Paragraph {i}.\n\n")).collect();
        let t = tree(&src, 500.0, 900.0, usize::MAX).0;
        assert!(!t.is_empty() && t.len() < 20, "{}", t.len());
        assert!(t[0].text != "Paragraph 0.");
        for i in &t {
            let r = i.rect.unwrap();
            assert!(r.y < 900.0 && r.bottom() > 500.0, "{r:?}");
        }
    }

    #[test]
    fn a_huge_paragraph_only_gives_the_lines_in_range() {
        let t = tree(&"word ".repeat(100_000), 1000.0, 1300.0, usize::MAX).0;
        assert_eq!(t.len(), 1);
        assert!(t[0].children.len() < 15, "{}", t[0].children.len());
        assert!(t[0].text.len() < 4097);
    }

    #[test]
    fn the_budget_is_a_limit() {
        let src: String = (0..500).map(|i| format!("Paragraph {i}.\n\n")).collect();
        let (t, used) = tree(&src, f32::MIN, f32::MAX, 100);
        assert_eq!(used, 100);
        assert_eq!(t.iter().map(Item::count).sum::<usize>(), 100);
    }

    #[test]
    fn a_large_table_gives_only_rows_in_range() {
        let src = format!("| a | b |\n|---|---|\n{}", "| 1 | 2 |\n".repeat(20_000));
        let t = tree(&src, 100_000.0, 100_500.0, usize::MAX).0;
        let n = t[0].children.len();
        assert!(n > 2 && n < 40, "{n}");
        assert!(matches!(t[0].kind, Kind::Table { rows: 20_001, columns: 2 }));
    }

    #[test]
    fn a_picture_alone_is_an_image() {
        let t = all("![A cat](cat.png)\n");
        assert_eq!(t[0].kind, Kind::Image);
        assert_eq!(t[0].text, "A cat");
    }

    #[test]
    fn task_items_say_if_they_are_done() {
        let t = all("- [x] done\n- [ ] not\n");
        assert!(matches!(t[0].children[0].kind, Kind::ListItem { checked: Some(true), .. }));
        assert!(matches!(t[0].children[1].kind, Kind::ListItem { checked: Some(false), .. }));
    }

    #[test]
    fn code_offers_its_copy_button_and_a_table_header_its_sort() {
        let t = all("```\nlet a;\n```\n\n| h |\n|---|\n| c |\n");
        let b = t[0].children.last().unwrap();
        assert_eq!(b.kind, Kind::Button("Copy code"));
        assert!(matches!(b.hit, Some(HitKind::Copy { .. })));
        let head = &t[1].children[0].children[0];
        assert!(matches!(head.kind, Kind::Cell { header: true, .. }));
        assert!(matches!(head.hit, Some(HitKind::Sort { col: 0, .. })));
    }

    #[test]
    fn closed_details_show_their_summary_only() {
        let t = all("<details>\n<summary>More</summary>\n\nHidden text.\n\n</details>\n");
        assert!(matches!(t[0].kind, Kind::Details { open: false }));
        assert_eq!(t[0].children.len(), 1);
        assert_eq!(t[0].children[0].text, "More");
        assert!(matches!(t[0].children[0].hit, Some(HitKind::Details { .. })));
    }
}
