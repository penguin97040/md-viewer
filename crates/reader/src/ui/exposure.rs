//! Turns the outline of the document near the view (see [`crate::access`]) into AccessKit nodes.
//!
//! GPUI's builder only appends leaves to the element's own node. A leaf may hold children of its
//! own, though, so every node is pushed flat and then only the top ones are kept as the Document's
//! children. A node listed under two parents panics in AccessKit, so each is listed exactly once.

use std::collections::HashMap;

use gpui::accesskit::{self, Action, Node, NodeId, Role, Toggled};
use gpui::A11ySubtreeBuilder;

use crate::access::{Item, Kind};
use crate::layout::{HitKind, Rect};
use crate::model::LinkTarget;

/// What one chunk contributes this frame. `top` is its position in the window, `left` the pane's.
pub struct ExposedChunk {
    pub chunk: usize,
    pub top: f64,
    pub left: f64,
    pub items: Vec<Item>,
}

/// What a screen reader can ask a node to do.
pub enum Act {
    Hit(usize, HitKind),
    Follow(LinkTarget),
    Reveal(usize, u32),
}

/// The tree for one frame. The Document's element writes it after the reader has prepainted.
#[derive(Default)]
pub struct A11yFrame {
    pub doc_id: u64,
    pub label: String,
    pub chunks: Vec<ExposedChunk>,
    /// The chunk and text offset of the assistive technology's caret, if it is anywhere.
    pub caret: Option<(usize, u32)>,
    /// The vertical range of the view in the window.
    pub view: (f64, f64),
    /// Run nodes of the last tree, with where each is in the document.
    runs: HashMap<NodeId, (usize, u32, String)>,
    actions: Vec<(NodeId, Action, Act)>,
}

fn is_run(i: &Item) -> bool {
    i.kind == Kind::Run
}

fn link_url(t: &LinkTarget) -> String {
    match t {
        LinkTarget::Web(u) => u.clone(),
        LinkTarget::Mail(a) => format!("mailto:{a}"),
        LinkTarget::Fragment(f) => format!("#{f}"),
        LinkTarget::Local { path, fragment: Some(f) } => format!("{path}#{f}"),
        LinkTarget::Local { path, fragment: None } => path.clone(),
    }
}

fn role_of(kind: &Kind) -> Role {
    match kind {
        Kind::Heading(_) => Role::Heading,
        Kind::Paragraph => Role::Paragraph,
        Kind::Quote => Role::Blockquote,
        Kind::List => Role::List,
        Kind::ListItem { .. } => Role::ListItem,
        Kind::Code => Role::Code,
        Kind::Button(_) => Role::Button,
        Kind::Table { .. } => Role::Table,
        Kind::Row(_) => Role::Row,
        Kind::Cell { header: true, .. } => Role::ColumnHeader,
        Kind::Cell { .. } => Role::Cell,
        Kind::Rule => Role::Splitter,
        Kind::Image | Kind::Diagram => Role::Image,
        Kind::Math => Role::Math,
        Kind::Details { .. } => Role::Details,
        Kind::Summary { .. } => Role::DisclosureTriangle,
        Kind::Term => Role::Term,
        Kind::Definition => Role::Definition,
        Kind::Footnotes => Role::DocEndnotes,
        Kind::Footnote => Role::DocFootnote,
        Kind::Group => Role::Group,
        Kind::Link(_) => Role::Link,
        Kind::Run => Role::TextRun,
    }
}

struct Writer<'a, 'b> {
    b: &'a mut A11ySubtreeBuilder<'b>,
    doc_id: u64,
    chunk: usize,
    top: f64,
    left: f64,
    /// Every run's id, in reading order, and whether the next one is on its line.
    run_ids: Vec<(NodeId, bool)>,
    run_at: usize,
    runs: HashMap<NodeId, (usize, u32, String)>,
    actions: Vec<(NodeId, Action, Act)>,
    caret: Option<(usize, u32)>,
    caret_at: Option<accesskit::TextPosition>,
    first_seen: Option<NodeId>,
    view: (f64, f64),
}

impl Writer<'_, '_> {
    fn id(&self, item: &Item, depth: u32) -> NodeId {
        self.b.synthetic_node_id((self.doc_id, self.chunk, item.key.0, item.key.1, depth))
    }

    fn collect_runs(&mut self, item: &Item, depth: u32) {
        if is_run(item) {
            let id = self.id(item, depth);
            self.run_ids.push((id, item.continues_line));
        }
        for c in &item.children {
            self.collect_runs(c, depth + 1);
        }
    }

    fn rect(&self, r: Rect) -> accesskit::Rect {
        let (x0, y0) = (self.left + f64::from(r.x), self.top + f64::from(r.y));
        accesskit::Rect { x0, y0, x1: x0 + f64::from(r.w), y1: y0 + f64::from(r.h) }
    }

    /// Builds a node and everything under it, returning its id if it was accepted.
    fn write(&mut self, item: Item, depth: u32) -> Option<NodeId> {
        let id = self.id(&item, depth);
        let mut node = Node::new(role_of(&item.kind));
        if let Some(r) = item.rect {
            node.set_bounds(self.rect(r));
        }
        if is_run(&item) {
            let at = self.run_at;
            self.run_at += 1;
            if item.continues_line {
                if let Some((next, _)) = self.run_ids.get(at + 1) {
                    node.set_next_on_line(*next);
                }
            }
            if let Some((prev, true)) = at.checked_sub(1).and_then(|p| self.run_ids.get(p)) {
                node.set_previous_on_line(*prev);
            }
            node.set_character_lengths(item.text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<u8>>());
            self.note_caret(id, &item);
            self.runs.insert(id, (self.chunk, item.offset, item.text.clone()));
            node.set_value(item.text);
            return self.b.push_child(id, node).then_some(id);
        }
        match &item.kind {
            Kind::Heading(level) => node.set_level(*level as usize),
            Kind::ListItem { position, size, checked } => {
                node.set_position_in_set(*position);
                node.set_size_of_set(*size);
                if let Some(c) = checked {
                    node.set_toggled(if *c { Toggled::True } else { Toggled::False });
                }
            }
            Kind::Table { rows, columns } => {
                node.set_row_count(*rows);
                node.set_column_count(*columns);
            }
            Kind::Row(r) => node.set_row_index(*r),
            Kind::Cell { row, column, .. } => {
                node.set_row_index(*row);
                node.set_column_index(*column);
            }
            Kind::Details { open } | Kind::Summary { open } => node.set_expanded(*open),
            Kind::Link(t) => {
                node.set_url(link_url(t));
                node.add_action(Action::Click);
                self.actions.push((id, Action::Click, Act::Follow(t.clone())));
            }
            Kind::Button(label) => node.set_label(*label),
            _ => {}
        }
        if !item.text.is_empty() {
            node.set_label(item.text.clone());
        }
        if let Some(h) = item.hit {
            node.add_action(Action::Click);
            self.actions.push((id, Action::Click, Act::Hit(self.chunk, h)));
        }
        node.add_action(Action::ScrollIntoView);
        self.actions.push((id, Action::ScrollIntoView, Act::Reveal(self.chunk, item.offset)));
        let mut kids = Vec::with_capacity(item.children.len());
        for c in item.children {
            if let Some(k) = self.write(c, depth + 1) {
                kids.push(k);
            }
        }
        // The children are pushed flat under the Document too; the Document's list is replaced at
        // the end, so each is listed only here.
        node.set_children(kids);
        self.b.push_child(id, node).then_some(id)
    }

    fn note_caret(&mut self, id: NodeId, item: &Item) {
        let Some(r) = item.rect else { return };
        let (y0, y1) = (self.top + f64::from(r.y), self.top + f64::from(r.bottom()));
        let seen = y1 > self.view.0 && y0 < self.view.1;
        if seen && self.first_seen.is_none() {
            self.first_seen = Some(id);
        }
        if let Some((chunk, byte)) = self.caret {
            let end = item.offset as usize + item.text.len();
            if seen && chunk == self.chunk && byte >= item.offset && (byte as usize) < end {
                let before = &item.text[..(byte - item.offset) as usize];
                self.caret_at = Some(accesskit::TextPosition { node: id, character_index: before.chars().count() });
            }
        }
    }
}

impl A11yFrame {
    /// Writes this frame's nodes under the element's own node.
    pub fn write(&mut self, b: &mut A11ySubtreeBuilder<'_>) {
        if !self.label.is_empty() {
            b.parent_node().set_label(std::mem::take(&mut self.label));
        }
        let chunks = std::mem::take(&mut self.chunks);
        let mut w = Writer {
            b,
            doc_id: self.doc_id,
            chunk: 0,
            top: 0.0,
            left: 0.0,
            run_ids: Vec::new(),
            run_at: 0,
            runs: HashMap::new(),
            actions: Vec::new(),
            caret: self.caret,
            caret_at: None,
            first_seen: None,
            view: self.view,
        };
        for c in &chunks {
            w.chunk = c.chunk;
            for i in &c.items {
                w.collect_runs(i, 0);
            }
        }
        let mut top = Vec::new();
        for c in chunks {
            w.chunk = c.chunk;
            w.top = c.top;
            w.left = c.left;
            for i in c.items {
                if let Some(id) = w.write(i, 0) {
                    top.push(id);
                }
            }
        }
        // A client notes what a node supports when it first sees it, and the Document only has a
        // Text interface while it has a run. So it has one before the file has loaded too.
        if top.is_empty() {
            let id = w.b.synthetic_node_id("empty");
            let mut run = Node::new(Role::TextRun);
            run.set_value(String::new());
            run.set_character_lengths(Vec::<u8>::new());
            if w.b.push_child(id, run) {
                top.push(id);
            }
        }
        let caret = w.caret_at.or(w.first_seen.map(|node| accesskit::TextPosition { node, character_index: 0 }));
        self.runs = w.runs;
        self.actions = w.actions;
        let parent = b.parent_node();
        parent.set_children(top);
        // Without a caret Orca reports offset -1 and moves on its own, so the view never hears
        // where the reader is.
        if let Some(at) = caret {
            parent.set_text_selection(accesskit::TextSelection { anchor: at, focus: at });
        }
    }

    /// The document position of a text position in the last tree.
    pub fn position_of(&self, p: accesskit::TextPosition) -> Option<(usize, u32)> {
        let (chunk, offset, text) = self.runs.get(&p.node)?;
        let bytes: usize = text.chars().take(p.character_index).map(char::len_utf8).sum();
        Some((*chunk, offset + bytes as u32))
    }

    pub fn take_actions(&mut self) -> Vec<(NodeId, Action, Act)> {
        std::mem::take(&mut self.actions)
    }
}
