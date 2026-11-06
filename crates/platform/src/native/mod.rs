//! What GPUI does not expose for cross-window drags.
//!
//! GPUI gives the source window its motion and release with out-of-bounds coordinates on X11, on
//! Windows (`SetCapture`) and on macOS. It does not say what those coordinates hit:
//!
//! - `App::window_stack` has no Windows implementation and, where it exists, lists only our own
//!   windows, so another application on top would look like a drop on the viewer beneath it.
//! - `Window::bounds` is per-window logical pixels, which do not compare across monitors of
//!   different scale on Windows.
//! - X11 display `visible_bounds` is the whole screen, ignoring panels.
//!
//! Compared with v0.2: the Windows hooks and the OLE drop target are not needed
//! here at all, because the web view's native drag session is what made them necessary.

use raw_window_handle::RawWindowHandle;

use crate::{DesktopPoint, DesktopRect};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod x11;

/// A top-level window's identity on the desktop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeWindow {
    X11(u32),
    Win32(isize),
    /// Wayland, macOS or anything else: no desktop-coordinate route (see [`Backend`]).
    Other(Backend),
}

/// How much of the desktop a platform lets us see.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Global pointer, stacking and work area from the X server. Run under Xvfb.
    X11,
    /// Global pointer and hit-testing from Win32. Compile-only.
    Windows,
    /// No global pointer or window position: an outside release must cancel.
    Wayland,
    /// No native adapter yet. Would use `App::window_stack`. Neither run nor compiled here.
    MacOs,
    Unknown,
}

impl NativeWindow {
    pub fn from_raw(raw: RawWindowHandle) -> Self {
        match raw {
            RawWindowHandle::Xcb(h) => NativeWindow::X11(h.window.get()),
            RawWindowHandle::Xlib(h) => NativeWindow::X11(h.window as u32),
            RawWindowHandle::Win32(h) => NativeWindow::Win32(h.hwnd.get()),
            RawWindowHandle::Wayland(_) => NativeWindow::Other(Backend::Wayland),
            RawWindowHandle::AppKit(_) => NativeWindow::Other(Backend::MacOs),
            _ => NativeWindow::Other(Backend::Unknown),
        }
    }

    pub fn backend(self) -> Backend {
        match self {
            NativeWindow::X11(_) => Backend::X11,
            NativeWindow::Win32(_) => Backend::Windows,
            NativeWindow::Other(b) => b,
        }
    }
}

/// What is on top at a desktop point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// One of our windows, by index into the list passed in.
    Ours(usize),
    /// Another application's window, or the bare desktop: both count as outside the viewer.
    Outside,
}

/// What is on top at `at`. `None` means this platform cannot say, or asking failed; callers must
/// treat that as "cancel", never as "outside".
///
/// `ignore` lists our own helper windows (the drag label). They are looked through: what is beneath
/// them is what the pointer is over.
pub fn hit_at(known: &[NativeWindow], ignore: &[NativeWindow], at: DesktopPoint) -> Option<Hit> {
    match known.first()?.backend() {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        Backend::X11 => x11::hit_at(known, ignore, at),
        #[cfg(target_os = "windows")]
        Backend::Windows => windows::hit_at(known, ignore, at),
        _ => None,
    }
}

/// The physical desktop position of a window's client area.
pub fn client_origin(window: NativeWindow) -> Option<DesktopPoint> {
    match window {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        NativeWindow::X11(id) => x11::client_origin(id),
        #[cfg(target_os = "windows")]
        NativeWindow::Win32(hwnd) => windows::client_origin(hwnd),
        _ => None,
    }
}

/// The work area of the monitor holding `at`.
pub fn work_area_at(backend: Backend, at: DesktopPoint) -> Option<DesktopRect> {
    match backend {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        Backend::X11 => x11::work_area_at(at),
        #[cfg(target_os = "windows")]
        Backend::Windows => windows::work_area_at(at),
        _ => {
            let _ = at;
            None
        }
    }
}

/// The scale factor of the monitor holding `at`, where it can differ between monitors. A new window
/// on another monitor needs this rather than the source window's scale.
pub fn scale_at(backend: Backend, at: DesktopPoint) -> Option<f32> {
    match backend {
        #[cfg(target_os = "windows")]
        Backend::Windows => windows::scale_at(at),
        _ => {
            let _ = at;
            None
        }
    }
}

/// Moves a new window to `rect` where window creation alone does not achieve that. On X11, GPUI
/// passes the position to `CreateWindow` but sets no position hint, so window managers place the
/// window themselves (xfwm4 was seen to ignore it). Elsewhere this does nothing.
pub fn place_window(window: NativeWindow, rect: DesktopRect) {
    match window {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        NativeWindow::X11(id) => x11::place_window(id, rect),
        _ => {
            let _ = rect;
        }
    }
}

/// Makes a just-created helper window (the drag label) unable to take activation, where window
/// creation alone does not guarantee it. X11 pop-ups are override-redirect, so need nothing.
pub fn prepare_ghost(window: NativeWindow) {
    match window {
        #[cfg(target_os = "windows")]
        NativeWindow::Win32(h) => windows::prepare_ghost(h),
        _ => {}
    }
}

/// Sizes, places and shows the hidden drag label without activating it, in physical pixels. It calls
/// back into the window procedure at once, so on Windows it must run with App not borrowed.
pub fn show_ghost(window: NativeWindow, rect: DesktopRect) {
    match window {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        NativeWindow::X11(id) => x11::show_window(id, rect),
        #[cfg(target_os = "windows")]
        NativeWindow::Win32(h) => windows::show_ghost(h, rect),
        _ => {
            let _ = rect;
        }
    }
}

/// Hides the drag label until the next drag.
pub fn hide_ghost(window: NativeWindow) {
    match window {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        NativeWindow::X11(id) => x11::hide_window(id),
        #[cfg(target_os = "windows")]
        NativeWindow::Win32(h) => windows::hide_ghost(h),
        _ => {}
    }
}

/// Moves the drag label without activating it or changing its size or stacking, in physical
/// pixels. On Windows it also keeps the window topmost.
pub fn place_ghost(window: NativeWindow, rect: DesktopRect) {
    match window {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        NativeWindow::X11(id) => x11::place_window(id, rect),
        #[cfg(target_os = "windows")]
        NativeWindow::Win32(h) => windows::place_ghost(h, rect),
        _ => {
            let _ = rect;
        }
    }
}
