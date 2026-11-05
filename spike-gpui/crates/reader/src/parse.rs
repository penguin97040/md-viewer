//! Markdown to document model. Events from pulldown-cmark drive a small builder; raw HTML is
//! mapped through `html.rs`. Chosen behaviour follows `internal/doc`: what the web reader shows
//! after goldmark and the sanitiser is the specification.

use std::collections::HashMap;
use std::ops::Range;

use pulldown_cmark::{BrokenLink, CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::autolink;
use crate::highlight;
use crate::html::{self, Tok};
use crate::model::*;
use crate::paths;
use crate::slug::Slugger;

pub fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_MATH
        | Options::ENABLE_DEFINITION_LIST
}

/// Link reference definitions collected from a whole document, keyed by normalised label.
#[derive(Default, Debug)]
pub struct RefMap(pub HashMap<String, (String, String)>);

pub fn normalise_label(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// What the first parse of one section keeps.
#[derive(Default, Debug)]
pub struct SectionScan {
    pub headings: Vec<ScannedHeading>,
    pub refs: Vec<(String, String, String)>,
}

#[derive(Debug)]
pub struct ScannedHeading {
    pub level: u8,
    pub text: String,
    pub explicit_id: Option<String>,
}

fn level_u8(l: HeadingLevel) -> u8 {
    match l {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// The first, parallel pass over one section: headings and link references only. Nothing about
/// the section is retained afterwards except these, so the AST never outlives this call.
pub fn scan_section(src: &str) -> SectionScan {
    let parser = Parser::new_ext(src, options());
    let mut scan = SectionScan {
        refs: parser
            .reference_definitions()
            .iter()
            .map(|(label, def)| (normalise_label(label), def.dest.to_string(), def.title.as_ref().map_or(String::new(), |t| t.to_string())))
            .collect(),
        ..Default::default()
    };
    let mut hidden = 0; // inside a footnote or definition: not in the outline (`walkHeadings`)
    let mut cur: Option<ScannedHeading> = None;
    for (ev, range) in parser.into_offset_iter() {
        match ev {
            Event::Start(Tag::Heading { level, id, .. }) => {
                if hidden == 0 {
                    cur = Some(ScannedHeading { level: level_u8(level), text: String::new(), explicit_id: id.map(|i| i.to_string()) });
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(mut h) = cur.take() {
                    h.text = h.text.trim().to_string();
                    scan.headings.push(h);
                }
            }
            Event::Start(Tag::FootnoteDefinition(_) | Tag::DefinitionListDefinition) => hidden += 1,
            Event::End(TagEnd::FootnoteDefinition | TagEnd::DefinitionListDefinition) => hidden -= 1,
            Event::Text(t) | Event::Code(t) => {
                if let Some(h) = cur.as_mut() {
                    h.text.push_str(&t);
                }
            }
            Event::InlineMath(_) | Event::DisplayMath(_) => {
                if let Some(h) = cur.as_mut() {
                    h.text.push_str(&src[range]);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(h) = cur.as_mut() {
                    h.text.push(' ');
                }
            }
            _ => {}
        }
    }
    scan
}

/// How heading ids are obtained.
enum Ids<'a> {
    /// A whole document in one pass.
    Slug(&'a mut Slugger),
    /// A section re-parsed on demand: ids were assigned in the first pass, in order.
    Assigned(&'a [String]),
}

/// One parsed section of a large file.
pub fn parse_section(src: &str, base: u32, index: u32, refs: &RefMap, ids: &[String], colours: bool) -> ChunkContent {
    let mut cb = |link: BrokenLink<'_>| -> Option<(CowStr<'_>, CowStr<'_>)> {
        refs.0.get(&normalise_label(&link.reference)).map(|(d, t)| (CowStr::from(d.clone()), CowStr::from(t.clone())))
    };
    let parser = Parser::new_with_broken_link_callback(src, options(), Some(&mut cb));
    let mut b = Builder::new(src, base, index, Ids::Assigned(ids), colours);
    b.run(parser.into_offset_iter(), None);
    b.finish_chunk().0
}

/// The result of parsing a small file in one pass.
pub struct WholeParse {
    pub chunks: Vec<ChunkContent>,
    pub ranges: Vec<(u32, u32)>,
    pub headings: Vec<HeadingInfo>,
}

/// A whole small document, its top-level blocks grouped into chunks of about `target` bytes.
pub fn parse_whole(src: &str, target: usize, colours: bool) -> WholeParse {
    let mut slugs = Slugger::new();
    let parser = Parser::new_ext(src, options());
    let mut b = Builder::new(src, 0, 0, Ids::Slug(&mut slugs), colours);
    let mut out = Vec::new();
    b.run(parser.into_offset_iter(), Some((target, &mut out)));
    let (last, headings) = b.finish_chunk();
    let cut = out.last().map_or(0, |(_, e): &(ChunkContent, u32)| *e);
    out.push((last, src.len() as u32));
    let mut ranges = Vec::with_capacity(out.len());
    let mut chunks = Vec::with_capacity(out.len());
    let mut start = 0;
    for (c, end) in out {
        ranges.push((start, end));
        start = end;
        chunks.push(c);
    }
    let _ = cut;
    WholeParse { chunks, ranges, headings }
}

// ---- the builder ------------------------------------------------------------------------------

enum Kind {
    Root,
    Quote,
    List { ordered: bool, start: u32, raw: bool },
    Item { checked: Option<bool> },
    Details { open: bool, summary: Vec<Inline>, summary_text: ByteSpan },
    /// A raw HTML container such as `<div align="center">`.
    Html { tag: String, align: Align },
    DefList { raw: bool },
    Def,
    Table { raw: bool, aligns: Vec<Align>, head: Vec<TableRow>, rows: Vec<TableRow>, in_head: bool, row: Option<TableRow> },
    /// A footnote definition being replayed.
    Note,
}

struct Frame {
    kind: Kind,
    blocks: Vec<Block>,
    items: Vec<ListItem>,
    defs: Vec<DefinitionItem>,
    src: u32,
    text: u32,
}

#[derive(PartialEq)]
enum InlineKind {
    Para { tight: bool },
    Heading { level: u8, explicit: Option<String>, raw: bool },
    Summary,
    Cell { head: bool, align: Align },
    Term,
}

enum Wrap {
    Root,
    Style(InlineStyle),
    Link(LinkTarget, Option<String>),
    /// A tag that changes nothing, or a link whose target was refused. Keeps its content.
    Plain,
}

struct Open {
    wrap: Wrap,
    /// The raw HTML tag name that closes this level; `None` for markdown syntax.
    tag: Option<String>,
    content: Vec<Inline>,
}

struct InlineCtx {
    kind: InlineKind,
    stack: Vec<Open>,
    text: u32,
    src: u32,
    display: Option<String>,
    plain: String,
}

struct Builder<'a, 's> {
    src: &'s str,
    base: u32,
    index: u32,
    ids: Ids<'a>,
    colours: bool,
    next_id: usize,
    text: String,
    no_find: Vec<ByteSpan>,
    anchors: Vec<Anchor>,
    sep: Option<&'static str>,
    ws: bool,
    frames: Vec<Frame>,
    inline: Option<InlineCtx>,
    buffered: String,
    link_depth: u32,
    image: Option<(Image, String)>,
    code: Option<(Option<String>, String, u32)>,
    pre: Option<String>,
    skip: Option<(String, i32)>,
    hidden: u32,
    defs: HashMap<String, Vec<(Event<'s>, Range<usize>)>>,
    note_order: Vec<String>,
    note_numbers: HashMap<String, (u32, u32)>,
    headings: Vec<HeadingInfo>,
}

const fn sep_rank(s: &str) -> u8 {
    match s.len() {
        2 => 2,
        _ => {
            if s.as_bytes()[0] == b'\n' {
                1
            } else {
                0
            }
        }
    }
}

impl<'a, 's> Builder<'a, 's> {
    fn new(src: &'s str, base: u32, index: u32, ids: Ids<'a>, colours: bool) -> Self {
        Self {
            src,
            base,
            index,
            ids,
            colours,
            next_id: 0,
            text: String::with_capacity(src.len()),
            no_find: Vec::new(),
            anchors: Vec::new(),
            sep: None,
            ws: true,
            frames: vec![Frame { kind: Kind::Root, blocks: Vec::new(), items: Vec::new(), defs: Vec::new(), src: base, text: 0 }],
            inline: None,
            buffered: String::new(),
            link_depth: 0,
            image: None,
            code: None,
            pre: None,
            skip: None,
            hidden: 0,
            defs: HashMap::new(),
            note_order: Vec::new(),
            note_numbers: HashMap::new(),
            headings: Vec::new(),
        }
    }

    // ---- flat text --------------------------------------------------------------------------

    fn want_sep(&mut self, s: &'static str) {
        match self.sep {
            Some(cur) if sep_rank(cur) >= sep_rank(s) => {}
            _ => self.sep = Some(s),
        }
    }

    /// Writes any pending separator and returns the offset where the next text will go.
    fn mark(&mut self) -> u32 {
        if let Some(s) = self.sep.take() {
            if !self.text.is_empty() && !self.text.ends_with('\n') {
                self.text.push_str(s);
            }
            self.ws = true;
        }
        self.text.len() as u32
    }

    /// Appends text collapsing white space as `white-space: normal` does.
    fn push_collapsed(&mut self, t: &str) -> ByteSpan {
        let start = self.mark();
        for c in t.chars() {
            if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c') {
                if !self.ws {
                    self.text.push(' ');
                    self.ws = true;
                }
            } else {
                self.text.push(c);
                self.ws = false;
            }
        }
        ByteSpan::new(start, self.text.len() as u32)
    }

    fn push_raw(&mut self, t: &str) -> ByteSpan {
        let start = self.mark();
        self.text.push_str(t);
        self.ws = t.ends_with(char::is_whitespace);
        ByteSpan::new(start, self.text.len() as u32)
    }

    fn push_no_find(&mut self, t: &str) -> ByteSpan {
        let span = self.push_raw(t);
        self.ws = false;
        if !span.is_empty() {
            self.no_find.push(span);
        }
        span
    }

    // ---- frames -----------------------------------------------------------------------------

    fn push_frame(&mut self, kind: Kind, src: u32) {
        let text = self.text.len() as u32;
        self.frames.push(Frame { kind, blocks: Vec::new(), items: Vec::new(), defs: Vec::new(), src, text });
    }

    fn add_block(&mut self, b: Block) {
        self.frames.last_mut().expect("root frame").blocks.push(b);
    }

    fn span_from(&self, src_start: u32, text_start: u32, src_end: u32) -> Span {
        Span { source: ByteSpan::new(src_start, src_end), text: ByteSpan::new(text_start, self.text.len() as u32) }
    }

    /// Pops the top frame, attaching what it built to its parent.
    fn pop_frame(&mut self, end: u32) {
        self.close_inline(end);
        let f = self.frames.pop().expect("frame");
        let span = self.span_from(f.src, f.text, end);
        match f.kind {
            Kind::Root => unreachable!("the root frame is never popped"),
            Kind::Quote => self.add_block(Block::Quote { blocks: f.blocks, span }),
            Kind::List { ordered, start, .. } => {
                let tight = f.items.iter().all(|i| i.blocks.iter().all(|b| !matches!(b, Block::Paragraph { tight: false, .. })));
                self.add_block(Block::List(List { ordered, start, tight, items: f.items, span }));
            }
            Kind::Item { checked } => {
                if let Some(parent) = self.frames.last_mut() {
                    parent.items.push(ListItem { checked, blocks: f.blocks, span });
                }
            }
            Kind::Details { open, summary, summary_text } => {
                self.add_block(Block::Details(Details { open, summary, summary_text, blocks: f.blocks, span }));
            }
            Kind::Html { align, .. } => match align {
                Align::Default => {
                    let parent = self.frames.last_mut().expect("root frame");
                    parent.blocks.extend(f.blocks);
                }
                _ => self.add_block(Block::Aligned { align, blocks: f.blocks, span }),
            },
            Kind::DefList { .. } => self.add_block(Block::DefinitionList { items: f.defs, span }),
            Kind::Def => {
                if let Some(item) = self.frames.last_mut().and_then(|p| p.defs.last_mut()) {
                    item.definitions.push(f.blocks);
                }
            }
            Kind::Table { aligns, head, mut rows, row, .. } => {
                if let Some(r) = row {
                    rows.push(r);
                }
                self.add_block(Block::Table(Table { aligns, head, rows, span }));
            }
            Kind::Note => self.add_block(Block::Quote { blocks: f.blocks, span }),
        }
    }

    fn pop_until(&mut self, pred: impl Fn(&Kind) -> bool, end: u32) -> bool {
        if !self.frames.iter().any(|f| pred(&f.kind)) {
            return false;
        }
        while let Some(f) = self.frames.last() {
            let done = pred(&f.kind);
            if matches!(f.kind, Kind::Root) {
                return false;
            }
            self.pop_frame(end);
            if done {
                return true;
            }
        }
        false
    }

    // ---- inline context ---------------------------------------------------------------------

    fn open_inline(&mut self, kind: InlineKind, src: u32) {
        self.flush_text();
        self.mark();
        self.inline = Some(InlineCtx {
            kind,
            stack: vec![Open { wrap: Wrap::Root, tag: None, content: Vec::new() }],
            text: self.text.len() as u32,
            src,
            display: None,
            plain: String::new(),
        });
        self.ws = true;
    }

    /// Text outside any explicit block (a tight list item, or text inside `<div>`) becomes a
    /// paragraph without margins.
    fn ensure_inline(&mut self, src: u32) {
        if self.inline.is_none() {
            if !matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Item { .. })) {
                self.want_sep(SEP_BLOCK);
            }
            self.open_inline(InlineKind::Para { tight: true }, src);
        }
    }

    fn close_inline(&mut self, end: u32) {
        self.flush_text();
        let Some(mut ctx) = self.inline.take() else { return };
        // Unclosed raw tags close at the end of their block.
        while ctx.stack.len() > 1 {
            let top = ctx.stack.pop().expect("level");
            let inline = finish_wrap(top);
            ctx.stack.last_mut().expect("root").content.extend(inline);
        }
        let mut content = ctx.stack.pop().expect("root").content;
        // A trailing space belongs to nothing: white space at the end of a line box is dropped.
        if self.text.ends_with(' ') && self.text.len() as u32 > ctx.text {
            self.text.pop();
            shrink_to(&mut content, self.text.len() as u32);
        }
        self.ws = true;
        let span = Span { source: ByteSpan::new(ctx.src, end), text: ByteSpan::new(ctx.text, self.text.len() as u32) };
        match ctx.kind {
            InlineKind::Para { tight } => {
                if let (Some(src), true) = (ctx.display, content.len() == 1) {
                    self.add_block(Block::Math { src, display: true, span });
                } else if !content.is_empty() {
                    self.add_block(Block::Paragraph { content, tight, span });
                }
                self.want_sep(if tight { SEP_LINE } else { SEP_BLOCK });
            }
            InlineKind::Heading { level, explicit, raw } => {
                let plain = ctx.plain.trim().to_string();
                let id = if raw {
                    explicit.unwrap_or_default()
                } else if self.hidden > 0 {
                    explicit.unwrap_or_default()
                } else {
                    let i = self.next_id;
                    self.next_id += 1;
                    match &mut self.ids {
                        Ids::Assigned(ids) => ids.get(i).cloned().unwrap_or_default(),
                        Ids::Slug(slugs) => match explicit {
                            Some(e) => {
                                slugs.reserve(&e);
                                e
                            }
                            None => slugs.slug(&plain),
                        },
                    }
                };
                if !raw && self.hidden == 0 {
                    self.headings.push(HeadingInfo { level, text: plain, id: id.clone(), chunk: self.index });
                }
                if !id.is_empty() {
                    self.anchors.push(Anchor { id: id.clone(), offset: ctx.text });
                }
                self.add_block(Block::Heading { level, id, content, span });
                self.want_sep(SEP_BLOCK);
            }
            InlineKind::Summary => {
                if let Some(Frame { kind: Kind::Details { summary, summary_text, .. }, .. }) = self.frames.iter_mut().rev().find(|f| matches!(f.kind, Kind::Details { .. })) {
                    *summary = content;
                    *summary_text = span.text;
                }
                self.want_sep(SEP_BLOCK);
            }
            InlineKind::Cell { align, .. } => {
                if let Some(Frame { kind: Kind::Table { row, .. }, .. }) = self.frames.last_mut() {
                    row.get_or_insert_with(TableRow::default).cells.push(TableCell { content, text: span.text, align });
                }
                self.want_sep(SEP_CELL);
            }
            InlineKind::Term => {
                if let Some(f) = self.frames.last_mut() {
                    f.defs.push(DefinitionItem { term: content, term_text: span.text, definitions: Vec::new() });
                }
                self.want_sep(SEP_LINE);
            }
        }
    }

    fn add_inline(&mut self, i: Inline) {
        if let Some(ctx) = self.inline.as_mut() {
            ctx.stack.last_mut().expect("root").content.push(i);
        }
    }

    fn add_text_span(&mut self, span: ByteSpan) {
        if span.is_empty() {
            return;
        }
        let ctx = self.inline.as_mut().expect("inline context");
        let content = &mut ctx.stack.last_mut().expect("root").content;
        if let Some(Inline::Text(last)) = content.last_mut() {
            if last.end == span.start {
                last.end = span.end;
                return;
            }
        }
        content.push(Inline::Text(span));
    }

    fn open_wrap(&mut self, wrap: Wrap, tag: Option<String>) {
        self.flush_text();
        if let Wrap::Link(..) = wrap {
            self.link_depth += 1;
        }
        if let Some(ctx) = self.inline.as_mut() {
            ctx.stack.push(Open { wrap, tag, content: Vec::new() });
        }
    }

    fn close_top_wrap(&mut self) {
        let Some(ctx) = self.inline.as_mut() else { return };
        if ctx.stack.len() <= 1 {
            return;
        }
        let top = ctx.stack.pop().expect("level");
        if let Wrap::Link(..) = top.wrap {
            self.link_depth -= 1;
        }
        let items = finish_wrap(top);
        let ctx = self.inline.as_mut().expect("inline context");
        ctx.stack.last_mut().expect("root").content.extend(items);
    }

    /// Closes the markdown syntax level on top, skipping stray raw tags opened inside it.
    fn close_syntax(&mut self) {
        self.flush_text();
        loop {
            let Some(ctx) = self.inline.as_ref() else { return };
            let raw = ctx.stack.last().is_some_and(|o| o.tag.is_some());
            if ctx.stack.len() <= 1 {
                return;
            }
            self.close_top_wrap();
            if !raw {
                return;
            }
        }
    }

    fn close_raw(&mut self, name: &str) {
        self.flush_text();
        let Some(ctx) = self.inline.as_ref() else { return };
        if !ctx.stack.iter().any(|o| o.tag.as_deref() == Some(name)) {
            return;
        }
        loop {
            let Some(ctx) = self.inline.as_ref() else { return };
            if ctx.stack.len() <= 1 {
                return;
            }
            let done = ctx.stack.last().is_some_and(|o| o.tag.as_deref() == Some(name));
            self.close_top_wrap();
            if done {
                return;
            }
        }
    }

    /// Writes buffered markdown text, finding extended autolinks unless already inside a link.
    fn flush_text(&mut self) {
        if self.buffered.is_empty() {
            return;
        }
        let t = std::mem::take(&mut self.buffered);
        if let Some(ctx) = self.inline.as_mut() {
            if matches!(ctx.kind, InlineKind::Heading { .. }) {
                ctx.plain.push_str(&t);
            }
        }
        let found = if self.link_depth == 0 { autolink::find(&t) } else { Vec::new() };
        let mut at = 0;
        for f in found {
            if f.start > at {
                let s = self.push_collapsed(&t[at..f.start]);
                self.add_text_span(s);
            }
            let s = self.push_collapsed(&t[f.start..f.end]);
            if let Some(target) = classify_link(&f.url) {
                self.add_inline(Inline::Link { target, title: None, content: vec![Inline::Text(s)] });
            } else {
                self.add_text_span(s);
            }
            at = f.end;
        }
        if at < t.len() {
            let s = self.push_collapsed(&t[at..]);
            self.add_text_span(s);
        }
    }

    fn text_event(&mut self, t: &str, src: u32) {
        if let Some((_, buf, _)) = self.code.as_mut() {
            buf.push_str(t);
            return;
        }
        if let Some((_, alt)) = self.image.as_mut() {
            alt.push_str(t);
            return;
        }
        self.ensure_inline(src);
        self.buffered.push_str(t);
    }

    /// Text from raw HTML, which is already entity-decoded by the caller and never linkified.
    fn raw_text(&mut self, t: &str, src: u32) {
        if let Some(buf) = self.pre.as_mut() {
            buf.push_str(&html::decode_entities(t));
            return;
        }
        let decoded = html::decode_entities(t);
        if self.inline.is_none() {
            if decoded.chars().all(char::is_whitespace) {
                return;
            }
            // Text directly in a table but outside a cell is not shown.
            if matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Table { .. } | Kind::List { .. } | Kind::DefList { .. })) {
                return;
            }
        }
        self.ensure_inline(src);
        self.flush_text();
        let s = self.push_collapsed(&decoded);
        self.add_text_span(s);
    }

    // ---- events -----------------------------------------------------------------------------

    fn run<I>(&mut self, events: I, mut cuts: Option<(usize, &mut Vec<(ChunkContent, u32)>)>)
    where
        I: Iterator<Item = (Event<'s>, Range<usize>)>,
    {
        let mut capture: Option<(String, Vec<(Event<'s>, Range<usize>)>, u32)> = None;
        let mut chunk_start = 0usize;
        for (ev, range) in events {
            // Footnote definitions go at the end of the document, numbered by first use, so they
            // are set aside and replayed then. This keeps the flat text in reading order.
            if let Some((_, buf, depth)) = capture.as_mut() {
                match &ev {
                    Event::Start(_) => *depth += 1,
                    Event::End(_) => {
                        *depth -= 1;
                    }
                    _ => {}
                }
                if *depth == 0 {
                    let (label, buf, _) = capture.take().expect("capture");
                    self.defs.entry(label).or_insert(buf);
                } else {
                    buf.push((ev, range));
                }
                continue;
            }
            if let Event::Start(Tag::FootnoteDefinition(label)) = &ev {
                capture = Some((label.to_string(), Vec::new(), 1));
                continue;
            }
            // A chunk ends before a top-level block once it is big enough.
            if let Some((target, out)) = cuts.as_mut() {
                let top_start = self.frames.len() == 1 && self.inline.is_none() && self.skip.is_none() && self.code.is_none() && self.pre.is_none() && matches!(ev, Event::Start(_) | Event::Rule);
                if top_start && range.start - chunk_start >= *target {
                    let idx = self.index + 1;
                    let (done, heads) = self.finish_section_for_chunk();
                    self.headings = heads;
                    out.push((done, range.start as u32));
                    self.index = idx;
                    chunk_start = range.start;
                }
            }
            self.event(ev, range);
        }
        self.flush_text();
    }

    /// Ends the current chunk and starts a fresh flat text, keeping heading and footnote state.
    fn finish_section_for_chunk(&mut self) -> (ChunkContent, Vec<HeadingInfo>) {
        let blocks = std::mem::take(&mut self.frames[0].blocks);
        let c = ChunkContent {
            index: self.index,
            text: std::mem::take(&mut self.text),
            blocks,
            no_find: std::mem::take(&mut self.no_find),
            anchors: std::mem::take(&mut self.anchors),
        };
        self.sep = None;
        self.ws = true;
        self.frames[0].text = 0;
        (c, std::mem::take(&mut self.headings))
    }

    fn event(&mut self, ev: Event<'s>, range: Range<usize>) {
        let (start, end) = (self.base + range.start as u32, self.base + range.end as u32);
        if self.skip.is_some() {
            self.skipped_event(&ev, end);
            return;
        }
        match ev {
            Event::Start(tag) => self.start(tag, start, end, &range),
            Event::End(tag) => self.end(tag, start, end),
            Event::Text(t) => self.text_event(&t, start),
            Event::Code(t) => {
                if let Some((_, alt)) = self.image.as_mut() {
                    alt.push_str(&t);
                    return;
                }
                self.ensure_inline(start);
                self.flush_text();
                if let Some(ctx) = self.inline.as_mut() {
                    if matches!(ctx.kind, InlineKind::Heading { .. }) {
                        ctx.plain.push_str(&t);
                    }
                }
                let s = self.push_collapsed(&t);
                self.add_inline(Inline::Code(s));
            }
            Event::InlineMath(t) | Event::DisplayMath(t) => {
                let display = range.end - range.start >= 4 && self.src[range.clone()].starts_with("$$");
                self.ensure_inline(start);
                self.flush_text();
                let only = self.inline.as_ref().is_some_and(|c| c.stack.len() == 1 && c.stack[0].content.is_empty());
                let raw = self.src[range].to_string();
                if let Some(ctx) = self.inline.as_mut() {
                    if matches!(ctx.kind, InlineKind::Heading { .. }) {
                        ctx.plain.push_str(&raw);
                    }
                    if display && only {
                        ctx.display = Some(t.trim().to_string());
                    } else {
                        ctx.display = None;
                    }
                }
                let s = self.push_no_find(t.trim());
                self.add_inline(Inline::Math { src: t.trim().to_string(), text: s });
            }
            Event::Html(t) => self.html(&t, start, end, true),
            Event::InlineHtml(t) => self.html(&t, start, end, false),
            Event::FootnoteReference(label) => self.note_ref(&label, start),
            Event::SoftBreak => {
                if let Some((_, alt)) = self.image.as_mut() {
                    alt.push(' ');
                    return;
                }
                self.text_event(" ", start);
            }
            Event::HardBreak => {
                self.ensure_inline(start);
                self.flush_text();
                self.mark();
                if let Some(ctx) = self.inline.as_mut() {
                    if matches!(ctx.kind, InlineKind::Heading { .. }) {
                        ctx.plain.push(' ');
                    }
                }
                self.text.push('\n');
                self.ws = true;
                self.add_inline(Inline::Break);
            }
            Event::Rule => {
                self.close_inline(start);
                let span = self.span_from(start, self.text.len() as u32, end);
                self.add_block(Block::Rule { span });
                self.want_sep(SEP_BLOCK);
            }
            Event::TaskListMarker(on) => {
                if let Some(Frame { kind: Kind::Item { checked }, .. }) = self.frames.last_mut() {
                    *checked = Some(on);
                }
            }
        }
    }

    /// While a dropped element such as `<script>` is open nothing is shown, but markdown
    /// headings in it still use up their pre-assigned ids so later headings keep theirs.
    fn skipped_event(&mut self, ev: &Event<'s>, _end: u32) {
        match ev {
            Event::Html(t) | Event::InlineHtml(t) => {
                let (name, mut depth) = self.skip.clone().expect("skip");
                for tok in html::scan(t) {
                    match tok {
                        Tok::Open { name: n, self_closing: false, .. } if n == name => depth += 1,
                        Tok::Close { name: n } if n == name => depth -= 1,
                        _ => {}
                    }
                    if depth == 0 {
                        break;
                    }
                }
                self.skip = if depth <= 0 { None } else { Some((name, depth)) };
            }
            Event::End(TagEnd::Heading(_)) if self.hidden == 0 => self.next_id += 1,
            _ => {}
        }
    }

    fn start(&mut self, tag: Tag<'s>, start: u32, _end: u32, _range: &Range<usize>) {
        match tag {
            Tag::Paragraph => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.open_inline(InlineKind::Para { tight: false }, start);
            }
            Tag::Heading { level, id, .. } => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.open_inline(InlineKind::Heading { level: level_u8(level), explicit: id.map(|i| i.to_string()), raw: false }, start);
            }
            Tag::BlockQuote(_) => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.push_frame(Kind::Quote, start);
            }
            Tag::CodeBlock(kind) => {
                self.close_inline(start);
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => fence_lang(&info),
                    CodeBlockKind::Indented => None,
                };
                self.code = Some((lang, String::new(), start));
            }
            Tag::HtmlBlock | Tag::MetadataBlock(_) => self.close_inline(start),
            Tag::List(first) => {
                let in_item = matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Item { .. }));
                self.close_inline(start);
                self.want_sep(if in_item { SEP_LINE } else { SEP_BLOCK });
                let start_at = first.map_or(1, |n| n.min(u32::MAX as u64) as u32);
                self.push_frame(Kind::List { ordered: first.is_some(), start: start_at, raw: false }, start);
            }
            Tag::Item => {
                self.close_inline(start);
                self.want_sep(SEP_LINE);
                self.mark();
                self.push_frame(Kind::Item { checked: None }, start);
            }
            Tag::FootnoteDefinition(_) => {}
            Tag::DefinitionList => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.push_frame(Kind::DefList { raw: false }, start);
            }
            Tag::DefinitionListTitle => {
                self.want_sep(SEP_LINE);
                self.open_inline(InlineKind::Term, start);
            }
            Tag::DefinitionListDefinition => {
                self.close_inline(start);
                self.want_sep(SEP_LINE);
                self.hidden += 1;
                self.push_frame(Kind::Def, start);
            }
            Tag::Table(aligns) => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.mark();
                let aligns = aligns
                    .iter()
                    .map(|a| match a {
                        pulldown_cmark::Alignment::None => Align::Default,
                        pulldown_cmark::Alignment::Left => Align::Left,
                        pulldown_cmark::Alignment::Center => Align::Centre,
                        pulldown_cmark::Alignment::Right => Align::Right,
                    })
                    .collect();
                self.push_frame(Kind::Table { raw: false, aligns, head: Vec::new(), rows: Vec::new(), in_head: false, row: None }, start);
            }
            Tag::TableHead => {
                if let Some(Frame { kind: Kind::Table { in_head, row, .. }, .. }) = self.frames.last_mut() {
                    *in_head = true;
                    *row = Some(TableRow::default());
                }
            }
            Tag::TableRow => {
                self.want_sep(SEP_LINE);
                if let Some(Frame { kind: Kind::Table { row, .. }, .. }) = self.frames.last_mut() {
                    *row = Some(TableRow::default());
                }
            }
            Tag::TableCell => {
                let head = matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Table { in_head: true, .. }));
                self.open_inline(InlineKind::Cell { head, align: Align::Default }, start);
            }
            Tag::Emphasis => self.style_open(InlineStyle::Emphasis, start),
            Tag::Strong => self.style_open(InlineStyle::Strong, start),
            Tag::Strikethrough => self.style_open(InlineStyle::Strikethrough, start),
            Tag::Superscript => self.style_open(InlineStyle::Sup, start),
            Tag::Subscript => self.style_open(InlineStyle::Sub, start),
            Tag::Link { dest_url, title, .. } => {
                self.ensure_inline(start);
                let title = (!title.is_empty()).then(|| title.to_string());
                let wrap = match classify_link(&dest_url) {
                    Some(target) => Wrap::Link(target, title),
                    None => Wrap::Plain,
                };
                self.open_wrap(wrap, None);
            }
            Tag::Image { dest_url, title, .. } => {
                self.ensure_inline(start);
                self.flush_text();
                let source = classify_image(&dest_url);
                let image = Image {
                    source: source.unwrap_or(ImageSource::Local(String::new())),
                    alt: String::new(),
                    alt_text: None,
                    title: (!title.is_empty()).then(|| title.to_string()),
                    width: None,
                    height: None,
                };
                self.image = Some((image, String::new()));
            }
        }
    }

    fn style_open(&mut self, s: InlineStyle, start: u32) {
        self.ensure_inline(start);
        self.open_wrap(Wrap::Style(s), None);
    }

    fn end(&mut self, tag: TagEnd, _start: u32, end: u32) {
        match tag {
            TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::DefinitionListTitle | TagEnd::TableCell => self.close_inline(end),
            TagEnd::BlockQuote(_) => {
                self.pop_until(|k| matches!(k, Kind::Quote), end);
                self.want_sep(SEP_BLOCK);
            }
            TagEnd::CodeBlock => self.end_code(end),
            TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) | TagEnd::FootnoteDefinition => {}
            TagEnd::List(_) => {
                self.pop_until(|k| matches!(k, Kind::List { .. }), end);
                self.want_sep(SEP_BLOCK);
            }
            TagEnd::Item => {
                self.pop_until(|k| matches!(k, Kind::Item { .. }), end);
                self.want_sep(SEP_LINE);
            }
            TagEnd::DefinitionList => {
                self.pop_until(|k| matches!(k, Kind::DefList { .. }), end);
                self.want_sep(SEP_BLOCK);
            }
            TagEnd::DefinitionListDefinition => {
                self.pop_until(|k| matches!(k, Kind::Def), end);
                self.hidden -= 1;
                self.want_sep(SEP_LINE);
            }
            TagEnd::Table => {
                self.close_inline(end);
                self.pop_until(|k| matches!(k, Kind::Table { .. }), end);
                self.want_sep(SEP_BLOCK);
            }
            TagEnd::TableHead => {
                if let Some(Frame { kind: Kind::Table { in_head, row, head, .. }, .. }) = self.frames.last_mut() {
                    *in_head = false;
                    if let Some(r) = row.take() {
                        head.push(r);
                    }
                }
                self.want_sep(SEP_LINE);
            }
            TagEnd::TableRow => {
                if let Some(Frame { kind: Kind::Table { row, rows, .. }, .. }) = self.frames.last_mut() {
                    if let Some(r) = row.take() {
                        rows.push(r);
                    }
                }
                self.want_sep(SEP_LINE);
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Superscript | TagEnd::Subscript | TagEnd::Link => self.close_syntax(),
            TagEnd::Image => self.end_image(),
        }
    }

    fn end_image(&mut self) {
        let Some((mut img, alt)) = self.image.take() else { return };
        img.alt = alt;
        self.place_image(img);
    }

    fn place_image(&mut self, mut img: Image) {
        // An image whose address was refused is dropped, as the sanitiser removes its src.
        if matches!(&img.source, ImageSource::Local(s) if s.is_empty()) {
            return;
        }
        if matches!(img.source, ImageSource::Web(_)) {
            self.flush_text();
            let s = self.push_collapsed(&img.alt.clone());
            img.alt_text = Some(s);
        }
        self.add_inline(Inline::Image(img));
    }

    fn end_code(&mut self, end: u32) {
        let Some((lang, code, start)) = self.code.take() else { return };
        self.want_sep(SEP_BLOCK);
        let body = code.strip_suffix('\n').unwrap_or(&code);
        if lang.as_deref() == Some("mermaid") {
            let t = self.mark();
            self.push_no_find(body);
            let span = self.span_from(start, t, end);
            self.add_block(Block::Mermaid { src: body.to_string(), span });
        } else {
            let t = self.mark();
            let text = self.push_raw(body);
            let tokens = if self.colours { highlight::highlight(lang.as_deref().unwrap_or(""), body, text.start) } else { Vec::new() };
            let span = self.span_from(start, t, end);
            self.add_block(Block::Code(CodeBlock { lang, text, tokens, copyable: true, span }));
        }
        self.want_sep(SEP_BLOCK);
    }

    fn note_ref(&mut self, label: &str, src: u32) {
        self.ensure_inline(src);
        self.flush_text();
        if !self.defs_contains(label) && !self.note_numbers.contains_key(label) {
            // A definition can come later in the source; numbering does not depend on that.
        }
        let next = self.note_order.len() as u32 + 1;
        let e = self.note_numbers.entry(label.to_string()).or_insert((next, 0));
        let (number, uses) = *e;
        e.1 += 1;
        if number == next {
            self.note_order.push(label.to_string());
        }
        let id = if uses == 0 { format!("fnref:{number}") } else { format!("fnref{uses}:{number}") };
        let t = self.mark();
        let label_span = self.push_no_find(&number.to_string());
        self.anchors.push(Anchor { id: id.clone(), offset: t });
        self.add_inline(Inline::FootnoteRef { id, target: format!("fn:{number}"), label: label_span });
    }

    fn defs_contains(&self, label: &str) -> bool {
        self.defs.contains_key(label)
    }

    /// Writes footnotes at the end of the chunk and hands back the finished content.
    fn finish_chunk(mut self) -> (ChunkContent, Vec<HeadingInfo>) {
        let end = self.base + self.src.len() as u32;
        self.flush_text();
        self.close_inline(end);
        while self.frames.len() > 1 {
            self.pop_frame(end);
        }
        self.write_notes(end);
        let c = ChunkContent { index: self.index, text: self.text, blocks: std::mem::take(&mut self.frames[0].blocks), no_find: self.no_find, anchors: self.anchors };
        (c, self.headings)
    }

    fn write_notes(&mut self, end: u32) {
        if self.note_order.is_empty() {
            return;
        }
        self.want_sep(SEP_BLOCK);
        let t = self.mark();
        let mut notes = Vec::new();
        let mut i = 0;
        // A note can reference another, which adds to the order as it is written.
        while i < self.note_order.len() {
            let label = self.note_order[i].clone();
            i += 1;
            let number = self.note_numbers[&label].0;
            let Some(events) = self.defs.remove(&label) else { continue };
            self.want_sep(SEP_LINE);
            let id = format!("fn:{number}");
            let at = self.mark();
            self.anchors.push(Anchor { id: id.clone(), offset: at });
            self.hidden += 1;
            self.push_frame(Kind::Note, end);
            for (ev, range) in events {
                self.event(ev, range);
            }
            self.flush_text();
            self.close_inline(end);
            while !matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Note | Kind::Root)) {
                self.pop_frame(end);
            }
            // The arrow back to the marker, at the end of the last paragraph.
            self.sep = None;
            let back = self.push_no_find("\u{a0}\u{21a9}\u{fe0e}");
            let inline = Inline::FootnoteBack { target: format!("fnref:{number}"), label: back };
            let frame = self.frames.pop().expect("note frame");
            let mut blocks = frame.blocks;
            match blocks.last_mut() {
                Some(Block::Paragraph { content, span, .. }) => {
                    content.push(inline);
                    span.text.end = back.end;
                }
                _ => blocks.push(Block::Paragraph { content: vec![inline], tight: false, span: Span { source: ByteSpan::default(), text: back } }),
            }
            self.hidden -= 1;
            notes.push(Footnote { number, id, blocks });
            self.want_sep(SEP_LINE);
        }
        if !notes.is_empty() {
            let span = self.span_from(end, t, end);
            self.add_block(Block::Footnotes { items: notes, span });
        }
    }

    // ---- raw HTML ---------------------------------------------------------------------------

    fn html(&mut self, t: &str, start: u32, end: u32, block: bool) {
        for tok in html::scan(t) {
            if self.skip.is_some() {
                let (name, mut depth) = self.skip.clone().expect("skip");
                match &tok {
                    Tok::Open { name: n, self_closing: false, .. } if *n == name => depth += 1,
                    Tok::Close { name: n } if *n == name => depth -= 1,
                    _ => {}
                }
                self.skip = if depth <= 0 { None } else { Some((name, depth)) };
                continue;
            }
            match tok {
                Tok::Text(x) => {
                    if block || self.inline.is_some() || self.pre.is_some() {
                        self.raw_text(x, start);
                    }
                }
                Tok::Open { name, attrs, self_closing } => self.html_open(name, attrs, self_closing, start, end, block),
                Tok::Close { name } => self.html_close(&name, end),
            }
        }
    }

    fn html_open(&mut self, name: String, attrs: Vec<(String, String)>, self_closing: bool, start: u32, end: u32, block: bool) {
        if html::skips_content(&name) {
            if !self_closing {
                self.skip = Some((name, 1));
            }
            return;
        }
        if self.pre.is_some() || !html::allowed(&name) {
            return;
        }
        if let Some(id) = html::attr(&attrs, "id").or_else(|| if name == "a" { html::attr(&attrs, "name") } else { None }) {
            if !id.is_empty() {
                let offset = self.mark();
                self.anchors.push(Anchor { id: id.to_string(), offset });
            }
        }
        let align = parse_align(html::attr(&attrs, "align"));
        // Inside a paragraph or table cell block elements cannot open a block: they are dropped,
        // their text flowing inline. There is no general HTML layout here.
        let inline_only = !block || self.inline.as_ref().is_some_and(|c| matches!(c.kind, InlineKind::Cell { .. } | InlineKind::Term | InlineKind::Summary | InlineKind::Heading { .. }));
        match name.as_str() {
            "details" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.mark();
                self.push_frame(Kind::Details { open: html::attr(&attrs, "open").is_some(), summary: Vec::new(), summary_text: ByteSpan::default() }, start);
                return;
            }
            "summary" if self.frames.iter().any(|f| matches!(f.kind, Kind::Details { .. })) && self.inline.is_none() => {
                self.want_sep(SEP_BLOCK);
                self.mark();
                self.open_inline(InlineKind::Summary, start);
                return;
            }
            "div" | "section" | "article" | "aside" | "figure" | "figcaption" | "center" | "hgroup" | "nav" | "main" | "header" | "footer" | "address" if !inline_only => {
                self.close_inline(start);
                let align = if name == "center" { Align::Centre } else { align };
                self.want_sep(SEP_BLOCK);
                self.mark();
                self.push_frame(Kind::Html { tag: name, align }, start);
                return;
            }
            "p" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.mark();
                if align != Align::Default {
                    self.push_frame(Kind::Html { tag: "p".into(), align }, start);
                }
                self.open_inline(InlineKind::Para { tight: false }, start);
                return;
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.mark();
                let level = name.as_bytes()[1] - b'0';
                if align != Align::Default {
                    self.push_frame(Kind::Html { tag: name.clone(), align }, start);
                }
                self.open_inline(InlineKind::Heading { level, explicit: html::attr(&attrs, "id").map(str::to_string), raw: true }, start);
                return;
            }
            "blockquote" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.mark();
                self.push_frame(Kind::Quote, start);
                return;
            }
            "ul" | "ol" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.mark();
                let first = html::attr(&attrs, "start").and_then(|s| s.parse().ok()).unwrap_or(1);
                self.push_frame(Kind::List { ordered: name == "ol", start: first, raw: true }, start);
                return;
            }
            "li" if !inline_only && self.frames.iter().any(|f| matches!(f.kind, Kind::List { .. })) => {
                self.close_inline(start);
                while !matches!(self.frames.last().map(|f| &f.kind), Some(Kind::List { .. } | Kind::Root)) {
                    self.pop_frame(start);
                }
                self.want_sep(SEP_LINE);
                self.mark();
                self.push_frame(Kind::Item { checked: None }, start);
                return;
            }
            "dl" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.mark();
                self.push_frame(Kind::DefList { raw: true }, start);
                return;
            }
            "dt" if !inline_only && self.frames.iter().any(|f| matches!(f.kind, Kind::DefList { .. })) => {
                self.close_inline(start);
                while !matches!(self.frames.last().map(|f| &f.kind), Some(Kind::DefList { .. } | Kind::Root)) {
                    self.pop_frame(start);
                }
                self.want_sep(SEP_LINE);
                self.mark();
                self.open_inline(InlineKind::Term, start);
                return;
            }
            "dd" if !inline_only && self.frames.iter().any(|f| matches!(f.kind, Kind::DefList { .. })) => {
                self.close_inline(start);
                while !matches!(self.frames.last().map(|f| &f.kind), Some(Kind::DefList { .. } | Kind::Root)) {
                    self.pop_frame(start);
                }
                if self.frames.last().is_some_and(|f| f.defs.is_empty()) {
                    self.frames.last_mut().expect("frame").defs.push(DefinitionItem::default());
                }
                self.want_sep(SEP_LINE);
                self.mark();
                self.hidden += 1;
                self.push_frame(Kind::Def, start);
                return;
            }
            "table" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.mark();
                self.push_frame(Kind::Table { raw: true, aligns: Vec::new(), head: Vec::new(), rows: Vec::new(), in_head: false, row: None }, start);
                return;
            }
            "thead" => {
                if let Some(Frame { kind: Kind::Table { in_head, .. }, .. }) = self.frames.last_mut() {
                    *in_head = true;
                }
                return;
            }
            "tbody" | "tfoot" => {
                self.close_inline(start);
                if let Some(Frame { kind: Kind::Table { in_head, row, head, rows, .. }, .. }) = self.frames.last_mut() {
                    if let Some(r) = row.take() {
                        if *in_head { head.push(r) } else { rows.push(r) }
                    }
                    *in_head = false;
                }
                return;
            }
            "tr" => {
                self.close_inline(start);
                if let Some(Frame { kind: Kind::Table { in_head, row, head, rows, .. }, .. }) = self.frames.last_mut() {
                    if let Some(r) = row.take() {
                        if *in_head { head.push(r) } else { rows.push(r) }
                    }
                    *row = Some(TableRow::default());
                }
                self.want_sep(SEP_LINE);
                return;
            }
            "td" | "th" if matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Table { .. })) => {
                self.close_inline(start);
                self.want_sep(SEP_CELL);
                self.mark();
                if let Some(Frame { kind: Kind::Table { row, in_head, .. }, .. }) = self.frames.last_mut() {
                    if row.is_none() {
                        *row = Some(TableRow::default());
                    }
                    let head = name == "th" && *in_head;
                    self.open_inline(InlineKind::Cell { head, align }, start);
                }
                return;
            }
            "caption" | "colgroup" | "col" | "picture" | "source" | "wbr" | "area" | "map" | "meter" | "progress" => return,
            "hr" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                let span = self.span_from(start, self.text.len() as u32, end);
                self.add_block(Block::Rule { span });
                return;
            }
            "pre" if !inline_only => {
                self.close_inline(start);
                self.want_sep(SEP_BLOCK);
                self.pre = Some(String::new());
                return;
            }
            "input" => {
                if html::attr(&attrs, "type") == Some("checkbox") {
                    if let Some(Frame { kind: Kind::Item { checked }, .. }) = self.frames.last_mut() {
                        *checked = Some(html::attr(&attrs, "checked").is_some());
                    }
                }
                return;
            }
            _ => {}
        }
        // From here on the element is inline.
        match name.as_str() {
            "br" => {
                self.ensure_inline(start);
                self.flush_text();
                self.mark();
                self.text.push('\n');
                self.ws = true;
                self.add_inline(Inline::Break);
                return;
            }
            "img" => {
                self.ensure_inline(start);
                self.flush_text();
                let src = html::attr(&attrs, "src").unwrap_or("");
                if let Some(source) = classify_image(src) {
                    let dim = |k: &str| html::attr(&attrs, k).and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok()).filter(|v| v.is_finite() && *v >= 0.0);
                    let img = Image {
                        source,
                        alt: html::attr(&attrs, "alt").unwrap_or("").to_string(),
                        alt_text: None,
                        title: html::attr(&attrs, "title").map(str::to_string),
                        width: dim("width"),
                        height: dim("height"),
                    };
                    self.place_image(img);
                }
                return;
            }
            _ => {}
        }
        if self_closing {
            return;
        }
        self.ensure_inline(start);
        let wrap = match name.as_str() {
            "a" => match html::attr(&attrs, "href").and_then(classify_link) {
                Some(t) => Wrap::Link(t, html::attr(&attrs, "title").map(str::to_string)),
                None => Wrap::Plain,
            },
            "em" | "i" | "cite" | "dfn" => Wrap::Style(InlineStyle::Emphasis),
            "strong" | "b" => Wrap::Style(InlineStyle::Strong),
            "del" | "s" | "strike" => Wrap::Style(InlineStyle::Strikethrough),
            "ins" | "u" => Wrap::Style(InlineStyle::Underline),
            "kbd" => Wrap::Style(InlineStyle::Kbd),
            "sub" => Wrap::Style(InlineStyle::Sub),
            "sup" => Wrap::Style(InlineStyle::Sup),
            "mark" => Wrap::Style(InlineStyle::Mark),
            "small" => Wrap::Style(InlineStyle::Small),
            "tt" | "samp" | "var" => Wrap::Style(InlineStyle::Mono),
            _ => Wrap::Plain,
        };
        let wrap = if name == "code" { Wrap::Style(InlineStyle::Code) } else { wrap };
        self.open_wrap(wrap, Some(name));
    }

    fn html_close(&mut self, name: &str, end: u32) {
        if name == "pre" {
            if let Some(body) = self.pre.take() {
                let body = body.strip_prefix('\n').unwrap_or(&body);
                let body = body.strip_suffix('\n').unwrap_or(body);
                let t = self.mark();
                let text = self.push_raw(body);
                let span = self.span_from(end, t, end);
                self.add_block(Block::Code(CodeBlock { lang: None, text, tokens: Vec::new(), copyable: false, span }));
                self.want_sep(SEP_BLOCK);
            }
            return;
        }
        if self.pre.is_some() {
            return;
        }
        match name {
            "details" => {
                if self.pop_until(|k| matches!(k, Kind::Details { .. }), end) {
                    self.want_sep(SEP_BLOCK);
                }
            }
            "summary" => {
                if self.inline.as_ref().is_some_and(|c| c.kind == InlineKind::Summary) {
                    self.close_inline(end);
                }
            }
            "div" | "section" | "article" | "aside" | "figure" | "figcaption" | "center" | "hgroup" | "nav" | "main" | "header" | "footer" | "address" => {
                let tag = name.to_string();
                if self.pop_until(|k| matches!(k, Kind::Html { tag: t, .. } if *t == tag), end) {
                    self.want_sep(SEP_BLOCK);
                }
            }
            "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let is_para = name == "p" && self.inline.as_ref().is_some_and(|c| matches!(c.kind, InlineKind::Para { .. }));
                let is_head = name != "p" && self.inline.as_ref().is_some_and(|c| matches!(c.kind, InlineKind::Heading { raw: true, .. }));
                if is_para || is_head {
                    self.close_inline(end);
                }
                if matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Html { tag, .. }) if tag == name) {
                    self.pop_frame(end);
                }
            }
            "blockquote" => {
                self.pop_until(|k| matches!(k, Kind::Quote), end);
            }
            "ul" | "ol" => {
                if self.frames.iter().any(|f| matches!(f.kind, Kind::List { raw: true, .. })) {
                    self.pop_until(|k| matches!(k, Kind::List { raw: true, .. }), end);
                    self.want_sep(SEP_BLOCK);
                }
            }
            "li" => {
                if matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Item { .. })) {
                    self.pop_frame(end);
                }
            }
            "dl" => {
                if self.frames.iter().any(|f| matches!(f.kind, Kind::DefList { raw: true })) {
                    self.pop_until(|k| matches!(k, Kind::DefList { raw: true }), end);
                }
            }
            "dt" => {
                if self.inline.as_ref().is_some_and(|c| c.kind == InlineKind::Term) {
                    self.close_inline(end);
                }
            }
            "dd" => {
                if matches!(self.frames.last().map(|f| &f.kind), Some(Kind::Def)) {
                    self.pop_frame(end);
                    self.hidden -= 1;
                }
            }
            "table" => {
                if self.frames.iter().any(|f| matches!(f.kind, Kind::Table { raw: true, .. })) {
                    self.close_inline(end);
                    self.pop_until(|k| matches!(k, Kind::Table { raw: true, .. }), end);
                    self.want_sep(SEP_BLOCK);
                }
            }
            "td" | "th" => {
                if self.inline.as_ref().is_some_and(|c| matches!(c.kind, InlineKind::Cell { .. })) {
                    self.close_inline(end);
                }
            }
            "tr" | "thead" | "tbody" | "tfoot" => {
                self.close_inline(end);
                if let Some(Frame { kind: Kind::Table { in_head, row, head, rows, .. }, .. }) = self.frames.last_mut() {
                    if let Some(r) = row.take() {
                        if *in_head { head.push(r) } else { rows.push(r) }
                    }
                    if name == "thead" {
                        *in_head = false;
                    }
                }
                self.want_sep(SEP_LINE);
            }
            _ => self.close_raw(name),
        }
    }
}

fn finish_wrap(top: Open) -> Vec<Inline> {
    match top.wrap {
        Wrap::Root | Wrap::Plain => top.content,
        Wrap::Style(style) => vec![Inline::Styled { style, content: top.content }],
        Wrap::Link(target, title) => vec![Inline::Link { target, title, content: top.content }],
    }
}

/// Clamps the spans of the last inline chain after trailing white space was removed.
fn shrink_to(content: &mut [Inline], end: u32) {
    let Some(last) = content.last_mut() else { return };
    match last {
        Inline::Text(s) | Inline::Code(s) => s.end = s.end.min(end),
        Inline::Styled { content, .. } | Inline::Link { content, .. } => shrink_to(content, end),
        _ => {}
    }
}

fn fence_lang(info: &str) -> Option<String> {
    let first = info.split([' ', '\t', '{']).next().unwrap_or("");
    (!first.is_empty()).then(|| first.to_lowercase())
}

fn parse_align(a: Option<&str>) -> Align {
    match a.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("center" | "middle") => Align::Centre,
        Some("left") => Align::Left,
        Some("right") => Align::Right,
        _ => Align::Default,
    }
}

fn percent_decode_lossy(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 {
            if let (Some(h), Some(l)) = (b.get(i + 1).and_then(|c| (*c as char).to_digit(16)), b.get(i + 2).and_then(|c| (*c as char).to_digit(16))) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where a link may go. `http`, `https` and `mailto` follow the sanitiser's allowed schemes; other
/// schemes lose their target and keep their text. Network paths are refused later, on follow.
pub fn classify_link(dest: &str) -> Option<LinkTarget> {
    let dest = dest.trim();
    if dest.is_empty() {
        return None;
    }
    if let Some(frag) = dest.strip_prefix('#') {
        return Some(LinkTarget::Fragment(percent_decode_lossy(frag)));
    }
    if let Some(scheme) = paths::scheme(dest) {
        return match scheme.as_str() {
            "http" | "https" => Some(LinkTarget::Web(dest.to_string())),
            "mailto" => Some(LinkTarget::Mail(dest.to_string())),
            _ => None,
        };
    }
    let (path, frag) = match dest.split_once('#') {
        Some((p, f)) => (p, Some(percent_decode_lossy(f))),
        None => (dest, None),
    };
    Some(LinkTarget::Local { path: path.to_string(), fragment: frag })
}

/// Where an image comes from. `None` when the sanitiser would remove its address.
pub fn classify_image(src: &str) -> Option<ImageSource> {
    if src.trim().is_empty() {
        return None;
    }
    if paths::is_web_url(src) {
        return Some(ImageSource::Web(src.to_string()));
    }
    if let Some(scheme) = paths::scheme(src) {
        if scheme == "data" {
            let lower = src.trim_start().to_ascii_lowercase();
            let ok = ["data:image/png;base64,", "data:image/jpeg;base64,", "data:image/jpg;base64,", "data:image/gif;base64,", "data:image/webp;base64,"].iter().any(|p| lower.starts_with(p));
            return ok.then(|| ImageSource::Data(src.to_string()));
        }
        // A Windows drive letter reads as a one-letter scheme.
        if scheme.len() != 1 {
            return None;
        }
    }
    Some(ImageSource::Local(src.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn whole(s: &str) -> WholeParse {
        parse_whole(s, 16 * 1024, false)
    }

    fn one(s: &str) -> ChunkContent {
        let mut w = whole(s);
        assert_eq!(w.chunks.len(), 1);
        w.chunks.remove(0)
    }

    fn inline_text(c: &ChunkContent, i: &[Inline]) -> String {
        let mut o = String::new();
        for x in i {
            match x {
                Inline::Text(s) | Inline::Code(s) => o.push_str(c.slice(*s)),
                Inline::Styled { content, .. } | Inline::Link { content, .. } => o.push_str(&inline_text(c, content)),
                Inline::Math { text, .. } => o.push_str(c.slice(*text)),
                Inline::FootnoteRef { label, .. } | Inline::FootnoteBack { target: _, label } => o.push_str(c.slice(*label)),
                Inline::Break => o.push('\n'),
                Inline::Image(img) => {
                    if let Some(a) = img.alt_text {
                        o.push_str(c.slice(a));
                    }
                }
            }
        }
        o
    }

    /// `TestSlugs`.
    #[test]
    fn slugs() {
        let w = whole("# Hello, World!\n\n## Hello World\n\n## Hello World\n\n## `code` & *more*\n\n## Custom {#mine}\n");
        let ids: Vec<_> = w.headings.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["hello-world", "hello-world-1", "hello-world-2", "code--more", "mine"]);
    }

    /// `TestChunking`.
    #[test]
    fn chunking() {
        let mut b = String::new();
        for i in 0..2000 {
            b.push_str(&format!("## Section {i}\n\nSome paragraph text for section {i} with **bold** words.\n\n"));
        }
        let w = whole(&b);
        assert!(w.chunks.len() >= 5, "{}", w.chunks.len());
        let total: u32 = w.ranges.iter().map(|r| r.1 - r.0).sum();
        assert_eq!(total as usize, b.len());
        assert_eq!(w.headings.len(), 2000);
        let last = w.headings.last().unwrap();
        assert!(w.chunks[last.chunk as usize].anchors.iter().any(|a| a.id == last.id));
    }

    /// `TestDetailsNotSplit`.
    #[test]
    fn details_not_split() {
        let mut b = String::from("<details>\n<summary>More</summary>\n\n");
        for _ in 0..1000 {
            b.push_str("Line of text inside details.\n\n");
        }
        b.push_str("</details>\n\nAfter.\n");
        let w = whole(&b);
        let Block::Details(d) = &w.chunks[0].blocks[0] else { panic!("{:?}", w.chunks[0].blocks[0]) };
        assert_eq!(d.blocks.len(), 1000, "details split across chunks");
    }

    /// `TestRendering`, on the model.
    #[test]
    fn rendering_blocks() {
        let src = "```mermaid\ngraph TD; A-->B\n```\n\n```go\nfunc main() {}\n```\n\nInline $x^2$ costs $5 and $10.\n\n$$\na+b\n$$\n\n- [x] done\n- [ ] todo\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nNote[^1].\n\n[^1]: Footnote.\n\n<script>alert(1)</script>\n\n<details><summary>S</summary>hidden</details>\n";
        let c = one(src);
        let mut kinds = Vec::new();
        for b in &c.blocks {
            kinds.push(match b {
                Block::Mermaid { src, .. } => {
                    assert_eq!(src, "graph TD; A-->B");
                    "mermaid"
                }
                Block::Code(cb) => {
                    assert_eq!(cb.lang.as_deref(), Some("go"));
                    "code"
                }
                Block::Paragraph { content, .. } => {
                    let t = inline_text(&c, content);
                    if t.contains("costs") {
                        assert!(content.iter().any(|i| matches!(i, Inline::Math { src, .. } if src == "x^2")), "{content:?}");
                        assert!(t.contains("costs $5 and $10."), "{t}");
                    }
                    "para"
                }
                Block::Math { src, display, .. } => {
                    assert_eq!(src, "a+b");
                    assert!(*display);
                    "math"
                }
                Block::List(l) => {
                    assert_eq!(l.items[0].checked, Some(true));
                    assert_eq!(l.items[1].checked, Some(false));
                    "list"
                }
                Block::Table(t) => {
                    assert_eq!(t.head.len(), 1);
                    assert_eq!(t.rows.len(), 1);
                    "table"
                }
                Block::Details(d) => {
                    assert_eq!(inline_text(&c, &d.summary), "S");
                    assert_eq!(d.blocks.len(), 1);
                    "details"
                }
                Block::Footnotes { items, .. } => {
                    assert_eq!(items[0].id, "fn:1");
                    "notes"
                }
                other => panic!("unexpected {other:?}"),
            });
        }
        assert_eq!(kinds, ["mermaid", "code", "para", "math", "list", "table", "para", "details", "notes"]);
        assert!(!c.text.contains("alert"), "{}", c.text);
        assert!(c.anchors.iter().any(|a| a.id == "fn:1"));
    }

    #[test]
    fn flat_text_is_in_reading_order_with_separators() {
        let c = one("# T\n\nOne *two*  three\nfour.\n\n- a\n- b\n\n| x | y |\n|---|---|\n| 1 | 2 |\n\nSee[^n].\n\n[^n]: Note.\n");
        assert_eq!(c.text, "T\n\nOne two three four.\n\na\nb\n\nx\ty\n1\t2\n\nSee1.\n\nNote.\u{a0}\u{21a9}\u{fe0e}");
        // The marker and arrow are shown but not searched.
        assert_eq!(c.no_find.len(), 2);
    }

    #[test]
    fn hard_breaks_and_autolinks() {
        let c = one("a  \nb https://example.com/x. c\n");
        let Block::Paragraph { content, .. } = &c.blocks[0] else { panic!() };
        assert!(content.iter().any(|i| matches!(i, Inline::Break)));
        assert!(content.iter().any(|i| matches!(i, Inline::Link { target: LinkTarget::Web(u), .. } if u == "https://example.com/x")));
        assert_eq!(c.text, "a\nb https://example.com/x. c");
    }

    #[test]
    fn inline_html_maps_to_styles() {
        let c = one("<kbd>Ctrl</kbd>+H<sub>2</sub>x<sup>2</sup> <mark>m</mark> <b>b</b> <span>s</span> <nosuch>t</nosuch>\n");
        let Block::Paragraph { content, .. } = &c.blocks[0] else { panic!() };
        let styles: Vec<_> = content.iter().filter_map(|i| if let Inline::Styled { style, .. } = i { Some(*style) } else { None }).collect();
        assert_eq!(styles, [InlineStyle::Kbd, InlineStyle::Sub, InlineStyle::Sup, InlineStyle::Mark, InlineStyle::Strong]);
        assert_eq!(c.text, "Ctrl+H2x2 m b s t");
    }

    /// `TestHostileMarkdown`: every case from the web reader's table must leave no target, image
    /// source or text that the reader would act on.
    #[test]
    fn hostile_markdown() {
        let cases = [
            "<script>alert(1)</script>",
            "<img src=x onerror=alert(1)>",
            "[a](javascript:alert(1))",
            "[a](JaVaScRiPt:alert(1))",
            "[a](&#106;avascript:alert(1))",
            "[a](vbscript:x)",
            "[a](data:text/html,<script>alert(1)</script>)",
            r#"<a href="javascript&colon;alert(1)">x</a>"#,
            "<svg><script>alert(1)</script></svg>",
            "<iframe src=https://x></iframe><object data=x></object><embed src=x><form action=x><input name=a></form>",
            r#"<meta http-equiv=refresh content="0;url=https://x"><base href=https://x/><style>*{}</style><p style="color:red">s</p>"#,
            "<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>",
            "<template><img src=x onerror=alert(1)></template>",
            "# Heading {onclick=alert(1) .code}",
            "[link](https://ok.example){onmouseover=alert(1)}",
            "<details open ontoggle=alert(1)><summary>s</summary></details>",
            "<input type=checkbox onfocus=alert(1) autofocus>",
            "```mermaid\n</pre><script>alert(1)</script>\n```",
            "```js\" onmouseover=\"alert(1)\n</code>\n```",
            "$</span><script>alert(1)</script>$",
            "<video src=https://t.example/v.mp4></video><audio src=x></audio>",
            r#"<table background="https://t.example/bg.png"><tr><td>x</td></tr></table>"#,
            "![t](https://t.example/x.png) ![p](//t.example/y.png) ![b](\\\\t.example/z.png) ![s]( \thttps://t.example/w.png)",
            "<img src=\"HTTPS://t.example/a.png\"> <img src=\"ht\ttps://t.example/b.png\"> <img src=\"/\\t.example/c.png\">",
            r#"<picture><source srcset="local.png 1x, https://t.example/a.png 2x"><img src="https://t.example/b.png"></picture>"#,
            "<div\nonclick=alert(1)\nclass=code>x</div>",
        ];
        fn scan_inlines(i: &[Inline], case: &str) {
            for x in i {
                match x {
                    Inline::Link { target, content, .. } => {
                        match target {
                            LinkTarget::Web(u) => assert!(u.starts_with("http"), "{case}: {u}"),
                            LinkTarget::Local { path, .. } => assert!(paths::scheme(path).is_none(), "{case}: {path}"),
                            _ => {}
                        }
                        scan_inlines(content, case);
                    }
                    Inline::Styled { content, .. } => scan_inlines(content, case),
                    Inline::Image(img) => {
                        // Anything pointing at another computer must be held, never local.
                        if let ImageSource::Local(s) = &img.source {
                            assert!(!paths::is_web_url(s), "{case}: {s}");
                        }
                    }
                    _ => {}
                }
            }
        }
        fn scan_blocks(b: &[Block], case: &str) {
            for x in b {
                match x {
                    Block::Paragraph { content, .. } | Block::Heading { content, .. } => scan_inlines(content, case),
                    Block::Quote { blocks, .. } | Block::Aligned { blocks, .. } => scan_blocks(blocks, case),
                    Block::Details(d) => {
                        scan_inlines(&d.summary, case);
                        scan_blocks(&d.blocks, case);
                    }
                    Block::List(l) => l.items.iter().for_each(|i| scan_blocks(&i.blocks, case)),
                    Block::Table(t) => t.head.iter().chain(&t.rows).flat_map(|r| &r.cells).for_each(|c| scan_inlines(&c.content, case)),
                    _ => {}
                }
            }
        }
        for case in cases {
            let w = whole(case);
            for c in &w.chunks {
                scan_blocks(&c.blocks, case);
                // Dropped elements leave nothing behind. Other cases may keep inert text.
                if case.starts_with("<script") || case.starts_with("<svg") || case.starts_with("<template") {
                    assert!(!c.text.contains("alert"), "{case}: {}", c.text);
                }
            }
        }
    }

    #[test]
    fn links_classify() {
        assert_eq!(classify_link("#caf%C3%A9"), Some(LinkTarget::Fragment("café".into())));
        assert_eq!(classify_link("other.md#s"), Some(LinkTarget::Local { path: "other.md".into(), fragment: Some("s".into()) }));
        assert_eq!(classify_link("HTTPS://x.example"), Some(LinkTarget::Web("HTTPS://x.example".into())));
        assert_eq!(classify_link("mailto:a@b.example"), Some(LinkTarget::Mail("mailto:a@b.example".into())));
        for bad in ["javascript:alert(1)", "file:///etc/passwd", "ftp://x.example", "java\tscript:alert(1)"] {
            assert_eq!(classify_link(bad), None, "{bad}");
        }
    }

    #[test]
    fn images_classify() {
        assert_eq!(classify_image("a.png"), Some(ImageSource::Local("a.png".into())));
        assert!(matches!(classify_image("https://x/a.png"), Some(ImageSource::Web(_))));
        assert!(matches!(classify_image(r"\\host\share\a.png"), Some(ImageSource::Web(_))));
        assert!(matches!(classify_image("data:image/png;base64,AA"), Some(ImageSource::Data(_))));
        assert_eq!(classify_image("data:text/html,x"), None);
        assert_eq!(classify_image("file:///x.png"), None);
    }

    #[test]
    fn section_uses_global_references_and_ids() {
        let mut refs = RefMap::default();
        refs.0.insert("r".into(), ("https://example.com/ref".into(), String::new()));
        let ids = vec!["assigned-7".to_string()];
        let c = parse_section("# Title\n\nSee [the ref][r] and [R].\n", 100, 3, &refs, &ids, false);
        assert_eq!(c.anchors[0].id, "assigned-7");
        let Block::Heading { span, .. } = &c.blocks[0] else { panic!() };
        assert_eq!(span.source.start, 100, "source offsets include the section base");
        let Block::Paragraph { content, .. } = &c.blocks[1] else { panic!() };
        let n = content.iter().filter(|i| matches!(i, Inline::Link { target: LinkTarget::Web(u), .. } if u == "https://example.com/ref")).count();
        assert_eq!(n, 2);
    }

    #[test]
    fn raw_html_blocks_map() {
        let c = one("<p align=\"center\">Centred.</p>\n\n<div align=\"right\">\n\nx\n\n</div>\n\n<center>c</center>\n\n<ul><li>one</li><li>two</li></ul>\n\n<table><tr><th>h</th></tr><tr><td align=\"right\">d</td></tr></table>\n\n<pre>a\n  b</pre>\n");
        assert!(matches!(&c.blocks[0], Block::Aligned { align: Align::Centre, .. }));
        assert!(matches!(&c.blocks[1], Block::Aligned { align: Align::Right, .. }));
        assert!(matches!(&c.blocks[2], Block::Aligned { align: Align::Centre, .. }));
        assert!(matches!(&c.blocks[3], Block::List(l) if l.items.len() == 2));
        assert!(matches!(&c.blocks[4], Block::Table(t) if t.rows.len() + t.head.len() == 2));
        assert!(matches!(&c.blocks[5], Block::Code(cb) if !cb.copyable && c.slice(cb.text) == "a\n  b"));
    }
}
