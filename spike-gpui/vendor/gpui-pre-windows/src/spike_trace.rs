//! Spike instrumentation: what the UI thread does between pressing a window's header and the window
//! following the pointer. Only when `MDV_TIMING` names a file; otherwise each call is one load.
//!
//! The UI thread only takes a time and sends a small value. A writer thread formats, buffers and
//! flushes, and works out one `drag_summary` line per press. It also samples the pointer for a
//! moment after a press: the OS reports nothing during a stall, and the gap between the pointer
//! moving and the window moving is the thing being looked for.

use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::sync::LazyLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::{Foundation::*, UI::WindowsAndMessaging::*};

/// Shorter frames and messages are left out of the log unless they are close to a press.
const SLOW_MS: f32 = 8.0;
const CLOSE_TO_PRESS: Duration = Duration::from_millis(800);
const SAMPLE_POINTER_FOR: Duration = Duration::from_millis(1500);

struct Event {
    at: Instant,
    what: &'static str,
    window: isize,
    arg: i64,
    ms: f32,
}

static TRACE: LazyLock<Option<Sender<Event>>> = LazyLock::new(|| {
    let path = std::env::var_os("MDV_TIMING")?;
    let file = OpenOptions::new().create(true).append(true).open(path).ok()?;
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("mdv-trace".to_owned())
        .spawn(move || write_loop(rx, BufWriter::new(file)))
        .ok()?;
    Some(tx)
});

#[inline]
pub(crate) fn start() -> Option<Instant> {
    TRACE.as_ref().map(|_| Instant::now())
}

#[inline]
pub(crate) fn mark(what: &'static str, window: HWND, arg: i64) {
    if let Some(tx) = TRACE.as_ref() {
        let _ = tx.send(Event { at: Instant::now(), what, window: window.0 as isize, arg, ms: 0.0 });
    }
}

#[inline]
pub(crate) fn span(what: &'static str, window: HWND, arg: i64, started: Option<Instant>) {
    if let (Some(tx), Some(started)) = (TRACE.as_ref(), started) {
        let at = Instant::now();
        let ms = (at - started).as_secs_f32() * 1000.0;
        let _ = tx.send(Event { at, what, window: window.0 as isize, arg, ms });
    }
}

/// After a window's paint. Most are idle wake-ups that draw nothing, and are not worth a line.
#[inline]
pub(crate) fn paint_done(window: HWND, started: Option<Instant>) {
    if started.is_some_and(|s| s.elapsed() >= Duration::from_micros(300)) {
        span("paint", window, 0, started);
    }
}

fn name_of(msg: u32) -> Option<(&'static str, &'static str)> {
    Some(match msg {
        WM_MOUSEACTIVATE => ("wm_mouseactivate", "wm_mouseactivate_done"),
        WM_ACTIVATE => ("wm_activate", "wm_activate_done"),
        WM_NCACTIVATE => ("wm_ncactivate", "wm_ncactivate_done"),
        WM_SETFOCUS => ("wm_setfocus", "wm_setfocus_done"),
        WM_KILLFOCUS => ("wm_killfocus", "wm_killfocus_done"),
        WM_NCLBUTTONDOWN => ("wm_nclbuttondown", "wm_nclbuttondown_done"),
        WM_NCLBUTTONDBLCLK => ("wm_nclbuttondblclk", "wm_nclbuttondblclk_done"),
        WM_NCLBUTTONUP => ("wm_nclbuttonup", "wm_nclbuttonup_done"),
        WM_SYSCOMMAND => ("wm_syscommand", "wm_syscommand_done"),
        WM_ENTERSIZEMOVE => ("wm_entersizemove", "wm_entersizemove_done"),
        WM_EXITSIZEMOVE => ("wm_exitsizemove", "wm_exitsizemove_done"),
        WM_CAPTURECHANGED => ("wm_capturechanged", "wm_capturechanged_done"),
        WM_MOVING => ("wm_moving", "wm_moving_done"),
        WM_MOVE => ("wm_move", "wm_move_done"),
        WM_GETOBJECT => ("wm_getobject", "wm_getobject_done"),
        _ => return None,
    })
}

/// At the top of the window procedure.
#[inline]
pub(crate) fn message_begins(window: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<Instant> {
    let started = start()?;
    if let Some((begin, _)) = name_of(msg) {
        // WM_GETOBJECT's interesting value is in lparam, the rest in wparam.
        let arg = if msg == WM_GETOBJECT { lparam.0 as i32 as i64 } else { wparam.0 as i64 };
        mark(begin, window, arg);
    }
    Some(started)
}

/// At the bottom of the window procedure.
#[inline]
pub(crate) fn message_ends(window: HWND, msg: u32, started: Option<Instant>) {
    if started.is_none() {
        return;
    }
    match name_of(msg) {
        Some((_, done)) => span(done, window, 0, started),
        None => span("message", window, msg as i64, started),
    }
}

#[derive(Default)]
struct Press {
    started: Option<Instant>,
    window: isize,
    activated: bool,
    dispatch_ms: f32,
    enter: Option<Instant>,
    first_moving: Option<Instant>,
    first_move: Option<Instant>,
    pointer_moved: Option<Instant>,
    pointer_at: Option<POINT>,
    paints: u32,
    paint_sum: f32,
    paint_max: f32,
    present_max: f32,
    getobject: u32,
    getobject_sum: f32,
    slowest: f32,
}

fn since(base: Instant, at: Option<Instant>) -> f32 {
    at.map_or(-1.0, |at| at.saturating_duration_since(base).as_secs_f32() * 1000.0)
}

fn write_loop(rx: Receiver<Event>, mut out: BufWriter<std::fs::File>) {
    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    let base = Instant::now();
    let stamp = |at: Instant| epoch + at.saturating_duration_since(base).as_secs_f64() * 1000.0;
    let mut press = Press::default();
    let mut activate_seen: Option<Instant> = None;
    let mut last_press: Option<Instant> = None;
    let mut sample_until: Option<Instant> = None;
    loop {
        let event = rx.recv_timeout(if sample_until.is_some() { Duration::from_millis(2) } else { Duration::from_millis(100) });
        if let Some(until) = sample_until {
            let now = Instant::now();
            let mut at = POINT::default();
            if unsafe { GetCursorPos(&mut at) }.is_ok() {
                match press.pointer_at {
                    None => press.pointer_at = Some(at),
                    Some(from) if from.x != at.x || from.y != at.y => {
                        press.pointer_moved = Some(now);
                        sample_until = None;
                    }
                    Some(_) => {}
                }
            }
            if now >= until {
                sample_until = None;
            }
        }
        let event = match event {
            Ok(event) => event,
            Err(RecvTimeoutError::Timeout) => {
                let _ = out.flush();
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        let near_press = last_press.is_some_and(|p| event.at.saturating_duration_since(p) < CLOSE_TO_PRESS);
        match event.what {
            "wm_mouseactivate" => activate_seen = Some(event.at),
            "press_before_dispatch" => {
                // WM_MOUSEACTIVATE comes first, and only to an inactive window.
                let activation = activate_seen.take().filter(|a| event.at - *a < Duration::from_millis(250));
                press = Press {
                    started: Some(activation.unwrap_or(event.at)),
                    activated: activation.is_some(),
                    window: event.window,
                    ..Press::default()
                };
                last_press = Some(event.at);
                sample_until = Some(event.at + SAMPLE_POINTER_FOR);
            }
            "press_after_dispatch" => press.dispatch_ms = event.ms,
            "wm_entersizemove" => press.enter = press.enter.or(Some(event.at)),
            "wm_moving" => press.first_moving = press.first_moving.or(Some(event.at)),
            "wm_move" => press.first_move = press.first_move.or(Some(event.at)),
            "paint" if press.started.is_some() => {
                press.paints += 1;
                press.paint_sum += event.ms;
                press.paint_max = press.paint_max.max(event.ms);
            }
            "present" => press.present_max = press.present_max.max(event.ms),
            "wm_getobject_done" => {
                press.getobject += 1;
                press.getobject_sum += event.ms;
            }
            // This returns when the move loop has ended, or at once for a press that never began one.
            "wm_nclbuttondown_done" | "wm_nclbuttondblclk_done" => {
                if let Some(started) = press.started.take() {
                    let _ = writeln!(
                        out,
                        "epoch_ms={:.1} ev=drag_summary w={} activated={} dispatch_ms={:.1} enter_ms={:.1} first_moving_ms={:.1} first_move_ms={:.1} pointer_moved_ms={:.1} stall_ms={:.1} paints={} paint_sum_ms={:.1} paint_max_ms={:.1} present_max_ms={:.1} getobject={} getobject_sum_ms={:.1} slowest_message_ms={:.1}",
                        stamp(event.at),
                        press.window & 0xffff,
                        press.activated,
                        press.dispatch_ms,
                        since(started, press.enter),
                        since(started, press.first_moving),
                        since(started, press.first_move),
                        since(started, press.pointer_moved),
                        match (press.pointer_moved, press.first_moving) {
                            (Some(p), Some(m)) => m.saturating_duration_since(p).as_secs_f32() * 1000.0,
                            _ => -1.0,
                        },
                        press.paints,
                        press.paint_sum,
                        press.paint_max,
                        press.present_max,
                        press.getobject,
                        press.getobject_sum,
                        press.slowest,
                    );
                }
            }
            _ => {}
        }
        // Done spans cover the whole move loop when they wrap `DefWindowProc`; those are not "slow".
        if event.what == "message" || event.what.ends_with("_done") && !matches!(event.what, "wm_nclbuttondown_done" | "wm_syscommand_done") {
            press.slowest = press.slowest.max(event.ms);
        }
        let frequent = matches!(event.what, "paint" | "present" | "message" | "wm_moving" | "wm_move" | "wm_moving_done" | "wm_move_done");
        if frequent && !near_press && event.ms < SLOW_MS {
            continue;
        }
        if event.what == "message" && event.ms < SLOW_MS {
            continue;
        }
        let _ = writeln!(
            out,
            "epoch_ms={:.1} ev={} w={} ms={:.2} arg={:#x}",
            stamp(event.at),
            event.what,
            event.window & 0xffff,
            event.ms,
            event.arg
        );
    }
    let _ = out.flush();
}
