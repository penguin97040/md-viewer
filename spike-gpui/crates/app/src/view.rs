//! The window chrome: header, tabs, TOC, find bar, web-images bar, toast, empty state and the
//! settings popover. Specification: frontend/index.html, style.css, windowbar.js, toc.js, tabs.js.

use std::collections::HashMap;
use std::time::Instant;

use gpui::{
    canvas, div, point, prelude::*, px, relative, uniform_list, Bounds, Context, CursorStyle,
    Decorations, Div, FocusHandle, FontWeight, HitboxBehavior, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ResizeEdge,
    SharedString, Stateful, UniformListScrollHandle, Window, WindowControlArea,
};
use mdv_theme::{metrics as m, Mode};

// reader: the real reader pane, its find field and file opening.
use mdv_reader::ui::{field, pane::{self, ReaderEvent, ReaderView}};
use std::path::PathBuf;
use crate::assets;
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
    pub name: SharedString,
    pub path: SharedString,
    pub headings: std::rc::Rc<Vec<Heading>>,
    /// reader: `None` only for the chrome's stub demo tabs.
    pub reader: Option<gpui::Entity<ReaderView>>,
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

pub struct AppView {
    pub mode: Mode,
    pub mac: bool,
    pub tabs: Vec<TabData>,
    pub active: usize,
    pub toc_open: bool,
    pub find_open: bool,
    pub find_field: field::Field, // reader
    pub find_focus: bool,         // reader
    find_blink: u64,              // reader
    find_started: Instant,        // reader
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
            find_field: field::Field::default(),
            find_focus: false,
            find_blink: 0,
            find_started: Instant::now(),
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
        }
    }

    fn fh(&mut self, id: &'static str, cx: &mut Context<Self>) -> FocusHandle {
        self.focus.entry(id).or_insert_with(|| cx.focus_handle().tab_stop(true)).clone()
    }

    fn headings(&self) -> std::rc::Rc<Vec<Heading>> {
        self.tabs.get(self.active).map(|t| t.headings.clone()).unwrap_or_default()
    }

    // ---- reader: begin --------------------------------------------------------------------

    fn reader(&self) -> Option<gpui::Entity<ReaderView>> {
        self.tabs.get(self.active).and_then(|t| t.reader.clone())
    }

    fn reader_settings(&self) -> pane::Settings {
        pane::Settings { padding: self.s.padding, font_size: self.s.font_size, line_height: self.s.line_height, serif: self.s.serif, wrap: self.s.wrap, mode: self.mode }
    }

    /// Opens a file in a tab, or switches to its existing tab. One tab per file.
    pub fn open_path(&mut self, path: PathBuf, fragment: Option<String>, cx: &mut Context<Self>) {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let shown: SharedString = path.to_string_lossy().to_string().into();
        if let Some(i) = self.tabs.iter().position(|t| t.path == shown) {
            self.active = i;
            self.current = 0;
            if let (Some(f), Some(r)) = (fragment, self.tabs[i].reader.clone()) {
                r.update(cx, |r, cx| r.jump_to_fragment(&f, cx));
            }
            cx.notify();
            return;
        }
        let name: SharedString = path.file_name().map_or("Untitled".to_string(), |n| n.to_string_lossy().to_string()).into();
        let settings = self.reader_settings();
        let reader = cx.new(|cx| ReaderView::new(settings, cx));
        cx.subscribe(&reader, |this, _, e: &ReaderEvent, cx| match e {
            ReaderEvent::OpenLocal { path, fragment } => this.open_path(path.clone(), fragment.clone(), cx),
            ReaderEvent::Toast(m) => this.show_toast(m, false, cx),
        })
        .detach();
        let first = self.tabs.iter().all(|t| t.reader.is_none());
        self.tabs.push(TabData { name, path: shown.clone(), headings: Default::default(), reader: Some(reader.clone()) });
        self.active = self.tabs.len() - 1;
        self.current = 0;
        if first {
            mdv_reader::ui::script::start(reader.clone(), cx);
        }
        let app = cx.entity().downgrade();
        reader.update(cx, |r, cx| {
            r.open(path, cx, move |r, result, cx| {
                let outline: Result<Vec<Heading>, String> = result.map(|()| {
                    r.doc.as_ref().map_or(Vec::new(), |d| d.headings.iter().map(|h| Heading { level: h.level, text: h.text.clone().into() }).collect())
                });
                if let (Ok(_), Some(f)) = (&outline, &fragment) {
                    r.jump_to_fragment(f, cx);
                }
                let _ = app.update(cx, |this, cx| {
                    match outline {
                        Ok(h) => {
                            if let Some(t) = this.tabs.iter_mut().find(|t| t.path == shown) {
                                t.headings = std::rc::Rc::new(h);
                            }
                        }
                        Err(e) => {
                            this.tabs.retain(|t| t.path != shown);
                            this.active = this.active.min(this.tabs.len().saturating_sub(1));
                            this.show_toast(&e, false, cx);
                        }
                    }
                    cx.notify();
                });
            });
        });
        cx.notify();
    }

    /// The native open dialog.
    fn prompt_open(&mut self, cx: &mut Context<Self>) {
        let answer = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = answer.await {
                let _ = this.update(cx, |this, cx| {
                    for p in paths {
                        this.open_path(p, None, cx);
                    }
                });
            }
        })
        .detach();
    }

    fn find_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _ = window;
        self.find_open = true;
        self.find_focus = true;
        self.find_started = Instant::now();
        self.close_popups();
        if let Some(r) = self.reader() {
            // A short one-line selection seeds the query, then it is all selected for replacing.
            let seed = r.read(cx).selected_text().map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && !t.contains('\n') && t.len() < 200);
            if let Some(t) = seed {
                self.find_field.set(&t);
            } else {
                self.find_field.select_all();
            }
            let q = self.find_field.text.clone();
            r.update(cx, |r, cx| {
                if mdv_reader::fold::fold(&q) != r.find.q {
                    r.find_query(&q, false, cx);
                }
            });
        }
        self.start_blink(cx);
    }

    /// The caret needs repainting at its blink rate while the find field has focus.
    fn start_blink(&mut self, cx: &mut Context<Self>) {
        self.find_blink += 1;
        let gen = self.find_blink;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(530)).await;
            let go = this.update(cx, |this, cx| {
                let on = this.find_blink == gen && this.find_open && this.find_focus;
                if on {
                    cx.notify();
                }
                on
            });
            if !matches!(go, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    fn find_close(&mut self, cx: &mut Context<Self>) {
        self.find_open = false;
        self.find_focus = false;
        if let Some(r) = self.reader() {
            r.update(cx, |r, cx| r.find_close(cx));
        }
    }

    fn find_step(&mut self, dir: i64, cx: &mut Context<Self>) {
        if let Some(r) = self.reader() {
            r.update(cx, |r, cx| r.find_step(dir, cx));
        }
    }

    fn find_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let k = e.keystroke.key.as_str();
        let md = &e.keystroke.modifiers;
        if k == "enter" {
            let q = self.find_field.text.clone();
            let shift = md.shift;
            if let Some(r) = self.reader() {
                r.update(cx, |r, cx| {
                    if mdv_reader::fold::fold(&q) != r.find.q {
                        r.find_query(&q, false, cx);
                    } else {
                        r.find_step(if shift { -1 } else { 1 }, cx);
                    }
                });
            }
            return true;
        }
        let edit = field::apply_key(&mut self.find_field, e, self.mac, cx);
        self.find_started = Instant::now();
        if edit.changed {
            let q = self.find_field.text.clone();
            if let Some(r) = self.reader() {
                r.update(cx, |r, cx| r.find_query(&q, true, cx));
            }
        }
        edit.handled
    }

    // ---- reader: end ----------------------------------------------------------------------

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
        let (lo, hi, step) = match key {
            StepKey::Padding => (20.0, 40.0, 4.0),
            StepKey::FontSize => (12.0, 28.0, 1.0),
            StepKey::LineHeight => (1.2, 2.2, 0.05),
        };
        let slot = match key {
            StepKey::Padding => &mut self.s.padding,
            StepKey::FontSize => &mut self.s.font_size,
            StepKey::LineHeight => &mut self.s.line_height,
        };
        let v = ((*slot + dir * step) * 100.0).round() / 100.0;
        *slot = v.clamp(lo, hi);
    }

    fn key_down(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = e.keystroke.key.as_str();
        let md = &e.keystroke.modifiers;
        let modkey = if self.mac { md.platform } else { md.control };
        if modkey {
            match k {
                "f" => self.find_open(window, cx),
                "b" => self.toc_open = !self.toc_open,
                "o" => self.prompt_open(cx),
                "p" => self.show_toast("Printing is not part of this spike", false, cx),
                // reader: anything else with the shortcut key goes to the find field or the page.
                _ => {
                    if self.find_open && self.find_focus {
                        if !self.find_key(e, cx) {
                            return;
                        }
                    } else {
                        let mac = self.mac;
                        let used = self.reader().is_some_and(|r| r.update(cx, |r, cx| r.key_down(e, mac, cx)));
                        if !used {
                            return;
                        }
                    }
                }
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
                    self.find_close(cx);
                }
            }
            "f5" => self.show_toast("Reloading is not part of this spike", false, cx),
            _ if self.find_open && self.find_focus => {
                if !self.find_key(e, cx) {
                    return;
                }
            }
            // reader: keys for the document.
            _ => {
                let mac = self.mac;
                let used = self.reader().is_some_and(|r| r.update(cx, |r, cx| r.key_down(e, mac, cx)));
                if !used {
                    return;
                }
            }
        }
        cx.notify();
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
        div()
            .id(id)
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
            .occlude()
            .hover(move |s| s.bg(c(p.bg_hover)).text_color(c(if on { p.accent } else { p.text })))
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
        let tabs: Vec<(SharedString, SharedString)> = self.tabs.iter().map(|t| (t.name.clone(), t.path.clone())).collect();
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
            .flex()
            .flex_1()
            .min_w_0()
            .items_stretch()
            .self_stretch()
            .overflow_hidden()
            .pt(px(6.0))
            .pb(px(4.0))
            .px(px(6.0));
        for (i, (name, path)) in tabs.iter().enumerate() {
            let is_active = i == active;
            let tab_hover = self.hover == Some(("tab", i));
            let x_fg = if is_active { p.text_strong } else if tab_hover { p.text } else { p.muted };
            let name = name.clone();
            let _ = path;
            let fh = self.fh(Box::leak(format!("tab{i}").into_boxed_str()), cx);
            strip = strip.child(
                div()
                    .id(("tab", i))
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
                    .occlude()
                    .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                        if *h {
                            this.hover = Some(("tab", i));
                        } else if this.hover == Some(("tab", i)) {
                            this.hover = None;
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.active = i;
                        this.current = 0;
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w_0().truncate().child(name))
                    .child(
                        div()
                            .id(("tab-x", i))
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
                                if i < this.tabs.len() {
                                    this.tabs.remove(i);
                                    if this.active >= this.tabs.len() {
                                        this.active = this.tabs.len().saturating_sub(1);
                                    }
                                    this.current = 0;
                                }
                                cx.notify();
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
            // Windows moves the window natively (HTCAPTION), but only if no handler stops the
            // press: handle_nc_mouse_down_msg returns early when propagation is stopped, and a
            // double-click must reach DefWindowProc to maximise. So no handler there.
            .when(!cfg!(target_os = "windows"), |d| {
                d.on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.press_header(e, window);
                }))
            });
        strip = strip.child(filler);

        let find_open = self.find_open;
        let b_toc = self.icon_btn("btn-toc", "toc", "Contents (Ctrl+B)", icon_w, m::ICON_H, m::ICON_SVG, self.toc_open, cx, |t, _, _| t.toc_open = !t.toc_open);
        let b_open = self.icon_btn("btn-open", "open", "Open (Ctrl+O)", icon_w, m::ICON_H, m::ICON_SVG, false, cx, |t, _, cx| t.prompt_open(cx));
        let b_reload = self.icon_btn("btn-reload", "reload", "Reload (F5)", icon_w, m::ICON_H, m::ICON_SVG, false, cx, |t, _, cx| t.show_toast("Reloading is not part of this spike", false, cx));
        let b_find = self.icon_btn("btn-find", "find", "Find (Ctrl+F)", icon_w, m::ICON_H, m::ICON_SVG, false, cx, move |t, w, cx| {
            if find_open {
                t.find_close(cx);
            } else {
                t.find_open(w, cx);
            }
            t.close_popups();
        });
        let b_print = self.icon_btn("btn-print", "print", "Save as PDF / Print (Ctrl+P)", if narrow { 40.0 } else { icon_w }, m::ICON_H, m::ICON_SVG, false, cx, |t, _, cx| {
            t.tools_open = false;
            t.show_toast("Printing is not part of this spike", false, cx)
        });
        let settings_open = self.settings_open;
        let b_settings = self.icon_btn("btn-settings", "settings", "Settings", if narrow { 40.0 } else { icon_w }, m::ICON_H, m::ICON_SVG, settings_open, cx, move |t, _, _| {
            t.tools_open = false;
            t.settings_open = !settings_open;
        });
        let tools_open = self.tools_open;
        let b_tools = self.icon_btn("btn-tools", "tools", "More actions", icon_w, m::ICON_H, m::ICON_SVG, false, cx, move |t, _, _| {
            t.settings_open = false;
            t.tools_open = !tools_open;
        });

        let wc_w = if narrow { 30.0 } else { m::WINCTL_W };
        let b_min = self.icon_btn("win-minimise", "minimise", "Minimise", wc_w, m::ICON_H, m::WINCTL_SVG, false, cx, |_, w, _| w.minimize_window()).window_control_area(WindowControlArea::Min);
        let max_name = if maximised { "restore" } else { "maximise" };
        let b_max = self.icon_btn("win-maximise", max_name, if maximised { "Restore" } else { "Maximise" }, wc_w, m::ICON_H, m::WINCTL_SVG, false, cx, |_, w, _| w.zoom_window()).window_control_area(WindowControlArea::Max);
        let b_close = self
            .icon_btn("win-close", "close", "Close window", wc_w, m::ICON_H, m::WINCTL_SVG, false, cx, |_, w, _| w.remove_window())
            .window_control_area(WindowControlArea::Close)
            .hover(move |s| s.bg(c(p.accent)).text_color(c(p.bg)));

        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(if narrow { 0.0 } else { 2.0 }))
            .h(px(m::BAR_HEIGHT))
            // Windows: the whole bar (gaps, padding, title, strip space) is a caption area.
            // Buttons and tabs `.occlude()` so the bar's hitbox is excluded under them and the
            // first-match hit test in gpui-pre sees them instead.
            .window_control_area(WindowControlArea::Drag)
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
        let count = self.reader().map_or(String::new(), |r| r.read(cx).find_count_text());
        let prev = self.icon_btn("find-prev", "prev", "Previous (Shift+Enter)", m::ICON_SMALL, m::ICON_SMALL, m::ICON_SMALL_SVG, false, cx, |t, _, cx| t.find_step(-1, cx));
        let next = self.icon_btn("find-next", "next", "Next (Enter)", m::ICON_SMALL, m::ICON_SMALL, m::ICON_SMALL_SVG, false, cx, |t, _, cx| t.find_step(1, cx));
        let close = self.icon_btn("find-close", "close", "Close (Esc)", m::ICON_SMALL, m::ICON_SMALL, m::ICON_SMALL_SVG, false, cx, |t, _, cx| t.find_close(cx));
        let look = field::FieldLook {
            font: gpui::font(SANS),
            size: 14.0,
            text: c(p.text),
            placeholder: c(0x757575),
            selection: c(p.accent),
            selected_text: c(p.bg),
            caret: c(p.text),
            started: self.find_started,
            focused: self.find_focus,
        };
        let font_for_hit = look.font.clone();
        let shown = self.find_field.clone();
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
                    .flex()
                    .items_center()
                    .w(px(m::FIND_INPUT_W))
                    .min_w_0()
                    .overflow_hidden()
                    .id("find-input")
                    .h(px(20.0))
                    .cursor(CursorStyle::IBeam)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.find_focus = true;
                        this.find_started = Instant::now();
                        this.start_blink(cx);
                        let look = field::FieldLook { font: font_for_hit.clone(), size: 14.0, text: c(0), placeholder: c(0), selection: c(0), selected_text: c(0), caret: c(0), started: Instant::now(), focused: true };
                        // The field's left edge: right-aligned bar, 10 px padding, known widths.
                        let vw = f32::from(window.viewport_size().width);
                        let bar_w = 10.0 + m::FIND_INPUT_W + 2.0 + 70.0 + 2.0 + m::ICON_SMALL * 3.0 + 2.0 * 2.0 + 6.0 + 2.0;
                        let left = vw - m::FIND_RIGHT - bar_w + 1.0 + 10.0;
                        let at = field::offset_at(&this.find_field.text, f32::from(e.position.x) - left, 0.0, &look, window);
                        if e.click_count >= 3 {
                            this.find_field.select_all();
                        } else if e.click_count == 2 {
                            this.find_field.select_word(at);
                        } else {
                            this.find_field.caret = at;
                            if !e.modifiers.shift {
                                this.find_field.anchor = at;
                            }
                        }
                        cx.notify();
                    }))
                    .child(field::field_canvas(&shown, look, m::FIND_INPUT_W)),
            )
            .child(
                div()
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
                    .child("This file shows images from the web. Loading them lets those sites see that you opened it.")
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
                    .my(px(20.0))
                    .text_size(px(20.0))
                    .line_height(relative(1.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Open a markdown file, or drop one here."),
            )
            .child(
                div()
                    .id("empty-open")
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
                    .on_click(cx.listener(|t, _, _, cx| t.prompt_open(cx)))
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

        let list = if n == 0 {
            div().px(px(16.0)).py(px(14.0)).text_size(px(13.0)).text_color(c(p.muted)).child("No headings").into_any_element()
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
                            div()
                                .id(("toc-item", i))
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
                                    // reader: the outline jumps the document.
                                    if let Some(r) = this.reader() {
                                        r.update(cx, |r, cx| r.jump_to_heading(i, cx));
                                    }
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
            let mut row = div().flex().border_1().border_color(c(p.border)).overflow_hidden();
            for (i, (label, on)) in [(a, a_on), (b, !a_on)].into_iter().enumerate() {
                let fh = this.fh(if i == 0 { Box::leak(format!("{id}-a").into_boxed_str()) } else { Box::leak(format!("{id}-b").into_boxed_str()) }, cx);
                row = row.child(
                    div()
                        .id((id, i))
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
                div()
                    .id(sid)
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
            div()
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
        div().absolute().size_full().bg(c(p.bg)).flex().items_center().justify_center().occlude().child(
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
            if std::env::var_os("MDV_TIMING").is_some() {
                crate::timing::log("first_render_started");
                window.on_next_frame(|_, _| crate::timing::log("first_frame_painted_approx"));
                window.refresh();
            }
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

        let header = self.header(narrow, cx);
        // reader: settings drive the active reader live, and its position drives the outline.
        if let Some(r) = self.reader() {
            let settings = self.reader_settings();
            let cur = r.update(cx, |r, _| {
                r.settings = settings;
                r.current_heading()
            });
            if let Some(cur) = cur {
                if cur != self.current {
                    self.current = cur;
                    self.toc_scroll.scroll_to_item(cur, gpui::ScrollStrategy::Nearest);
                }
            }
        }
        let toc = (has_doc && self.toc_open).then(|| self.toc_panel(vh, if narrow { (vw * 0.36).min(180.0) } else { m::TOC_WIDTH }, cx));
        let body = if let Some(r) = self.reader() {
            r.into_any_element()
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
                this.find_focus = false; // reader: a click outside the find field leaves it
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
            // body::after: the 1px perimeter outline, above everything and transparent to the pointer
            .child(div().absolute().size_full().border_1().border_color(c(p.border)))
    }
}
