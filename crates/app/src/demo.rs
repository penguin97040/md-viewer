//! The chrome on stub data, for screenshots and layout checks: `MDV_DEMO` takes a comma list. Only
//! with the `dev-harness` feature.

use std::rc::Rc;

use mdv_theme::Mode;

use crate::view::{AppView, Heading, TabData};

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
    (0..n).map(|i| heading(1 + (i % 3) as u8, &format!("Heading number {i}"))).collect()
}

pub fn apply(v: &mut AppView) {
    let demo = std::env::var("MDV_DEMO").unwrap_or_default();
    let has = |k: &str| demo.split(',').any(|d| d == k);
    if has("light") {
        v.mode = Mode::Light;
    }
    if has("tabs") || has("toc100k") {
        let headings = if has("toc100k") { Rc::new(huge(100_000)) } else { Rc::new(kepler()) };
        v.tabs = vec![
            TabData::new("kepler-track.md".into(), "kepler-track.md".into(), headings),
            TabData::new("gear-list.md".into(), "gear-list.md".into(), Rc::new(gear())),
        ];
        v.current = if has("toc100k") { 50_000 } else { 2 };
        if has("toc100k") {
            v.toc_scroll.scroll_to_item_strict(50_000, gpui::ScrollStrategy::Center);
        }
    }
    // Extra stub tabs by file name, for movement and overflow tests.
    if let Ok(names) = std::env::var("MDV_TABS") {
        for name in names.split(',') {
            v.tabs.push(TabData::new(name.to_string().into(), name.to_string().into(), Rc::new(kepler())));
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
}
