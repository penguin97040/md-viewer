#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::{prelude::*, px, rgb, size, App, Bounds, Context, Window, WindowBounds, WindowOptions, div};
use gpui_platform::application;
use std::borrow::Cow;

const CHAKRA: &[u8] = include_bytes!("../../../../frontend/fonts/ChakraPetch-Regular.ttf");

struct Hello;

impl Render for Hello {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .font_family("Chakra Petch")
            .bg(rgb(0x000000))
            .text_color(rgb(0xf2f2f2))
            .border_1()
            .border_color(rgb(0x2c2c2c))
            .flex()
            .items_center()
            .justify_center()
            .child("MD VIEWER (GPUI SPIKE)")
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.text_system().add_fonts(vec![Cow::Borrowed(CHAKRA)]).unwrap();
        let bounds = Bounds::centered(None, size(px(640.), px(400.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Hello),
        )
        .unwrap();
        cx.activate(true);
    });
}
