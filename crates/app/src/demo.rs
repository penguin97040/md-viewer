//! Chrome states for screenshots and layout checks: `MDV_DEMO` takes a comma list. Only
//! with the `dev-harness` feature.

use mdv_theme::Mode;

use crate::view::AppView;

pub fn apply(v: &mut AppView) {
    let demo = std::env::var("MDV_DEMO").unwrap_or_default();
    let has = |k: &str| demo.split(',').any(|d| d == k);
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
}
