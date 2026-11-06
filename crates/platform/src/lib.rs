//! Platform and tab-movement logic that does not depend on GPUI.
//!
//! All windows live in one process, so tab movement needs no coordinator and no RPC. The pieces:
//!
//! - [`drag`]: the press / drag / release / cancel state machine, including stale-press detection.
//! - [`order`]: where a tab lands in a strip, and which tab is active afterwards.
//! - [`placement`]: where a detached window opens, constrained to the display work area.
//! - [`single_instance`]: later launches hand their paths to the first process, and exit.
//! - [`native`]: what GPUI does not expose, namely the pointer position on the desktop, which
//!   window is really on top there, and the work area. Linux/X11 has been run. Windows code is
//!   compile-only until the owner runs the checklist. Wayland and macOS report no native route.

pub mod drag;
pub mod native;
pub mod order;
pub mod placement;
pub mod single_instance;

/// A point in physical desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopPoint {
    pub x: i32,
    pub y: i32,
}

/// A rectangle in physical desktop pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DesktopRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl DesktopRect {
    pub fn contains(&self, p: DesktopPoint) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.width && p.y < self.y + self.height
    }

    pub fn intersect(&self, o: &DesktopRect) -> Option<DesktopRect> {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = (self.x + self.width).min(o.x + o.width);
        let b = (self.y + self.height).min(o.y + o.height);
        (r > x && b > y).then_some(DesktopRect { x, y, width: r - x, height: b - y })
    }
}
