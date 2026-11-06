//! Small reusable pieces: colours, tooltips, letter-spaced text and the icon button.

use gpui::{
    div, prelude::*, px, rgb, svg, AnyView, App, FontFeatures, Hsla, IntoElement, SharedString,
    Window,
};
use mdv_theme::Palette;
use std::sync::Arc;

use crate::assets::Assets;

pub fn c(v: u32) -> Hsla {
    rgb(v).into()
}

pub fn transparent() -> Hsla {
    gpui::transparent_black()
}

/// `font-variant-numeric: tabular-nums` (OpenType `tnum`).
pub fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".to_string(), 1)]))
}

/// CSS `text-transform: uppercase`. GPUI has no such property, so the string is converted.
pub fn up(s: &str) -> String {
    s.to_uppercase()
}

/// CSS `letter-spacing: <em>em`. GPUI's `TextStyle` has no letter-spacing, so every character
/// becomes its own flex item with a trailing margin. Kerning is lost, which does not matter in a
/// monospaced face. Spaces use a fixed-width gap of half an em plus the spacing.
pub fn spaced(text: &str, size: f32, em: f32) -> impl IntoElement {
    let gap = size * em;
    div().flex().flex_none().children(text.chars().map(move |ch| {
        if ch == ' ' {
            div().w(px(size * 0.5 + gap)).h(px(1.0))
        } else {
            div().mr(px(gap)).child(ch.to_string())
        }
    }))
}

/// A stand-in for the browser's native `title` tooltip.
pub struct Tip {
    pub text: SharedString,
    pub pal: Palette,
}

impl Render for Tip {
    fn render(&mut self, _w: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .mt(px(14.0))
            .bg(c(self.pal.bg_soft))
            .border_1()
            .border_color(c(self.pal.border))
            .text_color(c(self.pal.text))
            .font_family("Geist")
            .text_size(px(12.0))
            .child(self.text.clone())
    }
}

pub fn tooltip(text: &'static str, pal: Palette) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    move |_w, cx| cx.new(|_| Tip { text: text.into(), pal }).into()
}

/// The `<svg>` inside an icon button: `size` px square, inheriting the text colour.
pub fn icon(name: &str, size: f32, color: Hsla) -> gpui::Svg {
    svg().path(Assets::icon(name)).size(px(size)).flex_none().text_color(color)
}
