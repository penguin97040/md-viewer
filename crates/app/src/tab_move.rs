//! Tab movement across windows. All windows share this process, so a move is a plain transfer of a
//! `TabData` between two views. There is no coordinator, no transaction token and no RPC.
//!
//! GPUI's own drag (`on_drag`, `on_drop`) cannot carry this: while the button is down the OS sends
//! input only to the source window, so another window's drop handlers never run, and a release
//! outside every window is simply the end of the drag. So the source window listens to its raw mouse
//! events, which still arrive with out-of-bounds coordinates, and this module works out what they hit
//! with `mdv_platform::native`.
//!
//! A move only takes the tab from its source after the destination is known to exist, and nothing
//! between the take and the insert can fail, so a failed move leaves the tab where it was.

use std::any::Any;
use std::fs::File;
use std::io::Write as _;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui::{
    canvas, point, prelude::*, px, size, AnyWindowHandle, App, Bounds, DispatchPhase, Entity, Global,
    CursorStyle, KeyDownEvent, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, TitlebarOptions, WeakEntity,
    Window, WindowBackgroundAppearance, WindowBounds, WindowDecorations, WindowId, WindowKind, WindowOptions,
};
use mdv_platform::drag::{CancelReason, Machine, Step};
use mdv_platform::native::{self, Backend, Hit, NativeWindow};
use mdv_platform::order::insertion_index;
use mdv_platform::placement::place_detached;
use mdv_platform::single_instance::{self, Owner, Request};
use mdv_platform::{DesktopPoint, DesktopRect};

use crate::view::{AppView, TabData};

pub type TabId = u64;

static NEXT_TAB: AtomicU64 = AtomicU64::new(1);

pub fn next_tab_id() -> TabId {
    NEXT_TAB.fetch_add(1, Ordering::Relaxed)
}

/// Everything that must follow a tab between windows. `TabData` moves by value, so anything added
/// to it follows automatically; `opaque` stands in for the reader's own per-tab state and is what
/// the tests use to prove that the same allocation arrives.
pub struct TabState {
    pub current_heading: usize,
    pub toc_offset: Pixels,
    pub opaque: Rc<dyn Any>,
}

impl Default for TabState {
    fn default() -> Self {
        Self { current_heading: 0, toc_offset: px(0.0), opaque: Rc::new(next_tab_id()) }
    }
}

/// Where a release at the current point would send the tab.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    /// Insert into this window's strip at this index. The source window's own strip means reorder.
    Strip { window: usize, index: usize },
    /// Inside a viewer, off the strip: a drop here cancels.
    InViewer,
    /// Outside every viewer: a drop here detaches.
    Outside(DesktopPoint),
    /// The platform cannot say where the pointer is: a drop here cancels.
    Unknown,
}

struct WindowEntry {
    view: WeakEntity<AppView>,
    handle: AnyWindowHandle,
    id: WindowId,
    native: NativeWindow,
    scale: f32,
}

/// The small floating label that follows the pointer during a drag. It is not in `Registry::windows`,
/// so nothing that counts viewers (routing, one tab per file, last window closed) sees it. There is
/// one per process, hidden between drags: a new window costs far more than a frame on Windows.
struct GhostWindow {
    handle: AnyWindowHandle,
    native: NativeWindow,
    view: Entity<Ghost>,
    /// The density the window was made at.
    scale: f32,
    /// Logical size, scaled to physical on each placement.
    size: (f32, f32),
    /// Where the window should be, physical pixels.
    rect: DesktopRect,
    shown: bool,
}

/// What the label shows, in the app's style.
struct Ghost {
    text: gpui::SharedString,
    mode: mdv_theme::Mode,
}

impl Render for Ghost {
    fn render(&mut self, _w: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let p = *self.mode.palette();
        gpui::div()
            .size_full()
            .flex()
            .items_center()
            .px(px(8.0))
            .bg(crate::ui::c(p.bg_bar))
            .border_1()
            .border_color(crate::ui::c(p.border))
            .text_color(crate::ui::c(p.text))
            .font_family("Geist Mono")
            .text_size(px(12.0))
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .child(self.text.clone())
    }
}

/// Offset of the label from the pointer, logical pixels, so it never sits under the hot spot.
const GHOST_OFFSET: (f32, f32) = (14.0, 18.0);
const GHOST_HEIGHT: f32 = 26.0;
const GHOST_PREPARE_DELAY: Duration = Duration::from_millis(500);

struct LiveDrag {
    source: WindowId,
    /// The latest pointer position in the source window's logical coordinates.
    last_local: Point<Pixels>,
    target: Target,
    marker_in: Option<WeakEntity<AppView>>,
}

#[derive(Default)]
pub struct Registry {
    windows: Vec<WindowEntry>,
    machine: Machine<TabId>,
    press_window: Option<WindowId>,
    drag: Option<LiveDrag>,
    ghost: Option<GhostWindow>,
    ghost_failed: bool,
    /// When the live drag started, until its label has been shown.
    drag_started: Option<Instant>,
    /// The press that ended on this frame was a drag, so the element's click must be ignored.
    swallow_click: bool,
    log: Option<File>,
    last_active: Option<WindowId>,
    last_layout: String,
    started: Option<Instant>,
    open_queue: Vec<String>,
    /// Evidence for "what does GPUI deliver during a drag": per-drag input counts.
    moves_in_source: u32,
    moves_outside_source_bounds: u32,
    moves_in_other_windows: u32,
    releases_in_other_windows: u32,
}

impl Global for Registry {}

/// One-time set-up: the registry, and quitting when the last window closes.
pub fn install(cx: &mut App) {
    let mut registry = Registry::default();
    if let Some(path) = crate::harness::tab_log() {
        registry.log = File::create(path).ok();
    }
    registry.started = Some(Instant::now());
    if let Some(q) = crate::harness::open_queue() {
        registry.open_queue = q.to_string_lossy().split(',').rev().map(str::to_string).collect();
    }
    cx.set_global(registry);
    serve_live_reload(cx);
    cx.on_window_closed(|cx, id| {
        let reg = cx.global_mut::<Registry>();
        reg.windows.retain(|w| w.id != id);
        if reg.ghost.as_ref().is_some_and(|g| g.handle.window_id() == id) {
            reg.ghost = None;
        }
        let source_closed = reg.drag.as_ref().is_some_and(|d| d.source == id);
        if source_closed {
            cancel(CancelReason::SourceClosed, cx);
        }
        log_state("window-closed", "", cx);
        if cx.global::<Registry>().windows.is_empty() {
            log_state("quit", "", cx);
            cx.quit();
        }
    })
    .detach();
}

/// Where file watchers report which tab's file changed.
static LIVE: std::sync::OnceLock<async_channel::Sender<TabId>> = std::sync::OnceLock::new();

/// Starts the task that turns file changes into reloads. Nothing polls here: the task sleeps on the
/// channel until a watcher sends.
fn serve_live_reload(cx: &mut App) {
    let (tx, rx) = async_channel::unbounded::<TabId>();
    if LIVE.set(tx).is_err() {
        return;
    }
    cx.spawn(async move |cx| {
        while let Ok(id) = rx.recv().await {
            cx.update(|cx| AppView::reload_tab_anywhere(id, crate::view::Reload::Live { retry: true }, cx));
        }
    })
    .detach();
}

/// A watch on `path` that reports to the live reload task as tab `id`.
pub fn watch_tab(id: TabId, path: &str) -> Option<crate::watch::Watch> {
    let tx = LIVE.get()?.clone();
    Some(crate::watch::Watch::new(path.into(), move || {
        let _ = tx.try_send(id);
    }))
}

/// Every viewer window's view.
pub fn all_views(cx: &App) -> Vec<Entity<AppView>> {
    cx.global::<Registry>().windows.iter().filter_map(|w| w.view.upgrade()).collect()
}

/// The view that holds tab `id` now, wherever the tab has been moved to.
pub fn view_of_tab(id: TabId, cx: &App) -> Option<Entity<AppView>> {
    all_views(cx).into_iter().find(|v| v.read(cx).tabs.iter().any(|t| t.id == id))
}

/// Options shared by the first window and every detached one.
pub fn viewer_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_background: WindowBackgroundAppearance::Opaque,
        // Linux (X11 and Wayland): draw our own frame. Windows and macOS use `titlebar` below.
        window_decorations: Some(WindowDecorations::Client),
        window_min_size: Some(size(px(320.0), px(240.0))),
        app_id: Some("md-viewer".into()),
        // Windows and macOS: no system title bar; the app draws the header. On macOS the native
        // traffic lights stay, placed inside the 88px left inset. Position is a guess (untested).
        titlebar: Some(TitlebarOptions {
            title: Some("MD Viewer".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(14.0), px(17.0))),
        }),
        ..Default::default()
    }
}

pub fn register(view: &Entity<AppView>, window: &mut Window, cx: &mut App) {
    let native = raw_window_handle::HasWindowHandle::window_handle(&*window)
        .map(|h| NativeWindow::from_raw(h.as_raw()))
        .unwrap_or(NativeWindow::Other(Backend::Unknown));
    let entry = WindowEntry {
        view: view.downgrade(),
        handle: window.window_handle(),
        id: window.window_handle().window_id(),
        native,
        scale: window.scale_factor(),
    };
    cx.global_mut::<Registry>().windows.push(entry);
    log_state("window-open", "", cx);
}

// ---- input --------------------------------------------------------------------------------------

/// A press on a tab. Called from the tab element.
pub fn press(tab: TabId, at: Point<Pixels>, window: &Window, cx: &mut App) {
    let reg = cx.global_mut::<Registry>();
    reg.machine.press(tab, (f32::from(at.x), f32::from(at.y)));
    reg.press_window = Some(window.window_handle().window_id());
    reg.swallow_click = false;
}

/// True once if the click now being delivered ended a drag rather than being a click.
pub fn take_swallow_click(cx: &mut App) -> bool {
    std::mem::take(&mut cx.global_mut::<Registry>().swallow_click)
}

/// Escape during a drag. Attached to the view root: GPUI sends key events only along the focus path,
/// so the event sink below, being a leaf, would never hear them.
pub fn capture_key_down(e: &KeyDownEvent, cx: &mut App) {
    if e.keystroke.key == "escape" && cancel(CancelReason::Escape, cx) {
        cx.stop_propagation();
    }
}

/// A zero-size element whose only job is to listen to raw window events, which GPUI delivers
/// regardless of the pointer position. Element listeners (`on_mouse_move`) filter by hover and would
/// go quiet as soon as the pointer left the window.
pub fn event_sink() -> impl IntoElement {
    canvas(
        |_, _, _| {},
        |_, _, window, cx| {
            // A window can change scale by moving to another monitor.
            let (id, scale) = (window.window_handle().window_id(), window.scale_factor());
            let active = window.is_window_active();
            let reg = cx.global_mut::<Registry>();
            if active {
                reg.last_active = Some(id);
            }
            if let Some(w) = reg.windows.iter_mut().find(|w| w.id == id) {
                w.scale = scale;
            }
            // Evidence log only: report when any strip's geometry settles somewhere new.
            if cx.global::<Registry>().log.is_some() {
                cx.defer(log_layout_if_changed);
            }
            // A closed hand on the source window while a drag is live. With the pointer captured
            // this is also what shows outside the window, where the platform honours it.
            let reg = cx.global::<Registry>();
            if reg.drag.is_some() && reg.press_window == Some(id) {
                window.set_window_cursor_style(CursorStyle::ClosedHand);
            }
            window.on_mouse_event(|e: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture {
                    on_move(e, window, cx);
                }
            });
            window.on_mouse_event(|e: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && e.button == MouseButton::Left {
                    on_release(e, window, cx);
                }
            });
        },
    )
    .absolute()
    .size_0()
}

fn on_move(e: &MouseMoveEvent, window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let viewport = window.viewport_size();
    let reg = cx.global_mut::<Registry>();
    if reg.press_window != Some(id) {
        if reg.drag.is_some() {
            reg.moves_in_other_windows += 1;
        }
        return;
    }
    if reg.drag.is_some() {
        reg.moves_in_source += 1;
        let (x, y) = (e.position.x, e.position.y);
        if x < px(0.0) || y < px(0.0) || x > viewport.width || y > viewport.height {
            reg.moves_outside_source_bounds += 1;
        }
    }
    let at = (f32::from(e.position.x), f32::from(e.position.y));
    let down = e.pressed_button == Some(MouseButton::Left);
    match reg.machine.moved(at, down) {
        Step::Started { tab } => {
            reg.drag = Some(LiveDrag { source: id, last_local: e.position, target: Target::Unknown, marker_in: None });
            reg.swallow_click = true;
            reg.drag_started = Some(Instant::now());
            log_state("drag-start", &format!("\"tab\":{tab}"), cx);
            set_moving(Some(tab), cx);
            start_autoscroll(cx);
            cx.defer(update_target);
        }
        Step::Moved { .. } => {
            if let Some(drag) = reg.drag.as_mut() {
                drag.last_local = e.position;
            }
            cx.defer(update_target);
        }
        Step::Cancelled { reason } => finish_cancel(reason, cx),
        _ => {}
    }
}

fn on_release(e: &MouseUpEvent, window: &mut Window, cx: &mut App) {
    let id = window.window_handle().window_id();
    let reg = cx.global_mut::<Registry>();
    if reg.press_window != Some(id) {
        if reg.drag.is_some() {
            reg.releases_in_other_windows += 1;
        }
        return;
    }
    reg.press_window = None;
    if let Step::Released { tab } = reg.machine.released() {
        if let Some(drag) = reg.drag.as_mut() {
            drag.last_local = e.position;
        }
        cx.defer(move |cx| finish_drop(tab, cx));
    }
}

/// Cancels a live drag. False if there was none, so Escape keeps its ordinary meaning.
pub fn cancel(reason: CancelReason, cx: &mut App) -> bool {
    match cx.global_mut::<Registry>().machine.cancel(reason) {
        Some(_) => {
            finish_cancel(reason, cx);
            true
        }
        None => false,
    }
}

fn finish_cancel(reason: CancelReason, cx: &mut App) {
    clear_marker(cx);
    remove_ghost(cx);
    set_moving(None, cx);
    let reg = cx.global_mut::<Registry>();
    reg.drag = None;
    log_state("cancel", &format!("\"reason\":\"{reason:?}\""), cx);
}

// ---- hit testing --------------------------------------------------------------------------------

fn resolve(cx: &mut App) -> Target {
    let reg = cx.global::<Registry>();
    let Some(drag) = reg.drag.as_ref() else { return Target::Unknown };
    let Some(source_ix) = reg.windows.iter().position(|w| w.id == drag.source) else { return Target::Unknown };
    let local = drag.last_local;

    // Platforms with no desktop coordinates (Wayland, and macOS until it has an adapter) can still
    // reorder inside the source strip. Anything else on them cancels.
    let Some(at) = pointer_desktop(cx) else {
        return target_in_window(source_ix, local, cx).unwrap_or(Target::Unknown);
    };
    let known: Vec<NativeWindow> = reg.windows.iter().map(|w| w.native).collect();
    let ignore: Vec<NativeWindow> = reg.ghost.iter().map(|g| g.native).collect();
    match native::hit_at(&known, &ignore, at) {
        None => Target::Unknown,
        Some(Hit::Outside) => Target::Outside(at),
        Some(Hit::Ours(i)) => {
            let w = &reg.windows[i];
            let Some(o) = native::client_origin(w.native) else { return Target::Unknown };
            let local = point(px((at.x - o.x) as f32 / w.scale), px((at.y - o.y) as f32 / w.scale));
            target_in_window(i, local, cx).unwrap_or(Target::InViewer)
        }
    }
}

/// The pointer on the desktop in physical pixels, or `None` where the platform cannot say.
fn pointer_desktop(cx: &App) -> Option<DesktopPoint> {
    let reg = cx.global::<Registry>();
    let drag = reg.drag.as_ref()?;
    let source = reg.windows.iter().find(|w| w.id == drag.source)?;
    let origin = native::client_origin(source.native)?;
    Some(DesktopPoint {
        x: origin.x + (f32::from(drag.last_local.x) * source.scale).round() as i32,
        y: origin.y + (f32::from(drag.last_local.y) * source.scale).round() as i32,
    })
}

/// Shows the label on first call during a drag, then moves it with the pointer. Omitted where the
/// platform gives no desktop coordinates (Wayland), as it could not be placed.
fn update_ghost(cx: &mut App) {
    let Some(at) = pointer_desktop(cx) else { return };
    let (tab, source_scale, backend, mode) = {
        let reg = cx.global::<Registry>();
        let Some(drag) = reg.drag.as_ref() else { return };
        let Some(w) = reg.windows.iter().find(|w| w.id == drag.source) else { return };
        let Some(view) = w.view.upgrade() else { return };
        let v = view.read(cx);
        let name = v.moving.and_then(|id| v.tabs.iter().find(|t| t.id == id)).map(|t| t.name.clone());
        (name, w.scale, w.native.backend(), v.mode)
    };
    let scale = native::scale_at(backend, at).unwrap_or(source_scale);
    if let Some(g) = cx.global_mut::<Registry>().ghost.as_mut().filter(|g| g.shown) {
        g.rect = ghost_rect(at, g.size, scale);
        native::place_ghost(g.native, g.rect);
        return;
    }
    let Some(text) = tab else { return };
    let chars = text.chars().count() as f32;
    let size = ((chars * 7.4 + 18.0).clamp(60.0, 200.0), GHOST_HEIGHT);
    let rect = ghost_rect(at, size, scale);
    // A window made at one density cannot be relied on to change while hidden, so this makes a new
    // one where the old one would be wrong. Only a screen of another density comes this way.
    if cx.global::<Registry>().ghost.as_ref().is_some_and(|g| (g.scale - scale).abs() > 0.01) {
        if let Some(old) = cx.global_mut::<Registry>().ghost.take() {
            old.handle.update(cx, |_, w, _| w.remove_window()).ok();
        }
    }
    let Some(view) = cx.global::<Registry>().ghost.as_ref().map(|g| g.view.clone()) else {
        open_ghost(Some(rect), at, scale, text, mode, cx);
        return;
    };
    view.update(cx, |g, cx| {
        g.text = text;
        g.mode = mode;
        cx.notify();
    });
    if let Some(g) = cx.global_mut::<Registry>().ghost.as_mut() {
        g.size = size;
        g.rect = rect;
        g.shown = true;
    }
    log_state("ghost-open", "", cx);
    // From a task, not here. App is borrowed here, and resizing and showing the window call back
    // into the window procedure, which needs App to take the new size and draw the new text. On
    // Windows the show itself draws, before the window appears, so the last name never flashes.
    cx.spawn(async move |cx| {
        let wanted = cx.update(|cx| cx.global::<Registry>().ghost.as_ref().filter(|g| g.shown).map(|g| (g.native, g.rect)));
        if let Some((native_window, rect)) = wanted {
            native::show_ghost(native_window, rect);
            cx.update(note_ghost_shown);
        }
    })
    .detach();
}

fn note_ghost_shown(cx: &mut App) {
    let since = cx.global_mut::<Registry>().drag_started.take();
    if let Some(since) = since {
        let ms = since.elapsed().as_secs_f64() * 1000.0;
        crate::timing::log(&format!("label_shown ms_since_drag_start={ms:.1}"));
        log_state("ghost-shown", &format!("\"ms\":{ms:.1}"), cx);
    }
}

/// Makes the label's window, which is hidden unless `shown_at` says where to show it.
fn open_ghost(shown_at: Option<DesktopRect>, at: DesktopPoint, scale: f32, text: gpui::SharedString, mode: mdv_theme::Mode, cx: &mut App) {
    let started = Instant::now();
    let size = (60.0, GHOST_HEIGHT);
    let bounds = match shown_at {
        Some(rect) => to_logical(rect, scale),
        // Parked: an X11 window is mapped whatever `show` says, until the call below hides it.
        None => Bounds::new(point(px(-10000.0), px(-10000.0)), gpui::size(px(size.0), px(size.1))),
    };
    let on_screen = point(px(at.x as f32 / scale), px(at.y as f32 / scale));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_background: WindowBackgroundAppearance::Opaque,
        kind: WindowKind::PopUp,
        titlebar: None,
        focus: false,
        show: shown_at.is_some(),
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("md-viewer".into()),
        display_id: cx.displays().iter().find(|d| d.bounds().contains(&on_screen)).map(|d| d.id()),
        ..Default::default()
    };
    let mut made = None;
    let opened = cx.open_window(options, |window, cx| {
        let native_window = raw_window_handle::HasWindowHandle::window_handle(&*window)
            .map(|h| NativeWindow::from_raw(h.as_raw()))
            .unwrap_or(NativeWindow::Other(Backend::Unknown));
        let view = cx.new(|_| Ghost { text, mode });
        made = Some((native_window, view.clone(), window.scale_factor()));
        view
    });
    match (opened, made) {
        (Ok(handle), Some((native_window, view, window_scale))) => {
            native::prepare_ghost(native_window);
            let rect = shown_at.unwrap_or(DesktopRect { x: -10000, y: -10000, width: 0, height: 0 });
            match shown_at {
                Some(rect) => native::place_ghost(native_window, rect),
                None => native::hide_ghost(native_window),
            }
            cx.global_mut::<Registry>().ghost = Some(GhostWindow {
                handle: handle.into(),
                native: native_window,
                view,
                scale: window_scale,
                size,
                rect,
                shown: shown_at.is_some(),
            });
            let ms = started.elapsed().as_secs_f64() * 1000.0;
            crate::timing::log(&format!("label_window_created ms={ms:.1} hidden={}", shown_at.is_none()));
            log_state("ghost-created", &format!("\"ms\":{ms:.1},\"hidden\":{}", shown_at.is_none()), cx);
            if shown_at.is_some() {
                log_state("ghost-open", "", cx);
                note_ghost_shown(cx);
            }
        }
        (opened, _) => {
            // No label is not a failed drag, and it will not work any better on the next move.
            let why = opened.err().map_or(String::new(), |e| e.to_string().replace('"', "'"));
            cx.global_mut::<Registry>().ghost_failed = true;
            log_state("ghost-failed", &format!("\"why\":\"{why}\""), cx);
        }
    }
}

/// Makes the hidden label shortly after launch, so the first drag has nothing to wait for. Late
/// enough not to delay the first window; a drag that gets in first makes it itself.
pub fn prepare_ghost_soon(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(GHOST_PREPARE_DELAY).await;
        cx.update(prepare_ghost);
    })
    .detach();
}

fn prepare_ghost(cx: &mut App) {
    let reg = cx.global::<Registry>();
    if reg.ghost.is_some() || reg.ghost_failed || reg.drag.is_some() {
        return;
    }
    let Some(w) = reg.windows.first() else { return };
    // No desktop coordinates (Wayland): there will never be a label to place.
    let Some(at) = native::client_origin(w.native) else { return };
    let scale = w.scale;
    let Some(view) = w.view.upgrade() else { return };
    let mode = view.read(cx).mode;
    open_ghost(None, at, scale, gpui::SharedString::default(), mode, cx);
}

fn ghost_rect(at: DesktopPoint, size: (f32, f32), scale: f32) -> DesktopRect {
    DesktopRect {
        x: at.x + (GHOST_OFFSET.0 * scale).round() as i32,
        y: at.y + (GHOST_OFFSET.1 * scale).round() as i32,
        width: (size.0 * scale).round() as i32,
        height: (size.1 * scale).round() as i32,
    }
}

/// Hides the label until the next drag. Safe to call when there is none.
fn remove_ghost(cx: &mut App) {
    let reg = cx.global_mut::<Registry>();
    reg.drag_started = None;
    if let Some(g) = reg.ghost.as_mut().filter(|g| g.shown) {
        g.shown = false;
        native::hide_ghost(g.native);
        log_state("ghost-closed", "", cx);
    }
}

/// The strip target if `local` is on window `i`'s strip.
fn target_in_window(i: usize, local: Point<Pixels>, cx: &App) -> Option<Target> {
    let view = cx.global::<Registry>().windows.get(i)?.view.upgrade()?;
    let view = view.read(cx);
    let strip = view.tab_scroll.bounds();
    if !strip.contains(&local) {
        return None;
    }
    let scroll = f32::from(view.tab_scroll.offset().x);
    let rects = (0..view.tabs.len()).filter_map(|t| {
        let b = view.tab_scroll.bounds_for_item(t)?;
        Some((f32::from(b.origin.x) + scroll, f32::from(b.size.width)))
    });
    Some(Target::Strip { window: i, index: insertion_index(f32::from(local.x), rects) })
}

fn update_target(cx: &mut App) {
    if cx.global::<Registry>().drag.is_none() {
        return;
    }
    update_ghost(cx);
    let target = resolve(cx);
    let reg = cx.global_mut::<Registry>();
    let Some(drag) = reg.drag.as_mut() else { return };
    let changed = drag.target != target;
    drag.target = target;
    if changed {
        show_marker(target, cx);
        log_state("hover", &format!("\"target\":\"{target:?}\""), cx);
    }
}

fn clear_marker(cx: &mut App) {
    let old = cx.global_mut::<Registry>().drag.as_mut().and_then(|d| d.marker_in.take());
    if let Some(view) = old.and_then(|w| w.upgrade()) {
        view.update(cx, |v, cx| {
            v.drop_marker = None;
            cx.notify();
        });
    }
}

fn show_marker(target: Target, cx: &mut App) {
    clear_marker(cx);
    let Target::Strip { window, index } = target else { return };
    let Some(view) = cx.global::<Registry>().windows.get(window).and_then(|w| w.view.upgrade()) else { return };
    view.update(cx, |v, cx| {
        let strip = v.tab_scroll.bounds();
        let scroll = f32::from(v.tab_scroll.offset().x);
        let edge = |t: usize, right: bool| {
            v.tab_scroll.bounds_for_item(t).map(|b| {
                f32::from(b.origin.x) + scroll + if right { f32::from(b.size.width) } else { 0.0 }
            })
        };
        let x = edge(index, false)
            .or_else(|| index.checked_sub(1).and_then(|p| edge(p, true)))
            .unwrap_or(f32::from(strip.origin.x));
        let x = x.clamp(f32::from(strip.origin.x), f32::from(strip.origin.x + strip.size.width) - 2.0);
        v.drop_marker = Some(crate::view::DropMarker { x, top: f32::from(strip.origin.y) + 5.0, height: f32::from(strip.size.height) - 10.0 });
        cx.notify();
    });
    let weak = view.downgrade();
    if let Some(d) = cx.global_mut::<Registry>().drag.as_mut() {
        d.marker_in = Some(weak);
    }
}

/// Dims the tab being moved in its source strip, like `.moving` in the web view.
fn set_moving(tab: Option<TabId>, cx: &mut App) {
    let source = cx.global::<Registry>().drag.as_ref().map(|d| d.source);
    let view = source.and_then(|id| {
        cx.global::<Registry>().windows.iter().find(|w| w.id == id).and_then(|w| w.view.upgrade())
    });
    if let Some(view) = view {
        view.update(cx, |v, cx| {
            v.moving = tab;
            cx.notify();
        });
    }
}

/// Edge autoscroll for an overflowed strip, as `movement.js` `scroll` does: within 36 px of an edge,
/// 12 px per frame, for as long as the pointer stays there, even if it stops moving.
fn start_autoscroll(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(16)).await;
            let keep_going = cx.update(autoscroll_tick);
            if !keep_going {
                break;
            }
        }
    })
    .detach();
}

fn autoscroll_tick(cx: &mut App) -> bool {
    let reg = cx.global::<Registry>();
    let Some(drag) = reg.drag.as_ref() else { return false };
    let Target::Strip { window, .. } = drag.target else { return true };
    let Some(w) = reg.windows.get(window) else { return true };
    let Some(view) = w.view.upgrade() else { return true };
    // The pointer position in the hovered window's coordinates, via the desktop.
    let source = reg.windows.iter().find(|s| s.id == drag.source);
    let x = match (source, native::client_origin(w.native)) {
        (Some(s), Some(o)) => {
            let Some(so) = native::client_origin(s.native) else { return true };
            let desktop = so.x as f32 + f32::from(drag.last_local.x) * s.scale;
            (desktop - o.x as f32) / w.scale
        }
        _ => f32::from(drag.last_local.x),
    };
    let moved = view.update(cx, |v, cx| {
        let strip = v.tab_scroll.bounds();
        let max = f32::from(v.tab_scroll.max_offset().x);
        if max <= 0.0 {
            return false;
        }
        let left = f32::from(strip.origin.x);
        let right = left + f32::from(strip.size.width);
        let delta = if x < left + 36.0 {
            12.0
        } else if x > right - 36.0 {
            -12.0
        } else {
            return false;
        };
        let offset = v.tab_scroll.offset();
        let next = (f32::from(offset.x) + delta).clamp(-max, 0.0);
        if next == f32::from(offset.x) {
            return false;
        }
        v.tab_scroll.set_offset(point(px(next), offset.y));
        cx.notify();
        true
    });
    if moved {
        // The tabs moved under a still pointer, so the marker must be worked out again.
        cx.defer(|cx| {
            let target = resolve(cx);
            let reg = cx.global_mut::<Registry>();
            if let Some(d) = reg.drag.as_mut() {
                d.target = target;
            }
            show_marker(target, cx);
            log_state("autoscroll", "", cx);
        });
    }
    true
}

// ---- completing a move ---------------------------------------------------------------------------

fn finish_drop(tab: TabId, cx: &mut App) {
    let target = if cx.global::<Registry>().drag.is_some() { resolve(cx) } else { Target::Unknown };
    clear_marker(cx);
    remove_ghost(cx);
    set_moving(None, cx);
    let Some(drag) = cx.global_mut::<Registry>().drag.take() else { return };
    let reg = cx.global_mut::<Registry>();
    let input = format!(
        "\"moves_in_source\":{},\"moves_outside_source_bounds\":{},\"moves_in_other_windows\":{},\"releases_in_other_windows\":{},\"release_local\":[{},{}]",
        std::mem::take(&mut reg.moves_in_source),
        std::mem::take(&mut reg.moves_outside_source_bounds),
        std::mem::take(&mut reg.moves_in_other_windows),
        std::mem::take(&mut reg.releases_in_other_windows),
        f32::from(drag.last_local.x),
        f32::from(drag.last_local.y),
    );
    log_state("drag-input", &input, cx);
    let source = cx.global::<Registry>().windows.iter().position(|w| w.id == drag.source);
    let Some(source) = source else {
        log_state("drop-ignored", "\"why\":\"source gone\"", cx);
        return;
    };
    match target {
        Target::Strip { window, index } if window == source => reorder(source, tab, index, cx),
        Target::Strip { window, index } => merge(source, tab, window, index, cx),
        Target::Outside(at) => detach(source, tab, at, cx),
        Target::InViewer => log_state("drop-cancelled", "\"why\":\"inside a viewer, off the strip\"", cx),
        Target::Unknown => log_state("drop-cancelled", "\"why\":\"pointer position unknown on this platform\"", cx),
    }
}

fn reorder(source: usize, tab: TabId, index: usize, cx: &mut App) {
    let Some(view) = cx.global::<Registry>().windows[source].view.upgrade() else { return };
    let moved = view.update(cx, |v, cx| {
        let moved = v.reorder_tab(tab, index);
        cx.notify();
        moved
    });
    log_state("reorder", &format!("\"tab\":{tab},\"index\":{index},\"moved\":{moved}"), cx);
}

fn fault(name: &str) -> bool {
    crate::harness::tab_move_fault(name)
}

fn merge(source: usize, tab: TabId, dest: usize, index: usize, cx: &mut App) {
    let reg = cx.global::<Registry>();
    let (src, src_handle) = (reg.windows[source].view.upgrade(), reg.windows[source].handle);
    let (dst, dst_handle) = (reg.windows[dest].view.upgrade(), reg.windows[dest].handle);
    let (Some(src), Some(dst)) = (src, dst) else {
        log_state("move-failed", "\"why\":\"a window went away\"", cx);
        return;
    };
    if fault("merge") {
        log_state("move-failed", "\"why\":\"injected merge fault\"", cx);
        return;
    }
    let Some(data) = src.update(cx, |v, cx| v.take_tab(tab, cx)) else {
        log_state("move-failed", "\"why\":\"tab not in source\"", cx);
        return;
    };
    dst.update(cx, |v, cx| v.insert_tab(data, index, cx));
    finish_move(&src, src_handle, dst_handle, cx);
    log_state("merge", &format!("\"tab\":{tab},\"index\":{index}"), cx);
}

fn detach(source: usize, tab: TabId, at: DesktopPoint, cx: &mut App) {
    let (src_handle, native, scale) = {
        let w = &cx.global::<Registry>().windows[source];
        (w.handle, w.native, w.scale)
    };
    let Some(src) = cx.global::<Registry>().windows[source].view.upgrade() else { return };
    let backend = native.backend();
    let Some(area) = native::work_area_at(backend, at) else {
        log_state("move-failed", "\"why\":\"no work area at the drop point\"", cx);
        return;
    };
    let placed = place_detached(at, area);
    let scale = native::scale_at(backend, at).unwrap_or(scale);
    let bounds = to_logical(placed, scale);

    let opened = if fault("detach") {
        Err(anyhow::anyhow!("injected detach fault"))
    } else {
        let mut options = viewer_options(bounds);
        options.display_id = cx
            .displays()
            .iter()
            .find(|d| d.bounds().contains(&bounds.origin))
            .map(|d| d.id());
        open_viewer(options, |_, _| {}, cx)
    };
    let dst = match opened {
        Ok(view) => view,
        Err(err) => {
            log_state("move-failed", &format!("\"why\":\"{}\"", err.to_string().replace('"', "'")), cx);
            return;
        }
    };
    let dst_entry = cx.global::<Registry>().windows.last();
    let dst_handle = dst_entry.map(|w| w.handle);
    if let Some(w) = dst_entry {
        native::place_window(w.native, placed);
    }
    let Some(data) = src.update(cx, |v, cx| v.take_tab(tab, cx)) else {
        // The tab vanished between the release and now. Do not leave an empty window behind.
        if let Some(h) = dst_handle {
            h.update(cx, |_, w, _| w.remove_window()).ok();
        }
        log_state("move-failed", "\"why\":\"tab not in source\"", cx);
        return;
    };
    dst.update(cx, |v, cx| v.insert_tab(data, 0, cx));
    if let Some(h) = dst_handle {
        finish_move(&src, src_handle, h, cx);
    }
    log_state(
        "detach",
        &format!("\"tab\":{tab},\"at\":[{},{}],\"placed\":[{},{},{},{}],\"area\":[{},{},{},{}]",
            at.x, at.y, placed.x, placed.y, placed.width, placed.height, area.x, area.y, area.width, area.height),
        cx,
    );
}

fn to_logical(r: DesktopRect, scale: f32) -> Bounds<Pixels> {
    Bounds::new(
        point(px(r.x as f32 / scale), px(r.y as f32 / scale)),
        size(px(r.width as f32 / scale), px(r.height as f32 / scale)),
    )
}

/// Closes the source if its last tab left, then brings the destination forward.
fn finish_move(src: &Entity<AppView>, src_handle: AnyWindowHandle, dst_handle: AnyWindowHandle, cx: &mut App) {
    if src.read(cx).tabs.is_empty() {
        src_handle.update(cx, |_, w, _| w.remove_window()).ok();
    }
    dst_handle
        .update(cx, |_, w, _| w.activate_window())
        .ok();
}

/// Opens one viewer window and registers it. Used by `main` and by detaching.
pub fn open_viewer(
    options: WindowOptions,
    init: impl FnOnce(&mut AppView, &mut gpui::Context<AppView>) + 'static,
    cx: &mut App,
) -> anyhow::Result<Entity<AppView>> {
    let mut created = None;
    let result = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| {
            let mut v = AppView::new(cx);
            init(&mut v, cx);
            v
        });
        let focus = view.read(cx).root_focus.clone();
        window.focus(&focus, cx);
        register(&view, window, cx);
        created = Some(view.clone());
        view
    });
    result?;
    created.ok_or_else(|| anyhow::anyhow!("the window closure did not run"))
}

// ---- later launches -----------------------------------------------------------------------------

/// Listens for launches handing over files. Nothing here polls: the UI task sleeps on the channel.
pub fn serve_launches(owner: Owner, cx: &mut App) {
    let (tx, rx) = async_channel::unbounded::<Request>();
    owner.serve(move |request| {
        // If the app has gone, the request drops unanswered and that launch retries.
        let _ = tx.send_blocking(request);
    });
    cx.spawn(async move |cx| {
        while let Ok(request) = rx.recv().await {
            cx.update(|cx| handle_launch(request, cx));
        }
    })
    .detach();
}

/// The most recently focused window, which is where a new file goes.
fn front_view(cx: &App) -> Option<(Entity<AppView>, AnyWindowHandle)> {
    let reg = cx.global::<Registry>();
    let active = cx.active_window().map(|h| h.window_id());
    let pick = |id: Option<WindowId>| id.and_then(|id| reg.windows.iter().find(|w| w.id == id));
    let entry = pick(active).or_else(|| pick(reg.last_active)).or_else(|| reg.windows.last())?;
    Some((entry.view.upgrade()?, entry.handle))
}

fn handle_launch(request: Request, cx: &mut App) {
    if open_in_front(&request.paths, cx) {
        request.acknowledge();
    }
}

/// Opens files handed over by another launch or by the system, in the most recent window, after the
/// same checks as any other open. Returns false when there is no window to open them in.
pub fn open_in_front(paths: &[std::path::PathBuf], cx: &mut App) -> bool {
    let Some((view, handle)) = front_view(cx) else {
        // No window any more: the app is closing. Unanswered, so that launch starts afresh.
        return false;
    };
    let mut opened = 0;
    let mut refused = Vec::new();
    for path in paths {
        match single_instance::validate_path(path) {
            Ok(path) => {
                view.update(cx, |v, cx| v.open_path(path, None, cx));
                opened += 1;
            }
            Err(why) => refused.push(why),
        }
    }
    if let Some(why) = refused.first() {
        let message = format!("Could not open a file from another launch: {why}");
        view.update(cx, |v, cx| v.show_toast(&message, false, cx));
    }
    // A launch with an existing file has already raised that file's window, and the last one wins.
    if !paths.iter().any(|p| single_instance::validate_path(p).is_ok() && is_elsewhere(p, &view, cx)) {
        handle.update(cx, |_, w, _| w.activate_window()).ok();
    }
    log_state("launch", &format!("\"opened\":{opened},\"refused\":{}", refused.len()), cx);
    true
}

fn is_elsewhere(path: &std::path::Path, front: &Entity<AppView>, cx: &App) -> bool {
    let Ok(canonical) = dunce::canonicalize(path) else { return false };
    let shown = canonical.to_string_lossy();
    cx.global::<Registry>().windows.iter().filter_map(|w| w.view.upgrade()).any(|v| {
        v != *front && v.read(cx).tabs.iter().any(|t| t.path.as_ref() == shown)
    })
}

// ---- one tab per file ----------------------------------------------------------------------------

/// Opens `path` in the front window, unless a tab anywhere already has it, in which case that tab
/// and its window come forward. Reached with Ctrl+O when the test harness set a queue.
pub fn open_next_from_queue(this: &mut AppView, cx: &mut gpui::Context<AppView>) -> bool {
    let Some(path) = cx.global_mut::<Registry>().open_queue.pop() else { return false };
    let me = cx.entity();
    let mut existing = None;
    for w in &cx.global::<Registry>().windows {
        if let Some(view) = w.view.upgrade() {
            if view == me {
                if let Some(id) = this.tabs.iter().find(|t| t.path.as_ref() == path).map(|t| t.id) {
                    existing = Some((None, id, w.handle));
                }
            } else if let Some(id) = view.read(cx).tabs.iter().find(|t| t.path.as_ref() == path).map(|t| t.id) {
                existing = Some((Some(view.clone()), id, w.handle));
            }
        }
    }
    match existing {
        Some((None, id, _)) => {
            this.activate_tab(id, cx);
            log_state_soon("open-existing", format!("\"path\":\"{path}\",\"tab\":{id}"), cx);
        }
        Some((Some(view), id, handle)) => {
            view.update(cx, |v, cx| v.activate_tab(id, cx));
            handle.update(cx, |_, w, _| w.activate_window()).ok();
            log_state_soon("open-existing", format!("\"path\":\"{path}\",\"tab\":{id}"), cx);
        }
        None => {
            let name = path.rsplit('/').next().unwrap_or(&path).to_string();
            let data = TabData::new(name.into(), path.clone().into(), Rc::new(Vec::new()));
            let at = this.tabs.len();
            this.insert_tab(data, at, cx);
            log_state_soon("open-new", format!("\"path\":\"{path}\""), cx);
        }
    }
    true
}

/// Brings forward a tab holding `path` in any window but the one being updated. Reading the calling
/// view here would panic, which is why it checks its own tabs first.
pub fn focus_in_other_window(path: &str, fragment: Option<&str>, cx: &mut gpui::Context<AppView>) -> bool {
    let me = cx.entity();
    let targets: Vec<_> = cx
        .global::<Registry>()
        .windows
        .iter()
        .filter_map(|w| w.view.upgrade().map(|v| (v, w.handle)))
        .filter(|(v, _)| *v != me)
        .collect();
    for (view, handle) in targets {
        let id = view.read(cx).tabs.iter().find(|t| t.path.as_ref() == path).map(|t| t.id);
        if let Some(id) = id {
            let fragment = fragment.map(str::to_string);
            view.update(cx, |v, cx| {
                v.activate_tab(id, cx);
                if let (Some(f), Some(r)) = (fragment, v.tabs.iter().find(|t| t.id == id).and_then(|t| t.reader.clone())) {
                    r.update(cx, |r, cx| r.jump_to_fragment(&f, cx));
                }
            });
            handle.update(cx, |_, w, _| w.activate_window()).ok();
            log_state_soon("open-existing", format!("\"path\":\"{path}\",\"tab\":{id}"), cx);
            return true;
        }
    }
    false
}

fn log_state_soon(event: &'static str, extra: String, cx: &mut App) {
    cx.defer(move |cx| log_state(event, &extra, cx));
}

// ---- evidence log --------------------------------------------------------------------------------

/// With the test harness's log, appends one JSON line per state change. This is how the tests read the model
/// after real input; it never drives anything.
pub fn log_state(event: &str, extra: &str, cx: &mut App) {
    if cx.global::<Registry>().log.is_none() {
        return;
    }
    let windows = windows_json(cx);
    let reg = cx.global_mut::<Registry>();
    let t = reg.started.map_or(0.0, |s| s.elapsed().as_secs_f64());
    let sep = if extra.is_empty() { "" } else { "," };
    let line = format!("{{\"t\":{t:.3},\"ev\":\"{event}\"{sep}{extra},\"windows\":[{}]}}\n", windows.join(","));
    if let Some(log) = reg.log.as_mut() {
        log.write_all(line.as_bytes()).ok();
        log.flush().ok();
    }
}

fn log_layout_if_changed(cx: &mut App) {
    let signature = windows_json(cx).join(",");
    if cx.global::<Registry>().last_layout != signature {
        cx.global_mut::<Registry>().last_layout = signature;
        log_state("layout", "", cx);
    }
}

fn windows_json(cx: &mut App) -> Vec<String> {
    let mut windows = Vec::new();
    let entries: Vec<_> = cx.global::<Registry>().windows.iter().map(|w| (w.view.clone(), w.native, w.scale)).collect();
    for (weak, native_window, scale) in entries {
        let Some(view) = weak.upgrade() else { continue };
        // The reader's reading position, which lives in its own entity and so moves with the tab.
        let readers: Vec<_> = view.read(cx).tabs.iter().map(|t| t.reader.clone()).collect();
        let reader_y: Vec<String> = readers
            .into_iter()
            .map(|r| {
                r.map_or("null,\"reader_top\":\"\"".to_string(), |r| {
                    let y = r.update(cx, |r, _| r.scroll_y());
                    let top = r.read(cx).top_line_text(16).replace(['"', '\\'], " ");
                    format!("{y:.1},\"reader_top\":\"{top}\"")
                })
            })
            .collect();
        let v = view.read(cx);
        let strip = v.tab_scroll.bounds();
        let scroll = f32::from(v.tab_scroll.offset().x);
        let rects: Vec<String> = (0..v.tabs.len())
            .filter_map(|t| v.tab_scroll.bounds_for_item(t))
            .map(|b| format!("[{},{}]", f32::from(b.origin.x) + scroll, f32::from(b.size.width)))
            .collect();
        let origin = native::client_origin(native_window).map_or("null".to_string(), |o| format!("[{},{}]", o.x, o.y));
        let tabs: Vec<String> = v
            .tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                format!(
                    "{{\"reader_y\":{},\"id\":{},\"path\":\"{}\",\"heading\":{},\"toc_offset\":{},\"opaque\":\"{:?}\"}}",
                    reader_y[i], t.id, t.path, t.state.current_heading, f32::from(t.state.toc_offset), Rc::as_ptr(&t.state.opaque) as *const u8,
                )
            })
            .collect();
        let native_id = match native_window {
            NativeWindow::X11(id) => id as i64,
            NativeWindow::Win32(h) => h as i64,
            NativeWindow::Other(_) => -1,
        };
        windows.push(format!(
            "{{\"win\":{},\"scale\":{},\"origin\":{},\"strip\":[{},{},{},{}],\"rects\":[{}],\"active\":{},\"scroll\":{},\"max_scroll\":{},\"marker\":{},\"find\":{},\"tabs\":[{}]}}",
            native_id,
            scale,
            origin,
            f32::from(strip.origin.x),
            f32::from(strip.origin.y),
            f32::from(strip.size.width),
            f32::from(strip.size.height),
            rects.join(","),
            v.active,
            f32::from(v.tab_scroll.offset().x),
            f32::from(v.tab_scroll.max_offset().x),
            v.drop_marker.map_or("null".to_string(), |m| m.x.to_string()),
            v.find_open,
            tabs.join(","),
        ));
    }
    windows
}
