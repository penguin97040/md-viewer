//! Accessibility helpers, and a sample document showing how the reader could expose virtualised text.
//!
//! GPUI (through AccessKit) only reports elements with both an id and a role. Plain strings passed to
//! `.child()` are invisible to assistive technology, so anything to be read needs a node of its own.
//!
//! The document sample exposes `Document > Paragraph or Heading > TextRun` for a window of paragraphs
//! around the viewport, like `viewer.js` keeps chunks mounted for two screens either way. Each is a
//! real zero-size element: the synthetic-children builder only appends leaves to the current node, so
//! nesting has to come from elements. A node listed under two parents panics in AccessKit, in release
//! builds too, so the reader must keep its tree well formed. `ScrollIntoView` on a node at the edge of
//! the window scrolls the view, and the window moves.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::accesskit::{self, Action, Live, Node, Role};
use gpui::{
    div, prelude::*, px, uniform_list, A11ySubtreeBuilder, Context, ElementId, Hsla, ScrollStrategy,
    UniformListScrollHandle,
};

/// Marks the element as a polite live region, like `<output aria-live="polite">`.
pub fn live_polite(b: &mut A11ySubtreeBuilder<'_>) {
    b.parent_node().set_live(Live::Polite);
}

pub fn disabled(b: &mut A11ySubtreeBuilder<'_>) {
    b.parent_node().set_disabled();
}

fn char_lengths(text: &str) -> Vec<u8> {
    text.chars().map(|c| c.len_utf8() as u8).collect()
}

/// A text input's content as a text run with the caret at its end, so that AT-SPI's Text interface
/// and a screen reader's echo have something to read.
pub fn input_text(text: String) -> impl FnOnce(&mut A11ySubtreeBuilder<'_>) + 'static {
    move |b| {
        let id = b.synthetic_node_id("run");
        let mut run = Node::new(Role::TextRun);
        let caret = text.chars().count();
        run.set_character_lengths(char_lengths(&text));
        run.set_value(text);
        b.push_child(id, run);
        let at = accesskit::TextPosition { node: id, character_index: caret };
        b.parent_node().set_text_selection(accesskit::TextSelection { anchor: at, focus: at });
    }
}

// ---- the document sample (MDV_A11Y_DOC=<paragraphs>) -------------------------------------------

const ROW: f32 = 28.0;

pub struct DocSample {
    pub paragraphs: usize,
    /// Paragraphs exposed either side of the viewport (`MDV_A11Y_OVERSCAN`).
    pub overscan: usize,
    pub scroll: UniformListScrollHandle,
    pub visible: Rc<RefCell<Range<usize>>>,
}

impl DocSample {
    pub fn from_env() -> Option<Self> {
        let paragraphs = std::env::var("MDV_A11Y_DOC").ok()?.parse().ok()?;
        let overscan = std::env::var("MDV_A11Y_OVERSCAN").ok().and_then(|v| v.parse().ok()).unwrap_or(40);
        Some(Self { paragraphs, overscan, scroll: UniformListScrollHandle::new(), visible: Rc::default() })
    }
}

fn is_heading(i: usize) -> bool {
    i % 25 == 0
}

fn paragraph_text(i: usize) -> String {
    if is_heading(i) {
        format!("Section {}", i / 25 + 1)
    } else {
        format!("Paragraph {i}: Tāwhaki walked the Kepler Track past Luxmore Hut and on along the ridge.")
    }
}

pub fn doc_sample<V: 'static + Render>(sample: &DocSample, text: Hsla, cx: &mut Context<V>) -> impl IntoElement {
    let n = sample.paragraphs;
    let scroll = sample.scroll.clone();
    let visible = sample.visible.clone();
    let shown = visible.clone();
    let list = uniform_list(
        "doc-rows",
        n,
        cx.processor(move |_, range: Range<usize>, window, _| {
            // The list learns its range after the blocks are built, so a change needs one more frame.
            if *shown.borrow() != range {
                *shown.borrow_mut() = range.clone();
                window.request_animation_frame();
            }
            range.map(|i| div().h(px(ROW)).w_full().truncate().text_color(text).child(paragraph_text(i))).collect::<Vec<_>>()
        }),
    )
    .track_scroll(&scroll)
    .flex_1()
    .h_full();

    let seen = visible.borrow().clone();
    let first = seen.start.saturating_sub(sample.overscan);
    let last = (seen.end + sample.overscan).min(n);
    let blocks = (first..last).map(|i| {
        let body = paragraph_text(i);
        let scroll = scroll.clone();
        div()
            .id(ElementId::NamedInteger("block".into(), i as u64))
            .role(if is_heading(i) { Role::Heading } else { Role::Paragraph })
            .when(is_heading(i), |d| d.aria_level(2))
            .absolute()
            .size_0()
            .on_a11y_action(Action::ScrollIntoView, move |_, window, _| {
                scroll.scroll_to_item_strict(i, ScrollStrategy::Top);
                window.refresh();
            })
            .a11y_synthetic_children(move |b| {
                let top = i as f64 * ROW as f64;
                let rect = accesskit::Rect { x0: 0.0, y0: top, x1: 800.0, y1: top + ROW as f64 };
                b.parent_node().set_bounds(rect);
                let mut run = Node::new(Role::TextRun);
                run.set_bounds(rect);
                run.set_character_lengths(char_lengths(&body));
                run.set_value(body);
                let id = b.synthetic_node_id("run");
                b.push_child(id, run);
            })
    });

    div()
        .id("doc")
        .role(Role::Document)
        .aria_label("Document")
        .focusable()
        .tab_stop(true)
        .relative()
        .flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .child(list)
        .children(blocks)
}
