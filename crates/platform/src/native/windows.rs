//! Win32: COMPILE-ONLY. This file has never run on a Windows machine.
//!
//! `WindowFromPoint` skips hidden windows. Coordinates are physical only if the process is DPI aware,
//! which the executable's manifest declares (per-monitor).

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetWindow, GetWindowLongPtrW, GetWindowRect, IsIconic, IsWindowVisible, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WindowFromPoint, GA_ROOT, GWL_EXSTYLE, GW_HWNDNEXT, HWND_TOPMOST,
    SWP_NOACTIVATE, SWP_NOSIZE, SW_HIDE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
};

use super::{Hit, NativeWindow};
use crate::{DesktopPoint, DesktopRect};

pub fn hit_at(known: &[NativeWindow], ignore: &[NativeWindow], at: DesktopPoint) -> Option<Hit> {
    let is_ignored = |h: HWND| ignore.iter().any(|w| matches!(w, NativeWindow::Win32(i) if *i == h.0 as isize));
    // SAFETY: all calls take plain values and tolerate a null or stale window.
    let mut top = unsafe { GetAncestor(WindowFromPoint(POINT { x: at.x, y: at.y }), GA_ROOT) };
    // Our drag label is on top of everything under the pointer. Walk down the stacking order to the
    // first visible window whose rectangle holds the point, as if the label were not there.
    while is_ignored(top) {
        let mut next = top;
        top = HWND::default();
        loop {
            // SAFETY: as above.
            next = unsafe { GetWindow(next, GW_HWNDNEXT) }.unwrap_or_default();
            if next.0.is_null() {
                break;
            }
            let mut r = windows::Win32::Foundation::RECT::default();
            // SAFETY: `r` is valid for the call.
            let (visible, iconic, ok) = unsafe {
                (IsWindowVisible(next).as_bool(), IsIconic(next).as_bool(), GetWindowRect(next, &mut r).is_ok())
            };
            if visible && !iconic && ok && at.x >= r.left && at.x < r.right && at.y >= r.top && at.y < r.bottom && !is_ignored(next) {
                top = next;
                break;
            }
        }
    }
    Some(
        known
            .iter()
            .position(|w| matches!(w, NativeWindow::Win32(h) if *h == top.0 as isize))
            .map_or(Hit::Outside, Hit::Ours),
    )
}

pub fn prepare_ghost(hwnd: isize) {
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: a stale handle makes both calls fail harmlessly.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
    }
}

pub fn place_ghost(hwnd: isize, r: DesktopRect) {
    // SAFETY: a stale handle makes the call fail, which is ignored.
    unsafe {
        let _ = SetWindowPos(HWND(hwnd as *mut _), Some(HWND_TOPMOST), r.x, r.y, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE);
    }
}

pub fn show_ghost(hwnd: isize, r: DesktopRect) {
    let hwnd = HWND(hwnd as *mut _);
    // SAFETY: a stale handle makes both calls fail, which is ignored.
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), r.x, r.y, r.width, r.height, SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

pub fn hide_ghost(hwnd: isize) {
    // SAFETY: a stale handle makes the call fail, which is ignored.
    unsafe {
        let _ = ShowWindow(HWND(hwnd as *mut _), SW_HIDE);
    }
}

pub fn client_origin(hwnd: isize) -> Option<DesktopPoint> {
    let mut point = POINT::default();
    // SAFETY: `point` is valid; a stale window handle makes the call fail, which is handled.
    unsafe { ClientToScreen(HWND(hwnd as *mut _), &mut point) }.ok().ok()?;
    Some(DesktopPoint { x: point.x, y: point.y })
}

pub fn work_area_at(at: DesktopPoint) -> Option<DesktopRect> {
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: `info` is a valid MONITORINFO with cbSize set, and the monitor handle is fresh.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: at.x, y: at.y }, MONITOR_DEFAULTTONEAREST);
        GetMonitorInfoW(monitor, &mut info).ok().ok()?;
    }
    let r = info.rcWork;
    Some(DesktopRect { x: r.left, y: r.top, width: r.right - r.left, height: r.bottom - r.top })
}

pub fn scale_at(at: DesktopPoint) -> Option<f32> {
    let (mut x, mut y) = (0u32, 0u32);
    // SAFETY: the out-pointers are valid and the monitor handle is fresh.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: at.x, y: at.y }, MONITOR_DEFAULTTONEAREST);
        GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y).ok()?;
    }
    Some(x as f32 / 96.0)
}
