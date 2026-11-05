//! The document model: what a chunk of markdown means, with no opinion on how it is drawn.
//!
//! The on-screen reader consumes this model today and PDF output will consume it later, so this
//! module must never import GPUI. Colours, fonts and pixel sizes belong to the consumer.
//!
//! # Text and positions
//!
//! Each parsed chunk owns one flat string, [`ChunkContent::text`], holding its visible text in
//! reading order. Inline text does not hold strings of its own: it holds [`ByteSpan`]s into that
//! string. Selection, find and copy all work on it, so a position stays meaningful while the
//! chunk is not parsed or laid out at all:
//!
//! - A [`DocPos`] (chunk index plus byte offset into the flat text) identifies a place in the
//!   whole document and orders correctly across chunks.
//! - Reparsing the same source yields the same flat text, so a chunk can be dropped from the
//!   cache and a position into it stays valid.
//! - Block separators are part of the flat text (see [`SEP_BLOCK`] and friends), which is what
//!   makes a copied selection read properly.
//! - [`ChunkContent::no_find`] marks text that is shown and copied but not searched: maths source,
//!   diagram source and footnote markers, matching what the web reader's find skips.
//!
//! Text a list marker or button shows (bullets, numbers, the Copy label) is not document text.
//! It is not in the flat string, as it is not selectable in a browser either.
//!
//! # Source mapping
//!
//! Every [`Block`] carries a [`Span`]: its byte range in the document source and in the flat text.

/// Separates blocks in the flat text.
pub const SEP_BLOCK: &str = "\n\n";
/// Separates list items, table rows and lines within a block in the flat text.
pub const SEP_LINE: &str = "\n";
/// Separates table cells in the flat text.
pub const SEP_CELL: &str = "\t";

/// A half-open byte range. Offsets are `u32` because a document is capped at 256 MB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ByteSpan {
    pub start: u32,
    pub end: u32,
}

impl ByteSpan {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }
    pub const fn len(self) -> u32 {
        self.end - self.start
    }
    pub const fn is_empty(self) -> bool {
        self.end <= self.start
    }
    pub const fn contains(self, offset: u32) -> bool {
        self.start <= offset && offset < self.end
    }
    pub fn range(self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// A position in the whole document: stable while the chunk it points into is unmounted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocPos {
    pub chunk: u32,
    pub offset: u32,
}

impl DocPos {
    pub const fn new(chunk: u32, offset: u32) -> Self {
        Self { chunk, offset }
    }
}

/// Where a block came from and where its text landed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Span {
    /// Bytes in the document source (after newline normalisation).
    pub source: ByteSpan,
    /// Bytes in [`ChunkContent::text`].
    pub text: ByteSpan,
}

/// One parsed chunk. Large files keep only a bounded number of these.
#[derive(Clone, Debug, Default)]
pub struct ChunkContent {
    pub index: u32,
    pub text: String,
    pub blocks: Vec<Block>,
    /// Ascending, non-overlapping spans of `text` that find must skip.
    pub no_find: Vec<ByteSpan>,
    /// Element ids in this chunk (headings and footnotes), with the text offset they point at.
    pub anchors: Vec<Anchor>,
}

impl ChunkContent {
    pub fn slice(&self, span: ByteSpan) -> &str {
        &self.text[span.range()]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub id: String,
    pub offset: u32,
}

/// A horizontal text alignment. `Default` means "inherit, or start".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Default,
    Left,
    Centre,
    Right,
}

/// A block-level element.
///
/// Maths and diagrams hold their raw source. Rendering them belongs to the content crate; the
/// reader asks it for an asset and reserves space until one arrives.
#[derive(Clone, Debug)]
pub enum Block {
    Heading { level: u8, id: String, content: Vec<Inline>, span: Span },
    /// `tight` is a list item's or definition's bare text: there is no paragraph margin.
    Paragraph { content: Vec<Inline>, tight: bool, span: Span },
    Quote { blocks: Vec<Block>, span: Span },
    List(List),
    Code(CodeBlock),
    Table(Table),
    Rule { span: Span },
    Details(Details),
    DefinitionList { items: Vec<DefinitionItem>, span: Span },
    Footnotes { items: Vec<Footnote>, span: Span },
    /// Blocks from raw HTML such as `<div align="center">` or `<center>`.
    Aligned { align: Align, blocks: Vec<Block>, span: Span },
    Math { src: String, display: bool, span: Span },
    Mermaid { src: String, span: Span },
}

impl Block {
    pub fn span(&self) -> Span {
        match self {
            Block::Heading { span, .. }
            | Block::Paragraph { span, .. }
            | Block::Quote { span, .. }
            | Block::Rule { span }
            | Block::DefinitionList { span, .. }
            | Block::Footnotes { span, .. }
            | Block::Aligned { span, .. }
            | Block::Math { span, .. }
            | Block::Mermaid { span, .. } => *span,
            Block::List(b) => b.span,
            Block::Code(b) => b.span,
            Block::Table(b) => b.span,
            Block::Details(b) => b.span,
        }
    }
}

/// An inline element. Text lives in the chunk's flat string.
#[derive(Clone, Debug)]
pub enum Inline {
    Text(ByteSpan),
    /// `<code>` in running text.
    Code(ByteSpan),
    Styled { style: InlineStyle, content: Vec<Inline> },
    Link { target: LinkTarget, title: Option<String>, content: Vec<Inline> },
    Image(Image),
    /// `src` is the raw source. `text` is its copy in the flat text, used until an asset arrives.
    Math { src: String, text: ByteSpan },
    /// A footnote marker linking to `Footnote::id`. Its visible label is in `no_find`.
    FootnoteRef { id: String, target: String, label: ByteSpan },
    /// The arrow at the end of a footnote linking back to its marker. In `no_find`.
    FootnoteBack { target: String, label: ByteSpan },
    /// A hard line break. The flat text has `\n` here.
    Break,
}

/// Inline formatting that changes how text looks but not what it means structurally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InlineStyle {
    Emphasis,
    Strong,
    Strikethrough,
    /// `<ins>` and `<u>`.
    Underline,
    Kbd,
    Sub,
    Sup,
    Mark,
    /// `<small>`.
    Small,
    /// `<tt>`, `<samp>` and `<var>`: monospaced with no code box.
    Mono,
    /// A raw `<code>` element, which can hold other inline elements.
    Code,
}

/// Where a link goes. Anything not listed (`javascript:`, `file:` and so on) is not a link: its
/// text is kept and the target dropped, as the HTML sanitiser does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// An `http` or `https` address, opened in the browser.
    Web(String),
    Mail(String),
    /// `#id` in this document.
    Fragment(String),
    /// A relative or absolute file reference, resolved against the document's folder only when
    /// followed. See `paths::resolve_link`, which refuses network and device paths.
    Local { path: String, fragment: Option<String> },
}

#[derive(Clone, Debug)]
pub struct Image {
    pub source: ImageSource,
    pub alt: String,
    /// The alt text in the flat text, only when it is what the reader sees (a held web image).
    pub alt_text: Option<ByteSpan>,
    pub title: Option<String>,
    /// `width` and `height` attributes from raw HTML, in CSS px.
    pub width: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageSource {
    /// A file reference, resolved against the document's folder only when loaded.
    Local(String),
    /// A base64 `data:image/...` URI.
    Data(String),
    /// On another computer. Never fetched by the reader: it shows the alt text in a dashed box.
    Web(String),
}

#[derive(Clone, Debug, Default)]
pub struct List {
    pub ordered: bool,
    pub start: u32,
    pub tight: bool,
    pub items: Vec<ListItem>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct ListItem {
    /// `Some` for a task list item.
    pub checked: Option<bool>,
    pub blocks: Vec<Block>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct CodeBlock {
    /// The fence's language, lower-cased.
    pub lang: Option<String>,
    /// The code in the flat text, without its final newline.
    pub text: ByteSpan,
    /// Coloured stretches, relative to the flat text, ascending. Gaps are plain code text.
    pub tokens: Vec<CodeToken>,
    /// Fenced and indented code has a copy button; a raw HTML `<pre>` does not.
    pub copyable: bool,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CodeToken {
    pub span: ByteSpan,
    pub class: TokenClass,
}

/// Syntax token classes, named after the chroma tokens the web reader's colours are defined on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum TokenClass {
    Keyword,
    KeywordConstant,
    KeywordDeclaration,
    KeywordNamespace,
    KeywordPseudo,
    KeywordType,
    Name,
    NameAttribute,
    NameBuiltin,
    NameBuiltinPseudo,
    NameClass,
    NameConstant,
    NameDecorator,
    NameEntity,
    NameException,
    NameFunction,
    NameLabel,
    NameNamespace,
    NameProperty,
    NameTag,
    NameVariable,
    Literal,
    String,
    StringEscape,
    StringRegex,
    StringSymbol,
    Number,
    Operator,
    Punctuation,
    Comment,
    CommentPreproc,
    GenericDeleted,
    GenericInserted,
    GenericHeading,
    GenericSubheading,
    GenericEmph,
    GenericStrong,
    Error,
}

#[derive(Clone, Debug, Default)]
pub struct Table {
    /// One per column, from the delimiter row.
    pub aligns: Vec<Align>,
    pub head: Vec<TableRow>,
    pub rows: Vec<TableRow>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

#[derive(Clone, Debug, Default)]
pub struct TableCell {
    pub content: Vec<Inline>,
    pub text: ByteSpan,
    /// An `align` attribute from raw HTML. The delimiter row's alignment is in [`Table::aligns`].
    pub align: Align,
}

#[derive(Clone, Debug, Default)]
pub struct Details {
    /// The `open` attribute: the initial state. The reader may toggle it.
    pub open: bool,
    pub summary: Vec<Inline>,
    /// The summary in the flat text.
    pub summary_text: ByteSpan,
    pub blocks: Vec<Block>,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct DefinitionItem {
    pub term: Vec<Inline>,
    pub term_text: ByteSpan,
    pub definitions: Vec<Vec<Block>>,
}

#[derive(Clone, Debug, Default)]
pub struct Footnote {
    pub number: u32,
    /// The element id, `fn:N`.
    pub id: String,
    pub blocks: Vec<Block>,
}

/// A table of contents entry. Only these and the link references outlive the first parse of a
/// large file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingInfo {
    pub level: u8,
    pub text: String,
    pub id: String,
    pub chunk: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positions_order_across_chunks() {
        assert!(DocPos::new(1, 900) < DocPos::new(2, 0));
        assert!(DocPos::new(2, 0) < DocPos::new(2, 1));
    }

    #[test]
    fn spans_are_half_open() {
        let s = ByteSpan::new(2, 5);
        assert!(s.contains(2) && s.contains(4) && !s.contains(5));
        assert_eq!(s.len(), 3);
        assert!(ByteSpan::new(5, 5).is_empty());
    }
}
