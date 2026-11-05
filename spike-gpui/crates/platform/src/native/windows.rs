//! Win32: COMPILE-ONLY. This file has never run. See the owner's checklist in the spike report.
//!
//! `WindowFromPoint` skips hidden windows. Coordinates are physical if the process is per-monitor
//! DPI aware, which GPUI's Windows platform requests; that is unconfirmed on a real machine.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};

use super::{Hit, NativeWindow};
use crate::{DesktopPoint, DesktopRect};

pub fn hit_at(known: &[NativeWindow], at: DesktopPoint) -> Option<Hit> {
    // SAFETY: both calls take plain values and tolerate a null or stale window.
    let top = unsafe { GetAncestor(WindowFromPoint(POINT { x: at.x, y: at.y }), GA_ROOT) };
    Some(
        known
            .iter()
            .position(|w| matches!(w, NativeWindow::Win32(h) if *h == top.0 as isize))
            .map_or(Hit::Outside, Hit::Ours),
    )
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
