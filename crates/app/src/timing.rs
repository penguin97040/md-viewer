//! Opt-in start-up timing log, with the `dev-harness` feature only.
//!
//! If `MDV_TIMING` names a file, each milestone is appended to it as one line with the wall-clock
//! time (`epoch_ms`, to line up with the shader lines written by the vendored renderer) and, on
//! Windows, the milliseconds since the process was created. Nothing happens when it is unset.

use std::io::Write;

pub fn log(event: &str) {
    let Some(path) = crate::harness::timing() else { return };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0);
    let since_start = process_age_ms().map(|a| format!("{a:.1}")).unwrap_or_else(|| "n/a".into());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "epoch_ms={now:.1} since_process_start_ms={since_start} event={event}");
    }
}

#[cfg(windows)]
fn process_age_ms() -> Option<f64> {
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(process: isize, creation: *mut FileTime, exit: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GetSystemTimeAsFileTime(out: *mut FileTime);
    }
    let (mut created, mut x, mut y, mut z, mut now) = (FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default(), FileTime::default());
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut created, &mut x, &mut y, &mut z) == 0 {
            return None;
        }
        GetSystemTimeAsFileTime(&mut now);
    }
    let t = |f: FileTime| ((f.high as u64) << 32 | f.low as u64) as f64;
    Some((t(now) - t(created)) / 10_000.0) // 100 ns ticks to ms
}

#[cfg(not(windows))]
fn process_age_ms() -> Option<f64> {
    None
}
