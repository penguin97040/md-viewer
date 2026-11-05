#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod assets;
mod ui;
mod view;

use std::borrow::Cow;
use std::rc::Rc;

use gpui::{AppContext as _,
    point, px, size, App, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions,
};
use gpui_platform::application;
use mdv_theme::Mode;
use view::{AppView, Heading, TabData};

fn heading(level: u8, text: &str) -> Heading {
    Heading { level, text: text.to_string().into() }
}

/// The outline shown in docs/screenshots/dark.png.
fn kepler() -> Vec<Heading> {
    vec![
        heading(1, "Kepler Track trip plan"),
        heading(2, "Route overview"),
        heading(2, "Timings"),
        heading(2, "Bookings"),
        heading(2, "Day by day"),
        heading(3, "Day 1: up to Luxmore Hut"),
        heading(3, "Day 2: the alpine ridge"),
        heading(3, "Day 3: Iris Burn to Moturau"),
        heading(3, "Day 4: out to Rainbow Reach"),
        heading(2, "Emergency"),
    ]
}

fn gear() -> Vec<Heading> {
    vec![
        heading(1, "Gear list"),
        heading(2, "Clothing"),
        heading(2, "Shelter and sleep"),
        heading(2, "Food and cooking"),
        heading(3, "Breakfast"),
        heading(3, "Dinner"),
    ]
}

/// 100,000 headings for the virtualisation test (`MDV_DEMO=toc100k`).
fn huge(n: usize) -> Vec<Heading> {
    (0..n)
        .map(|i| {
            let level = if i % 1000 == 0 { 1 } else if i % 100 == 0 { 2 } else { 3 };
            heading(level, &format!("Heading {i}: a reasonably long section title for row {i}"))
        })
        .collect()
}

fn main() {
    let demo = std::env::var("MDV_DEMO").unwrap_or_default();
    let has = move |k: &str| demo.split(',').any(|d| d == k);
    let (w, h) = std::env::var("MDV_SIZE")
        .ok()
        .and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1280.0), b.parse().unwrap_or(800.0))))
        .unwrap_or((1280.0_f32, 800.0_f32));

    application().with_assets(assets::Assets).run(move |cx: &mut App| {
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(assets::CHAKRA_REGULAR),
                Cow::Borrowed(assets::CHAKRA_SEMIBOLD),
                Cow::Borrowed(assets::SHARE_TECH_MONO),
            ])
            .unwrap();
        mdv_reader::ui::init(cx);
        let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();

        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        let mut options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_background: WindowBackgroundAppearance::Opaque,
            // Linux (X11 and Wayland): draw our own frame. Windows and macOS use `titlebar` below.
            window_decorations: Some(WindowDecorations::Client),
            window_min_size: Some(size(px(320.0), px(240.0))),
            app_id: Some("md-viewer".into()),
            ..Default::default()
        };
        // Windows and macOS: no system title bar; the app draws the header. On macOS the native
        // traffic lights stay, placed inside the 88px left inset. Position is a guess (untested).
        options.titlebar = Some(TitlebarOptions {
            title: Some("MD Viewer".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.0), px(17.0))),
        });

        cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| {
                let mut v = AppView::new(cx);
                for f in &files {
                    v.open_path(f.clone(), None, cx);
                }
                if has("light") {
                    v.mode = Mode::Light;
                }
                if has("tabs") || has("toc100k") {
                    let headings = if has("toc100k") { Rc::new(huge(100_000)) } else { Rc::new(kepler()) };
                    v.tabs = vec![
                        TabData { name: "kepler-track.md".into(), path: "kepler-track.md".into(), headings, reader: None },
                        TabData { name: "gear-list.md".into(), path: "gear-list.md".into(), headings: Rc::new(gear()), reader: None },
                    ];
                    v.current = if has("toc100k") { 50_000 } else { 2 };
                    if has("toc100k") {
                        v.toc_scroll.scroll_to_item_strict(50_000, gpui::ScrollStrategy::Center);
                    }
                }
                if has("find") {
                    v.find_open = true;
                    v.find_field.set("day");
                    v.find_focus = true;
                }
                if has("settings") {
                    v.settings_open = true;
                }
                if has("tools") {
                    v.tools_open = true;
                }
                if has("web") {
                    v.web_bar = true;
                }
                if has("toast") {
                    v.toast = Some(("Preparing PDF… 40%".into(), true));
                }
                v
            });
            let focus = view.read(cx).root_focus.clone();
            window.focus(&focus, cx);
            view
        })
        .unwrap();
        cx.activate(true);
    });
}
