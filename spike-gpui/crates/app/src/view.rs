//! The window chrome: header, tabs, TOC, find bar, web-images bar, toast, empty state and the
//! settings popover. Specification: frontend/index.html, style.css, windowbar.js, toc.js, tabs.js.

use std::collections::HashMap;
use std::time::Instant;

use gpui::{
    accesskit::{ActionData, Toggled}, canvas, div, point, prelude::*, px, relative, uniform_list,
    AccessibleAction, Bounds, Context, CursorStyle, Role, ScrollStrategy,
    Decorations, Div, FocusHandle, FontWeight, HitboxBehavior, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ResizeEdge, ScrollHandle,
    SharedString, Stateful, UniformListScrollHandle, Window, WindowControlArea,
};
use mdv_theme::{metrics as m, Mode};

use crate::a11y;
use crate::assets;
use crate::tab_move::{self, TabId, TabState};
use crate::ui::{c, icon, spaced, tabular, tooltip, transparent, up};

pub const VERSION: &str = "0.2.6";
const MONO: &str = "Share Tech Mono";
const SANS: &str = "Chakra Petch";
const SERIF: &str = "Georgia"; // CSS --serif stack starts with Charter; first installed face is used

#[derive(Clone)]
pub struct Heading {
    pub level: u8,
    pub text: SharedString,
}

pub struct TabData {
    pub id: TabId,
    pub name: SharedString,
    pub path: SharedString,
    pub headings: std::rc::Rc<Vec<Heading>>,
    /// Follows the tab between windows. Add per-tab reader state here, not to `AppView`.
    pub state: TabState,
}

impl TabData {
    pub fn new(name: SharedString, path: SharedString, headings: std::rc::Rc<Vec<Heading>>) -> Self {
        Self { id: tab_move::next_tab_id(), name, path, headings, state: TabState::default() }
    }
}

/// The insertion marker on a strip, in window coordinates (`.tab-insertion`).
#[derive(Clone, Copy)]
pub struct DropMarker {
    pub x: f32,
    pub top: f32,
    pub height: f32,
}

#[derive(Clone, Copy)]
pub struct Settings {
    pub serif: bool,
    pub padding: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub wrap: bool,
    pub live_reload: bool,
}

impl Settings {
    pub const DEFAULTS: Settings = Settings {
        serif: false,
        padding: 20.0,
        font_size: 17.0,
        line_height: 1.65,
        wrap: true,
        live_reload: true,
    };
}

#[derive(Clone, Copy, PartialEq)]
enum StepKey {
    Padding,
    FontSize,
    LineHeight,
}

impl StepKey {
    fn range(self) -> (f32, f32, f32) {
        match self {
            StepKey::Padding => (20.0, 40.0, 4.0),
            StepKey::FontSize => (12.0, 28.0, 1.0),
            StepKey::LineHeight => (1.2, 2.2, 0.05),
        }
    }

    fn name(self) -> &'static str {
        match self {
            StepKey::Padding => "Padding",
            StepKey::FontSize => "Font size",
            StepKey::LineHeight => "Line spacing",
        }
    }
}

pub struct AppView {
    pub mode: Mode,
    pub mac: bool,
    pub tabs: Vec<TabData>,
    pub active: usize,
    pub toc_open: bool,
    pub find_open: bool,
    pub find_text: String,
    pub find_pos: usize,
    pub web_bar: bool,
    pub toast: Option<(String, bool)>,
    pub settings_open: bool,
    pub tools_open: bool,
    pub licence: Option<&'static str>,
    pub s: Settings,
    pub toc_scroll: UniformListScrollHandle,
    pub current: usize,
    pub root_focus: FocusHandle,
    focus: HashMap<&'static str, FocusHandle>,
    drag_thumb: Option<(f32, f32)>,
    hover: Option<(&'static str, usize)>,
    should_move: bool,
    pub perf: bool,
    pub started: Instant,
    first_render_logged: bool,
    // tab movement: see tab_move.rs
    pub tab_scroll: ScrollHandle,
    pub drop_marker: Option<DropMarker>,
    pub moving: Option<TabId>,
    tab_focus: HashMap<TabId, FocusHandle>,
    // accessibility: see a11y.rs
    doc_sample: Option<a11y::DocSample>,
    title: String,
}

impl AppView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            mode: Mode::Dark,
            mac: cfg!(target_os = "macos") || std::env::var("MDV_PLATFORM").as_deref() == Ok("mac"),
            tabs: Vec::new(),
            active: 0,
            toc_open: true,
            find_open: false,
            find_text: String::new(),
            find_pos: 0,
            web_bar: false,
            toast: None,
            settings_open: false,
            tools_open: false,
            licence: None,
            s: Settings::DEFAULTS,
            toc_scroll: UniformListScrollHandle::new(),
            current: 0,
            root_focus: cx.focus_handle(),
            focus: HashMap::new(),
            drag_thumb: None,
            hover: None,
            should_move: false,
            perf: std::env::var("MDV_PERF").is_ok(),
            started: Instant::now(),
            first_render_logged: false,
            tab_scroll: ScrollHandle::new(),
            drop_marker: None,
            moving: None,
            tab_focus: HashMap::new(),
            doc_sample: a11y::DocSample::from_env(),
            title: String::new(),
        }
    }

    // ---- tab state (tab movement: see tab_move.rs) -------------------------------------------

    /// Copies the view's per-tab fields into the active tab, so they travel with it.
    fn save_active_state(&mut self) {
        let (current, offset, web) = (self.current, self.toc_scroll.0.borrow().base_handle.offset().y, self.web_bar);
        if let Some(tab) = self.tabs.get_mut(self.active) {
            tab.state.current_heading = current;
            tab.state.toc_offset = offset;
            tab.state.web_bar = web;
        }
    }

    /// The reverse of `save_active_state`, after `active` changed.
    fn load_active_state(&mut self) {
        let state = self.tabs.get(self.active).map(|t| (t.state.current_heading, t.state.toc_offset, t.state.web_bar));
        let (current, offset, web) = state.unwrap_or((0, px(0.0), false));
        self.current = current;
        self.web_bar = web;
        let handle = self.toc_scroll.0.borrow().base_handle.clone();
        let mut o = handle.offset();
        o.y = offset;
        handle.set_offset(o);
    }

    fn select_index(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.tabs.len() && i != self.active {
            self.save_active_state();
            self.active = i;
            self.load_active_state();
            self.tab_scroll.scroll_to_item(i);
            cx.notify();
        }
    }

    pub fn activate_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.select_index(i, cx);
        }
    }

    /// Moves a tab within this strip. `at` is an insertion index with the tab still in place. The
    /// same tab stays active and the strip's order is the keyboard order.
    pub fn reorder_tab(&mut self, id: TabId, at: usize) -> bool {
        let Some(from) = self.tabs.iter().position(|t| t.id == id) else { return false };
        let active = self.tabs.get(self.active).map(|t| t.id);
        let moved = mdv_platform::order::reorder(&mut self.tabs, from, at);
        if let Some(i) = active.and_then(|a| self.tabs.iter().position(|t| t.id == a)) {
            self.active = i;
        }
        moved
    }

    /// Takes a tab out of this strip for moving elsewhere, with its current state.
    pub fn take_tab(&mut self, id: TabId, cx: &mut Context<Self>) -> Option<TabData> {
        self.save_active_state();
        let i = self.tabs.iter().position(|t| t.id == id)?;
        let data = self.tabs.remove(i);
        self.tab_focus.remove(&id);
        self.active = mdv_platform::order::active_after_removal(self.active, i, self.tabs.len());
        self.load_active_state();
        cx.notify();
        Some(data)
    }

    /// Adds a tab at insertion index `at` and makes it active, restoring its state.
    pub fn insert_tab(&mut self, data: TabData, at: usize, cx: &mut Context<Self>) {
        self.save_active_state();
        let at = at.min(self.tabs.len());
        self.tabs.insert(at, data);
        self.active = at;
        self.load_active_state();
        self.tab_scroll.scroll_to_item(at);
        cx.notify();
    }

    fn close_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            let was_active = i == self.active;
            if !was_active {
                self.save_active_state();
            }
            self.tabs.remove(i);
            self.tab_focus.remove(&id);
            self.active = mdv_platform::order::active_after_removal(self.active, i, self.tabs.len());
            self.load_active_state();
            cx.notify();
            cx.defer(|cx| tab_move::log_state("close-tab", "", cx));
        }
    }

    fn fh(&mut self, id: &'static str, cx: &mut Context<Self>) -> FocusHandle {
        self.focus.entry(id).or_insert_with(|| cx.focus_handle().tab_stop(true)).clone()
    }

    fn headings(&self) -> std::rc::Rc<Vec<Heading>> {
        self.tabs.get(self.active).map(|t| t.headings.clone()).unwrap_or_default()
    }

    /// Hard-coded match count for the demo find bar: occurrences in the heading text.
    fn find_total(&self) -> usize {
        if self.find_text.is_empty() {
            return 0;
        }
        let q = self.find_text.to_lowercase();
        self.headings().iter().take(2000).map(|h| h.text.to_lowercase().matches(&q).count()).sum()
    }

    pub fn show_toast(&mut self, msg: &str, cancel: bool, cx: &mut Context<Self>) {
        self.toast = Some((msg.to_string(), cancel));
        let handle = cx.entity();
        if !cancel {
            cx.spawn(async move |_, cx| {
                cx.background_executor().timer(std::time::Duration::from_secs(4)).await;
                handle.update(cx, |this, cx| {
                    this.toast = None;
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    fn close_popups(&mut self) {
        self.settings_open = false;
        self.tools_open = false;
    }

    fn step(&mut self, key: StepKey, dir: f32) {
        let (_, _, step) = key.range();
        self.set_step_value(key, self.step_value(key) + dir * step);
    }

    fn step_value(&self, key: StepKey) -> f32 {
        match key {
            StepKey::Padding => self.s.padding,
            StepKey::FontSize => self.s.font_size,
            StepKey::LineHeight => self.s.line_height,
        }
    }

    fn set_step_value(&mut self, key: StepKey, value: f32) {
        let (lo, hi, _) = key.range();
        let slot = match key {
            StepKey::Padding => &mut self.s.padding,
            StepKey::FontSize => &mut self.s.font_size,
            StepKey::LineHeight => &mut self.s.line_height,
        };
        *slot = ((value * 100.0).round() / 100.0).clamp(lo, hi);
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = e.keystroke.key.as_str();
        let md = &e.keystroke.modifiers;
        let modkey = if self.mac { md.platform } else { md.control };
        if modkey {
            match k {
                "f" => self.open_find(window, cx),
                "b" => self.toc_open = !self.toc_open,
                "o" => {
                    if !tab_move::open_next_from_queue(self, cx) {
                        self.show_toast("Opening files is not part of this spike", false, cx)
                    }
                }
                // tab keys: order follows the strip (README shortcut table)
                "w" => {
                    if let Some(id) = self.tabs.get(self.active).map(|t| t.id) {
                        self.close_tab(id, cx);
                    }
                }
                "tab" if md.control => self.next_tab(if md.shift { -1 } else { 1 }, cx),
                "pagedown" => self.next_tab(1, cx),
                "pageup" => self.next_tab(-1, cx),
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" if !md.shift => {
                    let n = self.tabs.len();
                    let i = if k == "9" { n.saturating_sub(1) } else { k.parse::<usize>().unwrap_or(1) - 1 };
                    self.select_index(i, cx);
                }
                "p" => self.show_toast("Printing is not part of this spike", false, cx),
                "r" => self.show_toast("Reloading is not part of this spike", false, cx),
                "=" | "+" => self.step(StepKey::FontSize, 1.0),
                "-" => self.step(StepKey::FontSize, -1.0),
                "0" => self.s.font_size = Settings::DEFAULTS.font_size,
                "d" if md.shift => self.mode = if self.mode == Mode::Dark { Mode::Light } else { Mode::Dark },
                _ => return,
            }
            cx.notify();
            return;
        }
        match k {
            "tab" => {
                if md.shift {
                    window.focus_prev(cx)
                } else {
                    window.focus_next(cx)
                }
            }
            "escape" => {
                if self.licence.is_some() {
                    self.licence = None;
                } else if self.tools_open {
                    self.tools_open = false;
                } else if self.settings_open {
                    self.settings_open = false;
                } else if self.find_open {
                    self.close_find(window, cx);
                }
            }
            "f5" => self.show_toast("Reloading is not part of this spike", false, cx),
            "f3" => {
                let total = self.find_total();
                if !self.find_open {
                    self.open_find(window, cx);
                } else if total > 0 {
                    self.find_pos = if md.shift { (self.find_pos + total - 1) % total } else { (self.find_pos + 1) % total };
                }
            }
            _ if self.find_open => match k {
                "enter" => {
                    let total = self.find_total();
                    if total > 0 {
                        self.find_pos = if md.shift { (self.find_pos + total - 1) % total } else { (self.find_pos + 1) % total };
                    }
                }
                "backspace" => {
                    self.find_text.pop();
                    self.find_pos = 0;
                }
                _ => match &e.keystroke.key_char {
                    Some(ch) if !md.control && !md.platform => {
                        self.find_text.push_str(ch);
                        self.find_pos = 0;
                    }
                    _ => return,
                },
            },
            _ => return,
        }
        cx.notify();
    }

    fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = true;
        self.close_popups();
        let input = self.fh("find-input", cx);
        window.focus(&input, cx);
    }

    fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.find_open = false;
        window.focus(&self.root_focus, cx);
    }

    fn next_tab(&mut self, dir: isize, cx: &mut Context<Self>) {
        let n = self.tabs.len() as isize;
        if n > 1 {
            let i = (self.active as isize + dir).rem_euclid(n) as usize;
            self.select_index(i, cx);
        }
    }

    // ---- building blocks -------------------------------------------------------------------

    /// `.icon` (and `.icon.small`): a borderless button with an SVG, muted until hovered.
    #[allow(clippy::too_many_arguments)]
    fn icon_btn(
        &mut self,
        id: &'static str,
        name: &'static str,
        tip: &'static str,
        w: f32,
        h: f32,
        svg: f32,
        on: bool,
        cx: &mut Context<Self>,
        click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let p = *self.mode.palette();
        let fh = self.fh(id, cx);
        let hovered = self.hover == Some((id, 0));
        let hover_fg = if id == "win-close" { p.bg } else { p.text };
        let fg = if on { p.accent } else if hovered { hover_fg } else { p.muted };
        let (a11y_name, a11y_keys) = tip.split_once(" (").map_or((tip, None), |(n, k)| (n, Some(k.trim_end_matches(')'))));
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(a11y_name)
            .when_some(a11y_keys, |d, k| d.aria_keyshortcuts(k))
            .track_focus(&fh)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w(px(w))
            .h(px(h))
            .text_color(c(fg))
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                if *h {
                    this.hover = Some((id, 0));
                } else if this.hover == Some((id, 0)) {
                    this.hover = None;
                }
                cx.notify();
            }))
            .cursor_pointer()
            // One hover style per element: a second call panics in debug builds.
            .hover(move |s| {
                if id == "win-close" {
                    s.bg(c(p.accent)).text_color(c(p.bg))
                } else {
                    s.bg(c(p.bg_hover)).text_color(c(if on { p.accent } else { p.text }))
                }
            })
            .focus_visible(move |s| s.border_2().border_color(c(p.accent)))
            .tooltip(tooltip(tip, p))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                click(this, window, cx);
                cx.notify();
            }))
            .child(icon(name, svg, c(fg)))
    }

    fn link(&mut self, id: &'static str, label: &'static str, cx: &mut Context<Self>, click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static) -> Stateful<Div> {
        let p = *self.mode.palette();
        let fh = self.fh(id, cx);
        div()
            .id(id)
            .role(Role::Button)
            .aria_label(label)
            .track_focus(&fh)
            .text_color(c(p.accent))
            .text_size(px(12.0))
            .cursor_pointer()
            .focus_visible(move |s| s.border_2().border_color(c(p.accent)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, w, cx| {
                click(this, w, cx);
                cx.notify();
            }))
            .child(up(label))
    }

    fn header(&mut self, narrow: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let mac = self.mac;
        let icon_w = if narrow { 30.0 } else { m::ICON_W };
        let tabs: Vec<(TabId, SharedString)> = self.tabs.iter().map(|t| (t.id, t.name.clone())).collect();
        let moving = self.moving;
        let active = self.active;
        let maximised = false;

        let title = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(if narrow { 0.0 } else { 8.0 }))
            .pl(px(4.0))
            .pr(px(if narrow { 6.0 } else { 12.0 }))
            .window_control_area(WindowControlArea::Drag)
            .child(gpui::img(self.app_icon()).w(px(22.0)).h(px(22.0)).flex_none())
            .when(!narrow, |d| {
                d.font_family(MONO)
                    .text_size(px(12.0))
                    .line_height(relative(1.0))
                    .child(spaced(&up("MD Viewer"), 12.0, 0.08))
            });

        let mut strip = div()
            .id("tab-strip")
            .role(Role::TabList)
            .aria_label("Open files")
            .track_scroll(&self.tab_scroll)
            .flex()
            .flex_1()
            .min_w_0()
            .items_stretch()
            .self_stretch()
            .overflow_x_scroll()
            .pt(px(6.0))
            .pb(px(4.0))
            .px(px(6.0));
        for (i, (tab_id, name)) in tabs.iter().enumerate() {
            let tab_id = *tab_id;
            let is_active = i == active;
            let tab_hover = self.hover == Some(("tab", i));
            let x_fg = if is_active { p.text_strong } else if tab_hover { p.text } else { p.muted };
            let name = name.clone();
            let close_label: SharedString = format!("Close tab {name}").into();
            let fh = self.tab_focus.entry(tab_id).or_insert_with(|| cx.focus_handle().tab_stop(true)).clone();
            strip = strip.child(
                div()
                    .id(("tab", tab_id))
                    .role(Role::Tab)
                    .aria_label(name.clone())
                    .aria_selected(is_active)
                    .aria_position_in_set(i + 1)
                    .aria_size_of_set(tabs.len())
                    .group("tab")
                    .track_focus(&fh)
                    .flex()
                    .flex_initial()
                    .items_center()
                    .gap(px(2.0))
                    .w(px(m::TAB_BASIS))
                    .min_w(px(if narrow { 80.0 } else { m::TAB_MIN }))
                    .pl(px(if narrow { 8.0 } else { 12.0 }))
                    .pr(px(4.0))
                    .border_b_2()
                    .border_color(if is_active { c(p.accent) } else { transparent() })
                    .when(is_active, |d| d.bg(c(p.bg)))
                    .text_color(c(if is_active { p.text_strong } else { p.muted }))
                    .hover(move |s| if is_active { s } else { s.bg(c(p.bg_hover)).text_color(c(p.text)) })
                    .focus_visible(move |s| s.border_2().border_color(c(p.accent)))
                    .cursor_default()
                    .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                        if *h {
                            this.hover = Some(("tab", i));
                        } else if this.hover == Some(("tab", i)) {
                            this.hover = None;
                        }
                        cx.notify();
                    }))
                    .when(moving == Some(tab_id), |d| d.opacity(0.45))
                    .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        tab_move::press(tab_id, e.position, window, cx);
                    })
                    .on_mouse_up(MouseButton::Middle, cx.listener(move |this, _, _, cx| this.close_tab(tab_id, cx)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !tab_move::take_swallow_click(cx) {
                            this.activate_tab(tab_id, cx);
                        }
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        div()
                            .id(("tab-x", tab_id))
                            .role(Role::Button)
                            .aria_label(close_label)
                            .flex()
                            .flex_none()
                            .items_center()
                            .justify_center()
                            .w(px(m::TAB_X))
                            .h(px(m::TAB_X))
                            .cursor_pointer()
                            .opacity(if is_active { 0.7 } else { 0.0 })
                            .group_hover("tab", |s| s.opacity(0.7))
                            .hover(move |s| s.opacity(1.0).bg(c(p.bg_hover)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(tab_id, cx);
                            }))
                            .child(icon("tab-x", m::TAB_X_SVG, c(x_fg))),
                    ),
            );
        }
        // Unused strip space: the only part of the tab area that moves the window.
        let filler = div()
            .id("tab-filler")
            .flex_1()
            .min_w_0()
            .h_full()
            .window_control_area(WindowControlArea::Drag)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.press_header(e, window);
            }));
        strip = strip.child(filler);

        let find_open = self.find_open;
        let b_toc = self.icon_btn("btn-toc", "toc", "Contents (Ctrl+B)", icon_w, m::ICON_H, m::ICON_SVG, self.toc_open, cx, |t, _, _| t.toc_open = !t.toc_open)
            .aria_toggled(if self.toc_open { Toggled::True } else { Toggled::False });
        let b_open = self.icon_btn("btn-open", "open", "Open (Ctrl+O)", icon_w, m::ICON_H, m::ICON_SVG, false, cx, |t, _, cx| t.show_toast("Opening files is not part of this spike", false, cx));
        let b_reload = self.icon_btn("btn-reload", "reload", "Reload (F5)", icon_w, m::ICON_H, m::ICON_SVG, false, cx, |t, _, cx| t.show_toast("Reloading is not part of this spike", false, cx));
        let b_find = self.icon_btn("btn-find", "find", "Find (Ctrl+F)", icon_w, m::ICON_H, m::ICON_SVG, false, cx, move |t, w, cx| {
            if find_open {
                t.close_find(w, cx);
            } else {
                t.open_find(w, cx);
            }
        });
        let b_print = self.icon_btn("btn-print", "print", "Save as PDF / Print (Ctrl+P)", if narrow { 40.0 } else { icon_w }, m::ICON_H, m::ICON_SVG, false, cx, |t, _, cx| {
            t.tools_open = false;
            t.show_toast("Printing is not part of this spike", false, cx)
        });
        let settings_open = self.settings_open;
        let b_settings = self.icon_btn("btn-settings", "settings", "Settings", if narrow { 40.0 } else { icon_w }, m::ICON_H, m::ICON_SVG, settings_open, cx, move |t, _, _| {
            t.tools_open = false;
            t.settings_open = !settings_open;
        })
        .aria_expanded(settings_open);
        let tools_open = self.tools_open;
        let b_tools = self.icon_btn("btn-tools", "tools", "More actions", icon_w, m::ICON_H, m::ICON_SVG, false, cx, move |t, _, _| {
            t.settings_open = false;
            t.tools_open = !tools_open;
        })
        .aria_expanded(tools_open);

        let wc_w = if narrow { 30.0 } else { m::WINCTL_W };
        let b_min = self.icon_btn("win-minimise", "minimise", "Minimise", wc_w, m::ICON_H, m::WINCTL_SVG, false, cx, |_, w, _| w.minimize_window()).window_control_area(WindowControlArea::Min);
        let max_name = if maximised { "restore" } else { "maximise" };
        let b_max = self.icon_btn("win-maximise", max_name, if maximised { "Restore" } else { "Maximise" }, wc_w, m::ICON_H, m::WINCTL_SVG, false, cx, |_, w, _| w.zoom_window()).window_control_area(WindowControlArea::Max);
        let b_close = self
            .icon_btn("win-close", "close", "Close window", wc_w, m::ICON_H, m::WINCTL_SVG, false, cx, |_, w, _| w.remove_window())
            .window_control_area(WindowControlArea::Close);

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(if narrow { 0.0 } else { 2.0 }))
            .h(px(m::BAR_HEIGHT))
            .pl(px(if mac { m::MAC_BAR_INSET } else if narrow { 4.0 } else { m::BAR_PADDING_X }))
            .pr(px(if narrow { 4.0 } else { m::BAR_PADDING_X }))
            .bg(c(p.bg_bar))
            .border_b_1()
            .border_color(c(p.border))
            .child(title)
            .child(b_toc)
            .child(b_open)
            .child(b_reload)
            .child(strip)
            .child(b_find)
            .when(!narrow, |d| d.child(div().flex().flex_none().items_center().child(b_print).child(b_settings)))
            .when(narrow, |d| d.child(b_tools))
            .when(!mac, |d| {
                d.child(
                    div()
                        .id("window-controls")
                        .role(Role::Group)
                        .aria_label("Window controls")
                        .flex()
                        .flex_none()
                        .items_center()
                        .border_l_1()
                        .border_color(c(p.border))
                        .ml(px(if narrow { 4.0 } else { 8.0 }))
                        .pl(px(if narrow { 4.0 } else { 8.0 }))
                        .child(b_min)
                        .child(b_max)
                        .child(b_close),
                )
            })
    }

    fn app_icon(&self) -> std::sync::Arc<gpui::Image> {
        std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, assets::APP_ICON.to_vec()))
    }

    /// Press on empty header space: a double-click maximises; otherwise the window starts moving
    /// once the pointer moves with the button held (so a double-click is not swallowed by the
    /// window manager's move grab).
    fn press_header(&mut self, e: &MouseDownEvent, window: &mut Window) {
        if e.click_count == 2 {
            self.should_move = false;
            window.zoom_window();
        } else {
            self.should_move = true;
        }
    }

    /// Narrow-layout dropdown that holds print and settings (`#tools.open`).
    fn tools_popup(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let settings_open = self.settings_open;
        let b_print = self.icon_btn("tools-print", "print", "Save as PDF / Print (Ctrl+P)", 40.0, m::ICON_H, m::ICON_SVG, false, cx, |t, _, cx| {
            t.tools_open = false;
            t.show_toast("Printing is not part of this spike", false, cx)
        });
        let b_set = self.icon_btn("tools-settings", "settings", "Settings", 40.0, m::ICON_H, m::ICON_SVG, settings_open, cx, move |t, _, _| {
            t.tools_open = false;
            t.settings_open = !settings_open;
        });
        div()
            .absolute()
            .top(px(54.0))
            .right(px(8.0))
            .flex()
            .p(px(6.0))
            .border_1()
            .border_color(c(p.border))
            .bg(c(p.bg))
            .occlude()
            .child(b_print)
            .child(b_set)
    }

    fn find_bar(&mut self, vw: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let total = self.find_total();
        let count = if self.find_text.is_empty() {
            String::new()
        } else if total == 0 {
            "No results".to_string()
        } else {
            format!("{} of {}", self.find_pos + 1, total)
        };
        let prev = self.icon_btn("find-prev", "prev", "Previous (Shift+Enter)", m::ICON_SMALL, m::ICON_SMALL, m::ICON_SMALL_SVG, false, cx, |t, _, _| {
            let n = t.find_total();
            if n > 0 {
                t.find_pos = (t.find_pos + n - 1) % n;
            }
        });
        let next = self.icon_btn("find-next", "next", "Next (Enter)", m::ICON_SMALL, m::ICON_SMALL, m::ICON_SMALL_SVG, false, cx, |t, _, _| {
            let n = t.find_total();
            if n > 0 {
                t.find_pos = (t.find_pos + 1) % n;
            }
        });
        let close = self.icon_btn("find-close", "close", "Close (Esc)", m::ICON_SMALL, m::ICON_SMALL, m::ICON_SMALL_SVG, false, cx, |t, w, cx| t.close_find(w, cx));
        let empty = self.find_text.is_empty();
        let input_fh = self.fh("find-input", cx);
        let find_value = self.find_text.clone();
        let weak = cx.entity().downgrade();
        div()
            .absolute()
            .top(px(m::FIND_TOP))
            .right(px(m::FIND_RIGHT))
            .max_w(px(vw - 20.0))
            .flex()
            .items_center()
            .gap(px(2.0))
            .pl(px(10.0))
            .pr(px(6.0))
            .py(px(5.0))
            .bg(c(p.bg_bar))
            .border_1()
            .border_color(c(p.border))
            .occlude()
            .child(
                div()
                    .id("find-input")
                    .role(Role::SearchInput)
                    .aria_label("Find")
                    .aria_placeholder("Find")
                    .aria_value(find_value.clone())
                    .a11y_synthetic_children(a11y::input_text(find_value))
                    .on_a11y_action(AccessibleAction::SetValue, move |data, _, cx| {
                        if let Some(ActionData::Value(v)) = data {
                            let v = v.to_string();
                            weak.update(cx, |t, cx| {
                                t.find_text = v;
                                t.find_pos = 0;
                                cx.notify();
                            })
                            .ok();
                        }
                    })
                    .track_focus(&input_fh)
                    .flex()
                    .items_center()
                    .w(px(m::FIND_INPUT_W))
                    .min_w_0()
                    .overflow_hidden()
                    .child(if empty {
                        div().text_color(c(0x757575)).child("Find")
                    } else {
                        div().text_color(c(p.text)).child(self.find_text.clone())
                    })
                    // the caret: a 1px bar, as the focused <input> shows one
                    .child(div().w(px(1.0)).h(px(16.0)).bg(c(p.text)).when(!empty, |d| d.ml(px(1.0)))),
            )
            .child(
                div()
                    .id("find-count")
                    .role(Role::Status)
                    .aria_label(count.clone())
                    .a11y_synthetic_children(a11y::live_polite)
                    .min_w(px(70.0))
                    .pr(px(6.0))
                    .flex()
                    .justify_end()
                    .text_size(px(12.0))
                    .text_color(c(p.muted))
                    .font_features(tabular())
                    .child(count),
            )
            .child(prev)
            .child(next)
            .child(close)
    }

    fn web_bar(&mut self, vw: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let hidden = self.toast.is_some();
        let load = self.link("web-load", "Load images", cx, |t, _, _| t.web_bar = false);
        let dismiss = self.icon_btn("web-close", "dismiss", "Dismiss", m::ICON_SMALL, m::ICON_SMALL, m::ICON_SMALL_SVG, false, cx, |t, _, _| t.web_bar = false);
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(m::TOAST_BOTTOM))
            .flex()
            .justify_center()
            .when(hidden, |d| d.invisible())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .max_w(px(vw - 28.0))
                    .pl(px(14.0))
                    .pr(px(6.0))
                    .py(px(5.0))
                    .bg(c(p.bg_bar))
                    .border_1()
                    .border_color(c(p.border))
                    .font_family(MONO)
                    .text_size(px(12.0))
                    .text_color(c(p.muted))
                    .occlude()
                    .child(
                        div()
                            .id("web-message")
                            .role(Role::Label)
                            .aria_value("This file shows images from the web. Loading them lets those sites see that you opened it.")
                            .child("This file shows images from the web. Loading them lets those sites see that you opened it."),
                    )
                    .child(div().flex_none().child(load))
                    .child(dismiss),
            )
    }

    fn toast_view(&mut self, vw: f32, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let p = *self.mode.palette();
        let (msg, cancel) = self.toast.clone()?;
        let action = cancel.then(|| self.link("toast-cancel", "Cancel", cx, |t, _, _| t.toast = None));
        Some(
            div().absolute().left_0().right_0().bottom(px(m::TOAST_BOTTOM)).flex().justify_center().child(
                div()
                    .id("toast")
                    .role(Role::Status)
                    .aria_label(msg.clone())
                    .a11y_synthetic_children(a11y::live_polite)
                    .flex()
                    .gap(px(6.0))
                    .max_w(px((vw * 0.8).min(vw - 20.0)))
                    .px(px(16.0))
                    .py(px(9.0))
                    .bg(c(p.bg_bar))
                    .border_1()
                    .border_color(c(p.border))
                    .font_family(MONO)
                    .text_color(c(p.text))
                    .child(msg)
                    .children(action),
            ),
        )
    }

    fn empty_state(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let fh = self.fh("empty-open", cx);
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .text_color(c(p.text))
            .child(
                div()
                    .id("empty-title")
                    .role(Role::Heading)
                    .aria_level(1)
                    .aria_label("Open a markdown file, or drop one here.")
                    .my(px(20.0))
                    .text_size(px(20.0))
                    .line_height(relative(1.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Open a markdown file, or drop one here."),
            )
            .child(
                div()
                    .id("empty-open")
                    .role(Role::Button)
                    .aria_label("Open file")
                    .aria_keyshortcuts(if self.mac { "Cmd+O" } else { "Ctrl+O" })
                    .track_focus(&fh)
                    .mt(px(6.0))
                    .px(px(16.0))
                    .py(px(7.0))
                    .border_1()
                    .border_color(c(p.accent))
                    .bg(c(p.accent))
                    .text_color(c(p.bg))
                    .font_family(MONO)
                    .cursor_pointer()
                    .hover(move |s| s.bg(c(p.text)).border_color(c(p.text)))
                    .focus_visible(move |s| s.border_2().border_color(c(p.text)))
                    .on_click(cx.listener(|t, _, _, cx| t.show_toast("Opening files is not part of this spike", false, cx)))
                    .child(up("Open file…")),
            )
            .child(div().my(px(16.0)).font_family(MONO).text_size(px(16.0)).line_height(relative(1.4)).child(if self.mac { "⌘O" } else { "Ctrl+O" }))
    }

    // ---- TOC -------------------------------------------------------------------------------

    fn toc_panel(&mut self, win_h: f32, width: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let headings = self.headings();
        let n = headings.len();
        let base = headings.iter().map(|h| h.level).min().unwrap_or(1);
        let current = self.current;
        let perf = self.perf;
        let list_h = win_h - m::BAR_HEIGHT;
        let scrollable = n as f32 * m::TOC_ROW > list_h;
        let handle = self.toc_scroll.clone();
        let row_scroll = self.toc_scroll.clone();

        let list = if n == 0 {
            div()
                .id("toc-empty")
                .role(Role::Label)
                .aria_value("No headings")
                .px(px(16.0))
                .py(px(14.0))
                .text_size(px(13.0))
                .text_color(c(p.muted))
                .child("No headings")
                .into_any_element()
        } else {
            uniform_list(
                "toc-list",
                n,
                cx.processor(move |_this, range: std::ops::Range<usize>, _w, cx| {
                    let t0 = Instant::now();
                    let rows: Vec<_> = range
                        .clone()
                        .map(|i| {
                            let h = &headings[i];
                            let l1 = h.level == base;
                            let is_cur = i == current;
                            let scroll_row = row_scroll.clone();
                            div()
                                .id(("toc-item", i))
                                .role(Role::Link)
                                .aria_label(if h.text.is_empty() { SharedString::from("(untitled)") } else { h.text.clone() })
                                .aria_level(h.level as usize)
                                .aria_position_in_set(i + 1)
                                .aria_size_of_set(n)
                                .aria_selected(is_cur)
                                .focusable()
                                .tab_stop(true)
                                .focus_visible(move |s| s.border_2().border_color(c(p.accent)))
                                .on_a11y_action(AccessibleAction::ScrollIntoView, move |_, window, _| {
                                    // Only on-screen rows exist, so walking a long list relies on this.
                                    scroll_row.scroll_to_item_strict(i, ScrollStrategy::Center);
                                    window.refresh();
                                })
                                .h(px(m::TOC_ROW))
                                .w_full()
                                .line_height(px(m::TOC_ROW))
                                .pl(px(m::TOC_PAD_LEFT + (h.level - base) as f32 * m::TOC_INDENT - 2.0))
                                .pr(px(12.0))
                                .truncate()
                                .text_size(px(m::TOC_FONT))
                                .border_l_2()
                                .border_color(if is_cur { c(p.accent) } else { transparent() })
                                .text_color(c(if is_cur {
                                    p.text_strong
                                } else if l1 {
                                    p.text
                                } else {
                                    p.muted
                                }))
                                .when(l1, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .cursor_pointer()
                                .hover(move |s| s.bg(c(p.bg_hover)).text_color(c(p.text)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.current = i;
                                    cx.notify();
                                }))
                                .child(if h.text.is_empty() { SharedString::from("(untitled)") } else { h.text.clone() })
                        })
                        .collect();
                    if perf {
                        eprintln!("[perf] toc rows {}..{} built in {:?}", range.start, range.end, t0.elapsed());
                    }
                    rows
                }),
            )
            .track_scroll(&handle)
            .flex_1()
            .h_full()
            .into_any_element()
        };

        // The 12px scrollbar. Chromium's ::-webkit-scrollbar takes layout width, so the list
        // shrinks by 12px while it overflows.
        let bar = scrollable.then(|| self.scrollbar(&handle, list_h, cx));
        div()
            .id("toc")
            .role(Role::Navigation)
            .aria_label("Table of contents")
            .flex()
            .flex_none()
            .w(px(width))
            .h_full()
            .bg(c(p.bg_bar))
            .border_r_1()
            .border_color(c(p.border))
            .child(list)
            .children(bar)
    }

    fn scrollbar(&mut self, handle: &UniformListScrollHandle, view_h: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let state = handle.0.borrow();
        let off = -f32::from(state.base_handle.offset().y);
        let max = f32::from(state.base_handle.max_offset().y);
        drop(state);
        let content = max + view_h;
        let thumb_h = (view_h / content * view_h).clamp(24.0, view_h);
        let travel = (view_h - thumb_h).max(1.0);
        let top = if max > 0.0 { (off / max).clamp(0.0, 1.0) * travel } else { 0.0 };
        let dragging = self.drag_thumb.is_some();
        div()
            .relative()
            .flex_none()
            .w(px(m::SCROLLBAR))
            .h_full()
            .child(
                div()
                    .id("toc-thumb")
                    .absolute()
                    .top(px(top))
                    .left_0()
                    .w(px(m::SCROLLBAR))
                    .h(px(thumb_h))
                    .p(px(m::SCROLLBAR_THUMB_INSET))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.drag_thumb = Some((f32::from(e.position.y), off));
                        cx.notify();
                    }))
                    .child(div().size_full().bg(c(if dragging { p.muted } else { p.border })).hover(move |s| s.bg(c(p.muted)))),
            )
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.should_move {
            self.should_move = false;
            if e.pressed_button == Some(MouseButton::Left) {
                window.start_window_move();
            }
        }
        if let Some((y0, off0)) = self.drag_thumb {
            let state = self.toc_scroll.0.borrow();
            let max = f32::from(state.base_handle.max_offset().y);
            let view_h = f32::from(state.last_item_size.as_ref().map(|s| s.item.height).unwrap_or(px(600.0)));
            let content = max + view_h;
            let thumb_h = (view_h / content * view_h).clamp(24.0, view_h);
            let travel = (view_h - thumb_h).max(1.0);
            let dy = f32::from(e.position.y) - y0;
            let new_off = (off0 + dy / travel * max).clamp(0.0, max);
            let mut o = state.base_handle.offset();
            o.y = px(-new_off);
            state.base_handle.set_offset(o);
            drop(state);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, _e: &MouseUpEvent, _w: &mut Window, cx: &mut Context<Self>) {
        self.should_move = false;
        if self.drag_thumb.take().is_some() {
            cx.notify();
        }
    }

    // ---- settings popover ------------------------------------------------------------------

    fn settings_popover(&mut self, vw: f32, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let s = self.s;
        let mode = self.mode;

        let seg = |this: &mut Self, id: &'static str, a: &'static str, b: &'static str, a_on: bool, cx: &mut Context<Self>, set: fn(&mut Self, bool)| {
            let group = if id == "seg-theme" { "Theme" } else { "Font" };
            let mut row = div().id(id).role(Role::RadioGroup).aria_label(group).flex().border_1().border_color(c(p.border)).overflow_hidden();
            for (i, (label, on)) in [(a, a_on), (b, !a_on)].into_iter().enumerate() {
                let fh = this.fh(if i == 0 { Box::leak(format!("{id}-a").into_boxed_str()) } else { Box::leak(format!("{id}-b").into_boxed_str()) }, cx);
                row = row.child(
                    div()
                        .id((id, i))
                        .role(Role::RadioButton)
                        .aria_label(label)
                        .aria_toggled(if on { Toggled::True } else { Toggled::False })
                        .track_focus(&fh)
                        .px(px(10.0))
                        .py(px(4.0))
                        .when(i == 1, |d| d.border_l_1().border_color(c(p.border)))
                        .when(on, |d| d.bg(c(p.bg_hover)))
                        .text_color(c(if on { p.text_strong } else { p.muted }))
                        .cursor_pointer()
                        .hover(move |s| s.text_color(c(p.text_strong)))
                        .focus_visible(move |s| s.border_2().border_color(c(p.accent)))
                        .on_click(cx.listener(move |t, _, _, cx| {
                            set(t, i == 0);
                            cx.notify();
                        }))
                        .child(label),
                );
            }
            row
        };

        let stepper = |this: &mut Self, id: &'static str, key: StepKey, text: String, at_min: bool, at_max: bool, cx: &mut Context<Self>| {
            let mk = |this: &mut Self, sid: &'static str, label: &'static str, dir: f32, disabled: bool, cx: &mut Context<Self>| {
                let fh = this.fh(sid, cx);
                let name = key.name().to_lowercase();
                div()
                    .id(sid)
                    .role(Role::Button)
                    .aria_label(format!("{} {name}", if dir < 0.0 { "Decrease" } else { "Increase" }))
                    .when(disabled, |d| d.a11y_synthetic_children(a11y::disabled))
                    .track_focus(&fh)
                    .px(px(10.0))
                    .py(px(4.0))
                    .text_color(c(p.muted))
                    .when(disabled, |d| d.opacity(0.35).cursor_default())
                    .when(!disabled, |d| {
                        d.cursor_pointer()
                            .hover(move |s| s.text_color(c(p.text_strong)))
                            .focus_visible(move |s| s.border_2().border_color(c(p.accent)))
                            .on_click(cx.listener(move |t, _, _, cx| {
                                t.step(key, dir);
                                cx.notify();
                            }))
                    })
                    .child(label)
            };
            let minus = mk(this, Box::leak(format!("{id}-minus").into_boxed_str()), "−", -1.0, at_min, cx);
            let plus = mk(this, Box::leak(format!("{id}-plus").into_boxed_str()), "+", 1.0, at_max, cx);
            let (lo, hi, step) = key.range();
            let value = this.step_value(key);
            let (up_h, down_h, set_h) = (cx.entity().downgrade(), cx.entity().downgrade(), cx.entity().downgrade());
            div()
                .id(id)
                .role(Role::SpinButton)
                .aria_label(key.name())
                .aria_value(text.clone())
                .aria_numeric_value(value as f64)
                .aria_min_numeric_value(lo as f64)
                .aria_max_numeric_value(hi as f64)
                .aria_numeric_value_step(step as f64)
                .on_a11y_action(AccessibleAction::Increment, move |_, _, cx| {
                    up_h.update(cx, |t, cx| {
                        t.step(key, 1.0);
                        cx.notify();
                    })
                    .ok();
                })
                .on_a11y_action(AccessibleAction::Decrement, move |_, _, cx| {
                    down_h.update(cx, |t, cx| {
                        t.step(key, -1.0);
                        cx.notify();
                    })
                    .ok();
                })
                .on_a11y_action(AccessibleAction::SetValue, move |data, _, cx| {
                    if let Some(ActionData::NumericValue(v)) = data {
                        let v = *v as f32;
                        set_h.update(cx, |t, cx| {
                            t.set_step_value(key, v);
                            cx.notify();
                        })
                        .ok();
                    }
                })
                .flex()
                .border_1()
                .border_color(c(p.border))
                .overflow_hidden()
                .child(minus)
                .child(
                    div()
                        .flex()
                        .justify_center()
                        .min_w(px(46.0))
                        .py(px(4.0))
                        .border_l_1()
                        .border_r_1()
                        .border_color(c(p.border))
                        .font_features(tabular())
                        .child(text),
                )
                .child(plus)
        };

        let row = |label: &str, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.0))
                .min_h(px(m::ROW_MIN_H))
                .text_size(px(12.0))
                .child(div().text_color(c(p.text)).child(up(label)))
                .child(control)
        };

        let checkbox = |this: &mut Self, id: &'static str, on: bool, cx: &mut Context<Self>, set: fn(&mut Self)| {
            let fh = this.fh(id, cx);
            div()
                .id(id)
                .role(Role::CheckBox)
                .aria_label(if id == "cb-wrap" { "Wrap code and tables" } else { "Live reload" })
                .aria_toggled(if on { Toggled::True } else { Toggled::False })
                .track_focus(&fh)
                .flex()
                .items_center()
                .justify_center()
                .w(px(16.0))
                .h(px(16.0))
                .rounded(px(2.0))
                .border_1()
                .border_color(c(if on { p.accent } else { p.muted }))
                .when(on, |d| d.bg(c(p.accent)).text_color(c(p.bg)))
                .cursor_pointer()
                .focus_visible(move |s| s.border_2().border_color(c(p.accent)))
                .on_click(cx.listener(move |t, _, _, cx| {
                    set(t);
                    cx.notify();
                }))
                .when(on, |d| d.child(icon("check", 12.0, c(p.bg))))
        };

        let wrap_cb = checkbox(self, "cb-wrap", s.wrap, cx, |t| t.s.wrap = !t.s.wrap);
        let live_cb = checkbox(self, "cb-live", s.live_reload, cx, |t| t.s.live_reload = !t.s.live_reload);
        let theme_seg = seg(self, "seg-theme", "Dark", "Light", mode == Mode::Dark, cx, |t, a| t.mode = if a { Mode::Dark } else { Mode::Light });
        let font_seg = seg(self, "seg-font", "Sans", "Serif", !s.serif, cx, |t, a| t.s.serif = !a);
        let pad = stepper(self, "step-pad", StepKey::Padding, format!("{} px", s.padding).to_uppercase(), s.padding <= 20.0, s.padding >= 40.0, cx);
        let fsz = stepper(self, "step-fsz", StepKey::FontSize, format!("{}", s.font_size), s.font_size <= 12.0, s.font_size >= 28.0, cx);
        let lh = stepper(self, "step-lh", StepKey::LineHeight, format!("{:.2}", s.line_height), s.line_height <= 1.2 + 1e-6, s.line_height >= 2.2 - 1e-6, cx);
        let reset = self.link("reset", "Reset", cx, |t, _, _| {
            t.s = Settings::DEFAULTS;
            t.mode = Mode::Dark;
        });
        let lic_a = self.licence_link("lic-chakra", "Chakra Petch licence", assets::OFL_CHAKRA, cx);
        let lic_b = self.licence_link("lic-share", "Share Tech Mono licence", assets::OFL_SHARE_TECH, cx);

        div()
            .id("settings")
            .role(Role::Group)
            .aria_label("Settings")
            .absolute()
            .top(px(m::POPOVER_TOP))
            .right(px(m::POPOVER_RIGHT))
            .w(px(m::POPOVER_W))
            .max_w(px(vw - 20.0))
            .pt(px(8.0))
            .pb(px(10.0))
            .px(px(14.0))
            .bg(c(p.bg_bar))
            .border_1()
            .border_color(c(p.border))
            .font_family(MONO)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(row("Theme", theme_seg.into_any_element()))
            .child(row("Font", font_seg.into_any_element()))
            .child(row("Padding", pad.into_any_element()))
            .child(row("Font size", fsz.into_any_element()))
            .child(row("Line spacing", lh.into_any_element()))
            .child(row("Wrap code and tables", wrap_cb.into_any_element()))
            .child(row("Live reload", live_cb.into_any_element()))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .mt(px(6.0))
                    .pt(px(8.0))
                    .border_t_1()
                    .border_color(c(p.border))
                    .text_color(c(p.muted))
                    .text_size(px(12.0))
                    .child(up(&format!("MD Viewer {VERSION}")))
                    .child(reset),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .mt(px(16.0))
                    .text_color(c(p.muted))
                    .text_size(px(10.0))
                    .line_height(relative(1.4))
                    .child(icon("nz", 22.0, c(p.accent)).w(px(17.6)).h(px(22.0)).text_color(c(p.accent)))
                    .child(spaced(&up("Aotearoa New Zealand"), 10.0, 0.08)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .mt(px(12.0))
                    .text_size(px(10.0))
                    .text_color(c(p.muted))
                    .child(lic_a)
                    .child(div().px(px(4.0)).child("·"))
                    .child(lic_b),
            )
    }

    fn licence_link(&mut self, id: &'static str, label: &'static str, text: &'static str, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = *self.mode.palette();
        let fh = self.fh(id, cx);
        div()
            .id(id)
            .role(Role::Link)
            .aria_label(label)
            .track_focus(&fh)
            .underline()
            .cursor_pointer()
            .focus_visible(move |s| s.border_2().border_color(c(p.accent)))
            .on_click(cx.listener(move |t, _, _, cx| {
                t.licence = Some(text);
                cx.notify();
            }))
            .child(label)
    }

    fn licence_dialog(&mut self, vw: f32, vh: f32, text: &'static str, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        let close = self.link("licence-close", "Close", cx, |t, _, _| t.licence = None);
        div().id("licence").role(Role::Dialog).aria_label("Licence").absolute().size_full().bg(c(p.bg)).flex().items_center().justify_center().occlude().child(
            div()
                .flex()
                .flex_col()
                .w(px(700.0_f32.min(vw - 32.0)))
                .max_h(px(vh - 32.0))
                .p(px(20.0))
                .bg(c(p.bg))
                .text_color(c(p.text))
                .border_1()
                .border_color(c(p.border))
                .child(div().flex().flex_none().child(close))
                .child(
                    div()
                        .id("licence-text")
                        .role(Role::Label)
                        .aria_value(text)
                        .overflow_y_scroll()
                        .font_family(MONO)
                        .text_size(px(13.0))
                        .line_height(relative(1.6))
                        .child(text),
                ),
        )
    }
}

fn resize_edge(pos: gpui::Point<gpui::Pixels>, edge: f32, size: gpui::Size<gpui::Pixels>) -> Option<ResizeEdge> {
    let e = px(edge);
    let (l, t) = (pos.x < e, pos.y < e);
    let (r, b) = (pos.x > size.width - e, pos.y > size.height - e);
    Some(match (l, r, t, b) {
        (true, _, true, _) => ResizeEdge::TopLeft,
        (_, true, true, _) => ResizeEdge::TopRight,
        (true, _, _, true) => ResizeEdge::BottomLeft,
        (_, true, _, true) => ResizeEdge::BottomRight,
        (_, _, true, _) => ResizeEdge::Top,
        (_, _, _, true) => ResizeEdge::Bottom,
        (true, ..) => ResizeEdge::Left,
        (_, true, ..) => ResizeEdge::Right,
        _ => return None,
    })
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_render_logged {
            self.first_render_logged = true;
            if self.perf {
                eprintln!("[perf] first render after {:?}", self.started.elapsed());
            }
        }
        let p = *self.mode.palette();
        let vs = window.viewport_size();
        let (vw, vh) = (f32::from(vs.width), f32::from(vs.height));
        let narrow = vw <= m::NARROW_MAX_WIDTH;
        let has_doc = !self.tabs.is_empty();
        let client_decorations = matches!(window.window_decorations(), Decorations::Client { .. });
        let root_focus = self.root_focus.clone();

        // The window title is how assistive technology tells windows apart.
        let title = self.tabs.get(self.active).map_or("MD Viewer".to_string(), |t| format!("{} - MD Viewer", t.name));
        if self.title != title {
            window.set_window_title(&title);
            self.title = title;
        }

        let header = self.header(narrow, cx);
        let toc = (has_doc && self.toc_open).then(|| self.toc_panel(vh, if narrow { (vw * 0.36).min(180.0) } else { m::TOC_WIDTH }, cx));
        let sample = self.doc_sample.take();
        let sample_body = sample.as_ref().map(|d| a11y::doc_sample(d, c(p.text), cx).into_any_element());
        self.doc_sample = sample;
        let body = if let Some(sample_body) = sample_body {
            sample_body
        } else if has_doc {
            let tab_name = self.tabs[self.active].name.clone();
            let s = self.s;
            div()
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .p(px(s.padding))
                .font_family(if s.serif { SERIF } else { SANS })
                .text_size(px(s.font_size))
                .line_height(relative(s.line_height))
                .text_color(c(p.muted))
                .child(format!("Reader pane for {tab_name}: owned by the reader crate (stub)"))
                .into_any_element()
        } else {
            self.empty_state(cx).into_any_element()
        };

        let find = self.find_open.then(|| self.find_bar(vw, cx));
        let web = self.web_bar.then(|| self.web_bar(vw, cx));
        let toast = self.toast_view(vw, cx);
        let settings = self.settings_open.then(|| self.settings_popover(vw, cx));
        let tools = (narrow && self.tools_open).then(|| self.tools_popup(cx));
        let licence = self.licence.map(|t| self.licence_dialog(vw, vh, t, cx));

        div()
            .id("root")
            .role(Role::Group)
            .aria_label("MD Viewer")
            .track_focus(&root_focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(c(p.bg))
            .text_color(c(p.text))
            .font_family(SANS)
            .text_size(px(14.0))
            .line_height(relative(1.4))
            .capture_key_down(|e: &KeyDownEvent, _, cx| tab_move::capture_key_down(e, cx))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, w, cx| this.key_down(e, w, cx)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, w, cx| this.mouse_move(e, w, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, e: &MouseUpEvent, w, cx| this.mouse_up(e, w, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                // Clicks on empty header space move the window; any other stray click closes popups.
                let y = f32::from(e.position.y);
                if this.perf {
                    eprintln!("[perf] root mouse down at {:?} client={} bounds={:?}", e.position, client_decorations, window.window_bounds().get_bounds().size);
                }
                if client_decorations && !cfg!(target_os = "windows") {
                    let size = window.window_bounds().get_bounds().size;
                    if let Some(edge) = resize_edge(e.position, 6.0, size) {
                        if this.perf {
                            eprintln!("[perf] start resize {edge:?}");
                        }
                        window.start_window_resize(edge);
                        return;
                    }
                    if y < m::BAR_HEIGHT {
                        this.press_header(e, window);
                    }
                }
                if this.settings_open || this.tools_open {
                    this.close_popups();
                    cx.notify();
                }
            }))
            .when(client_decorations, |d| {
                d.child(
                    canvas(
                        |_b, window, _cx| {
                            window.insert_hitbox(Bounds::new(point(px(0.0), px(0.0)), window.window_bounds().get_bounds().size), HitboxBehavior::Normal)
                        },
                        |_b, hitbox, window, _cx| {
                            let size = window.window_bounds().get_bounds().size;
                            if let Some(edge) = resize_edge(window.mouse_position(), 6.0, size) {
                                window.set_cursor_style(
                                    match edge {
                                        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                                        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                                        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
                                        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
                                    },
                                    &hitbox,
                                );
                            }
                        },
                    )
                    .size_full()
                    .absolute(),
                )
            })
            .child(header)
            .child(div().flex().flex_1().min_h_0().w_full().children(toc).child(body))
            .children(find)
            .children(web)
            .children(toast)
            .children(settings)
            .children(tools)
            .children(licence)
            // tab movement: raw event listeners, and the insertion marker
            .child(tab_move::event_sink())
            .children(self.drop_marker.map(|m| {
                div().absolute().left(px(m.x)).top(px(m.top)).w(px(2.0)).h(px(m.height)).bg(c(p.accent))
            }))
            // body::after: the 1px perimeter outline, above everything and transparent to the pointer
            .child(div().absolute().size_full().border_1().border_color(c(p.border)))
    }
}
