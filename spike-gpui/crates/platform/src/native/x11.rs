//! X11: a private connection, used only during a drag. Hit-testing rules come from MISTAKES 32
//! and 34: walk the root's children top-down, skip anything unmapped or override-redirect (drag
//! icons, the compositor's helpers), and identify our client inside the WM frame that was hit.

use std::cell::RefCell;

use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt as _, MapState, StackMode, Window};
use x11rb::rust_connection::RustConnection;

use super::{Hit, NativeWindow};
use crate::{DesktopPoint, DesktopRect};

thread_local! {
    static CONNECTION: RefCell<Option<(RustConnection, Window)>> = const { RefCell::new(None) };
}

fn with_connection<R>(f: impl FnOnce(&RustConnection, Window) -> Option<R>) -> Option<R> {
    CONNECTION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let (conn, screen) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots.get(screen)?.root;
            *slot = Some((conn, root));
        }
        let (conn, root) = slot.as_ref()?;
        let result = f(conn, *root);
        // A dead connection must not stick: a later drag should be able to reconnect.
        if result.is_none() {
            *slot = None;
        }
        result
    })
}

/// The ancestor of `window` that is a direct child of the root: the WM frame, or the window itself.
fn toplevel_of(conn: &RustConnection, root: Window, mut window: Window) -> Option<Window> {
    for _ in 0..16 {
        let parent = conn.query_tree(window).ok()?.reply().ok()?.parent;
        if parent == root || parent == x11rb::NONE {
            return Some(window);
        }
        window = parent;
    }
    None
}

pub fn hit_at(known: &[NativeWindow], ignore: &[NativeWindow], at: DesktopPoint) -> Option<Hit> {
    with_connection(|conn, root| {
        let tops: Vec<Option<Window>> = known
            .iter()
            .map(|w| match w {
                NativeWindow::X11(id) => toplevel_of(conn, root, *id),
                _ => None,
            })
            .collect();

        let ignored: Vec<Window> = ignore
            .iter()
            .filter_map(|w| match w {
                NativeWindow::X11(id) => toplevel_of(conn, root, *id),
                _ => None,
            })
            .collect();
        let children = conn.query_tree(root).ok()?.reply().ok()?.children;
        for &child in children.iter().rev() {
            if ignored.contains(&child) {
                continue;
            }
            // A window can vanish between the calls: skip it, it is not under the pointer.
            let Some(attrs) = conn.get_window_attributes(child).ok().and_then(|c| c.reply().ok()) else {
                continue;
            };
            if attrs.map_state != MapState::VIEWABLE || attrs.override_redirect {
                continue;
            }
            let Some(geom) = conn.get_geometry(child).ok().and_then(|c| c.reply().ok()) else {
                continue;
            };
            let rect = DesktopRect {
                x: geom.x as i32,
                y: geom.y as i32,
                width: geom.width as i32 + 2 * geom.border_width as i32,
                height: geom.height as i32 + 2 * geom.border_width as i32,
            };
            if rect.contains(at) {
                return Some(tops.iter().position(|t| *t == Some(child)).map_or(Hit::Outside, Hit::Ours));
            }
        }
        Some(Hit::Outside)
    })
}

pub fn client_origin(window: Window) -> Option<DesktopPoint> {
    with_connection(|conn, root| {
        let t = conn.translate_coordinates(window, root, 0, 0).ok()?.reply().ok()?;
        Some(DesktopPoint { x: t.dst_x as i32, y: t.dst_y as i32 })
    })
}

fn monitor_at(conn: &RustConnection, root: Window, at: DesktopPoint) -> Option<DesktopRect> {
    let monitors = conn.randr_get_monitors(root, true).ok()?.reply().ok()?.monitors;
    monitors
        .iter()
        .map(|m| DesktopRect { x: m.x as i32, y: m.y as i32, width: m.width as i32, height: m.height as i32 })
        .find(|r| r.contains(at))
}

/// `_NET_WORKAREA` for the current desktop, limited to the monitor under `at`. The hint spans the
/// whole X screen, so on several monitors it says nothing about any one of them on its own.
pub fn work_area_at(at: DesktopPoint) -> Option<DesktopRect> {
    with_connection(|conn, root| {
        let atom = |name: &[u8]| conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom);
        let desktop = conn
            .get_property(false, root, atom(b"_NET_CURRENT_DESKTOP")?, AtomEnum::CARDINAL, 0, 1)
            .ok()?
            .reply()
            .ok()?
            .value32()
            .and_then(|mut v| v.next())
            .unwrap_or(0);
        let hint = conn
            .get_property(false, root, atom(b"_NET_WORKAREA")?, AtomEnum::CARDINAL, 0, u32::MAX)
            .ok()?
            .reply()
            .ok()
            .and_then(|r| r.value32().map(|v| v.collect::<Vec<u32>>()))
            .and_then(|v| {
                v.chunks_exact(4).nth(desktop as usize).map(|c| DesktopRect {
                    x: c[0] as i32,
                    y: c[1] as i32,
                    width: c[2] as i32,
                    height: c[3] as i32,
                })
            });
        let monitor = monitor_at(conn, root, at);
        match (hint, monitor) {
            (Some(h), Some(m)) => h.intersect(&m).or(Some(m)),
            (Some(h), None) => Some(h),
            (None, m) => m,
        }
    })
}

pub fn show_window(window: Window, rect: DesktopRect) {
    with_connection(|conn, _| {
        let aux = ConfigureWindowAux::new()
            .x(rect.x)
            .y(rect.y)
            .width(rect.width.max(1) as u32)
            .height(rect.height.max(1) as u32)
            .stack_mode(StackMode::ABOVE);
        conn.configure_window(window, &aux).ok()?;
        conn.map_window(window).ok()?;
        conn.flush().ok()
    });
}

pub fn hide_window(window: Window) {
    with_connection(|conn, _| {
        conn.unmap_window(window).ok()?;
        conn.flush().ok()
    });
}

pub fn place_window(window: Window, rect: DesktopRect) {
    with_connection(|conn, _| {
        let aux = ConfigureWindowAux::new().x(rect.x).y(rect.y);
        conn.configure_window(window, &aux).ok()?;
        conn.flush().ok()
    });
}
