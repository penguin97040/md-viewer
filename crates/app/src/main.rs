#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod a11y;
mod assets;
#[cfg(feature = "dev-harness")]
mod demo;
mod harness;
mod print;
mod tab_move;
mod ui;
mod timing;
mod view;

use std::borrow::Cow;

use gpui::{px, size, App, Bounds};
use gpui_platform::application;
use mdv_platform::single_instance::{self, Launch};

fn main() {
    mdv_reader::ui::crash::install_hook(mdv_platform::show_fatal_error);
    timing::log("main_entered");
    let (w, h) = harness::window_size();

    // Later launches pass their files to the first process and stop here, before any window.
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let owner = if harness::no_single_instance() {
        None
    } else {
        match single_instance::start(&files) {
            Ok(Launch::HandedOff) => return,
            Ok(Launch::Owner(owner)) => Some(owner),
            Err(e) => {
                eprintln!("md-viewer: running on its own: {e}");
                None
            }
        }
    };

    application().with_assets(assets::Assets).run(move |cx: &mut App| {
        // Greyscale text: far easier to read in dark mode on a QD-OLED panel.
        cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(assets::GEIST_REGULAR),
                Cow::Borrowed(assets::GEIST_SEMIBOLD),
                Cow::Borrowed(assets::GEIST_MONO_REGULAR),
            ])
            .unwrap();
        mdv_reader::ui::init(cx);
        tab_move::install(cx);

        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        tab_move::open_viewer(tab_move::viewer_options(bounds), move |v, cx| {
            for f in &files {
                v.open_path(f.clone(), None, cx);
            }
            #[cfg(feature = "dev-harness")]
            demo::apply(v);
        }, cx)
        .unwrap();
        timing::log("window_created");
        tab_move::prepare_ghost_soon(cx);
        if let Some(owner) = owner {
            tab_move::serve_launches(owner, cx);
        }
        cx.activate(true);
    });
}
