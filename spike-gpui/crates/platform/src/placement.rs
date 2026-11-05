//! Placement of a detached window. Same rule as `native_linux.go` `compute_placement`: default
//! 1100x820, clamped to the work area, with the drop point 80 px in and 20 px down so the pointer
//! lands on the new window's header.

use crate::{DesktopPoint, DesktopRect};

pub const DEFAULT_WIDTH: i32 = 1100;
pub const DEFAULT_HEIGHT: i32 = 820;
const POINTER_INSET_X: i32 = 80;
const POINTER_INSET_Y: i32 = 20;

/// Size and position for a window opened by a drop at `drop`, entirely inside `work_area`.
pub fn place_detached(drop: DesktopPoint, work_area: DesktopRect) -> DesktopRect {
    let width = DEFAULT_WIDTH.min(work_area.width);
    let height = DEFAULT_HEIGHT.min(work_area.height);
    let x = (drop.x - POINTER_INSET_X).min(work_area.x + work_area.width - width).max(work_area.x);
    let y = (drop.y - POINTER_INSET_Y).min(work_area.y + work_area.height - height).max(work_area.y);
    DesktopRect { x, y, width, height }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: DesktopRect = DesktopRect { x: 0, y: 30, width: 1920, height: 1050 };

    fn inside(r: DesktopRect, a: DesktopRect) -> bool {
        r.x >= a.x && r.y >= a.y && r.x + r.width <= a.x + a.width && r.y + r.height <= a.y + a.height
    }

    #[test]
    fn opens_near_the_drop_point() {
        let r = place_detached(DesktopPoint { x: 500, y: 200 }, AREA);
        assert_eq!((r.x, r.y, r.width, r.height), (420, 180, 1100, 820));
    }

    #[test]
    fn stays_inside_at_every_corner() {
        for (x, y) in [(-500, -500), (5000, -500), (-500, 5000), (5000, 5000), (1919, 1079), (0, 30)] {
            let r = place_detached(DesktopPoint { x, y }, AREA);
            assert!(inside(r, AREA), "{x},{y} gave {r:?}");
        }
    }

    #[test]
    fn shrinks_to_a_small_work_area() {
        let small = DesktopRect { x: 100, y: 100, width: 800, height: 600 };
        let r = place_detached(DesktopPoint { x: 0, y: 0 }, small);
        assert_eq!(r, small);
    }

    #[test]
    fn respects_a_monitor_with_a_negative_origin() {
        let left = DesktopRect { x: -1920, y: 0, width: 1920, height: 1080 };
        let r = place_detached(DesktopPoint { x: -100, y: 1000 }, left);
        assert!(inside(r, left), "{r:?}");
    }
}
