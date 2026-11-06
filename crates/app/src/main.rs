#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod a11y;
mod assets;
mod print;
mod settings;
mod tab_move;
mod timing;
mod ui;
mod view;
mod watch;

use std::borrow::Cow;
use std::path::PathBuf;

use gpui::{px, size, App, Bounds};
use gpui_platform::application;
use mdv_platform::single_instance::{self, Launch};
use mdv_theme::Mode;

/// The path in a `file:` URL, which is how macOS hands over a document opened from the Finder.
/// Other schemes, and files on another host, are refused.
fn file_url_path(url: &str) -> Option<PathBuf> {
    let rest = url.get(..7).filter(|s| s.eq_ignore_ascii_case("file://")).map(|_| &url[7..])?;
    let rest = rest.split(['?', '#']).next()?;
    let (host, path) = rest.split_at(rest.find('/')?);
    if !(host.is_empty() || host.eq_ignore_ascii_case("localhost")) {
        return None;
    }
    let mut bytes = Vec::with_capacity(path.len());
    let raw = path.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' {
            let hex = std::str::from_utf8(raw.get(i + 1..i + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(raw[i]);
            i += 1;
        }
    }
    let text = String::from_utf8(bytes).ok().filter(|t| !t.contains('\0'))?;
    // `file:///C:/x` carries the drive after the slash.
    let text = if cfg!(windows) { text.strip_prefix('/').map_or(text.clone(), str::to_string) } else { text };
    Some(PathBuf::from(text))
}

fn main() {
    timing::log("main_entered");
    let demo = std::env::var("MDV_DEMO").unwrap_or_default();
    let has = move |k: &str| demo.split(',').any(|d| d == k);
    let (w, h) = std::env::var("MDV_SIZE")
        .ok()
        .and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1280.0), b.parse().unwrap_or(800.0))))
        .unwrap_or((1280.0_f32, 800.0_f32));

    // Later launches pass their files to the first process and stop here, before any window.
    let files: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    let owner = if std::env::var_os("MDV_NO_SINGLE_INSTANCE").is_some() {
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

    // The system hands over documents opened from the Finder (macOS) here, both at launch and while
    // the app runs. They wait in the channel until the window exists.
    let (open_tx, open_rx) = async_channel::unbounded::<Vec<String>>();
    let app = application().with_assets(assets::Assets);
    app.on_open_urls(move |urls| {
        let _ = open_tx.try_send(urls);
    });
    app.run(move |cx: &mut App| {
        // Greyscale text by default: the owner found it far easier to read in dark mode on a
        // QD-OLED panel. MDV_TEXT=system keeps the platform's own smoothing, for comparison.
        if std::env::var("MDV_TEXT").as_deref() != Ok("system") {
            cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
        }
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(assets::GEIST_REGULAR),
                Cow::Borrowed(assets::GEIST_SEMIBOLD),
                Cow::Borrowed(assets::GEIST_MONO_REGULAR),
            ])
            .unwrap();
        mdv_reader::ui::init(cx);
        settings::install(cx);
        tab_move::install(cx);

        let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
        tab_move::open_viewer(tab_move::viewer_options(bounds), move |v, cx| {
            for f in &files {
                v.open_path(f.clone(), None, cx);
            }
            if has("light") {
                v.mode = Mode::Light;
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
        }, cx)
        .unwrap();
        timing::log("window_created");
        tab_move::prepare_ghost_soon(cx);
        if let Some(owner) = owner {
            tab_move::serve_launches(owner, cx);
        }
        cx.spawn(async move |cx| {
            while let Ok(urls) = open_rx.recv().await {
                let paths: Vec<PathBuf> = urls.iter().filter_map(|u| file_url_path(u)).collect();
                cx.update(|cx| {
                    if !paths.is_empty() && tab_move::open_in_front(&paths, cx) {
                        cx.activate(true);
                    }
                });
            }
        })
        .detach();
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(file_url_path("file:///Users/a/My%20Notes%20%C3%A9.md"), Some(PathBuf::from("/Users/a/My Notes é.md")));
        assert_eq!(file_url_path("FILE://localhost/tmp/a.md"), Some(PathBuf::from("/tmp/a.md")));
        assert_eq!(file_url_path("file:///tmp/a.md#top"), Some(PathBuf::from("/tmp/a.md")));
        assert_eq!(file_url_path("file://server/share/a.md"), None);
        assert_eq!(file_url_path("https://example.com/a.md"), None);
        assert_eq!(file_url_path("file:///tmp/a%00.md"), None);
        assert_eq!(file_url_path("file:///tmp/a%zz.md"), None);
        assert_eq!(file_url_path("file:///tmp/%ff.md"), None);
        assert_eq!(file_url_path("file://"), None);
    }
}
