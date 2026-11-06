//! A scripted run for measurement and acceptance checks, driven by `MDV_SCRIPT=<file>`. It lets a
//! test step exact positions and read back results. Results go to `MDV_RESULTS=<file>`.

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::{Duration, Instant};

use gpui::{AsyncApp, Entity, WeakEntity};

use super::pane::ReaderView;
use crate::model::DocPos;

fn rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmRSS")).and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok())))
        .unwrap_or(0)
}

fn pct(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[((v.len() as f32 - 1.0) * p).round() as usize]
}

fn log(out: &mut std::fs::File, s: &str) {
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}

/// Waits for a frame: the reader paints only when something changed, so wait for a paint.
async fn next_frame(this: &WeakEntity<ReaderView>, cx: &mut AsyncApp, limit_ms: u64) {
    let start = this.update(cx, |v, _| v.perf.work_ms.len()).unwrap_or(0);
    let t = Instant::now();
    loop {
        cx.background_executor().timer(Duration::from_millis(2)).await;
        let n = this.update(cx, |v, _| v.perf.work_ms.len()).unwrap_or(0);
        if n > start || t.elapsed().as_millis() as u64 > limit_ms {
            return;
        }
    }
}

/// Stands in for the content crate: every asset arrives with a size unlike its placeholder.
struct FakeAssets;
impl crate::layout::Assets for FakeAssets {
    fn math(&self, src: &str, display: bool, font_size: f32) -> Option<crate::layout::AssetBox> {
        let h = if display { font_size * 3.0 } else { font_size * 1.4 };
        Some(crate::layout::AssetBox { width: src.len() as f32 * font_size * 0.6, height: h, baseline: h * 0.75, key: 1 })
    }
    fn mermaid(&self, _src: &str) -> Option<crate::layout::AssetBox> {
        Some(crate::layout::AssetBox { width: 600.0, height: 320.0, baseline: 320.0, key: 2 })
    }
}

pub fn start(view: Entity<ReaderView>, cx: &mut gpui::App) {
    let Ok(path) = std::env::var("MDV_SCRIPT") else { return };
    let results = std::env::var("MDV_RESULTS").unwrap_or_else(|_| "/dev/stderr".into());
    let script = std::fs::read_to_string(path).unwrap_or_default();
    let shot = std::env::var("MDV_SHOT_CMD").ok();
    let weak = view.downgrade();
    cx.spawn(async move |cx| {
        let mut out = std::fs::OpenOptions::new().create(true).append(true).open(results).expect("results file");
        let this = weak;
        for line in script.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
            let args: Vec<&str> = rest.split_whitespace().collect();
            let num = |i: usize| args.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(0.0);
            match cmd {
                "log" => log(&mut out, rest),
                "wait" => cx.background_executor().timer(Duration::from_millis(num(0) as u64)).await,
                "wait_doc" => loop {
                    if this.update(cx, |v, _| !v.visible.is_empty()).unwrap_or(true) {
                        break;
                    }
                    cx.background_executor().timer(Duration::from_millis(2)).await;
                },
                "open_stats" => {
                    let s = this.update(cx, |v, _| {
                        let d = v.doc.as_ref();
                        format!(
                            "chunks={} headings={} parse_ms={:.1} first_content_ms={:.1} rss_kb={} mounted={} parsed_cached={}",
                            d.map_or(0, |d| d.chunks.len()),
                            d.map_or(0, |d| d.headings.len()),
                            d.map_or(0.0, |d| d.parse_time.as_secs_f64() * 1000.0),
                            v.perf.first_content.map_or(0.0, |t| t.as_secs_f64() * 1000.0),
                            rss_kb(),
                            v.mounted_chunks(),
                            d.map_or(0, |d| d.cached_count())
                        )
                    });
                    log(&mut out, &s.unwrap_or_default());
                }
                "reset_perf" => {
                    let _ = this.update(cx, |v, _| {
                        v.perf.work_ms.clear();
                        v.perf.interval_ms.clear();
                        v.perf.layout_ms.clear();
                        v.perf.mounts = 0;
                        v.perf.reset_interval();
                    });
                }
                // scroll DY COUNT DELAY_MS
                "scroll" => {
                    for _ in 0..num(1) as u32 {
                        let _ = this.update(cx, |v, cx| {
                            v.scroll_by(num(0));
                            cx.notify();
                        });
                        next_frame(&this, cx, 1000).await;
                        if num(2) > 0.0 {
                            cx.background_executor().timer(Duration::from_millis(num(2) as u64)).await;
                        }
                    }
                }
                "stats" => {
                    let s = this.update(cx, |v, _| {
                        let (mut w, mut i, mut l) = (v.perf.work_ms.clone(), v.perf.interval_ms.clone(), v.perf.layout_ms.clone());
                        let mut r = String::new();
                        let d = v.doc.as_ref();
                        let _ = write!(
                            r,
                            "{rest}: frames={} work_ms p50={:.2} p95={:.2} max={:.2}; interval_ms p50={:.2} p95={:.2} max={:.2}; layouts={} sync={} layout_ms p50={:.2} p95={:.2} max={:.2}; mounted={} parsed_cached={} parses={} rss_kb={}",
                            w.len(), pct(&mut w, 0.5), pct(&mut w, 0.95), pct(&mut w, 1.0), pct(&mut i, 0.5), pct(&mut i, 0.95), pct(&mut i, 1.0),
                            l.len(), v.perf.mounts, pct(&mut l, 0.5), pct(&mut l, 0.95), pct(&mut l, 1.0),
                            v.mounted_chunks(), d.map_or(0, |d| d.cached_count()), d.map_or(0, |d| d.parse_count()), rss_kb()
                        );
                        r
                    });
                    log(&mut out, &s.unwrap_or_default());
                }
                "heading" => {
                    let t = Instant::now();
                    let _ = this.update(cx, |v, cx| v.jump_to_heading(num(0) as usize, cx));
                    next_frame(&this, cx, 2000).await;
                    let elapsed = t.elapsed().as_secs_f64() * 1000.0;
                    let s = this.update(cx, |v, _| {
                        let want = v.doc.as_ref().and_then(|d| d.headings.get(num(0) as usize)).map_or(String::new(), |h| h.text.clone());
                        let cur = v.current_heading();
                        format!("jump heading {}: {elapsed:.1} ms to painted frame; want {want:?}; top {:?}; current_heading={cur:?}; mounted={}", args[0], v.top_line_text(30), v.mounted_chunks())
                    });
                    log(&mut out, &s.unwrap_or_default());
                }
                "fragment" => {
                    let _ = this.update(cx, |v, cx| v.jump_to_fragment(args[0], cx));
                    next_frame(&this, cx, 2000).await;
                }
                "scroll_y" => {
                    // The scrollbar's mapping: a fraction of the whole height.
                    let t = Instant::now();
                    let _ = this.update(cx, |v, cx| {
                        let total = v.total_height();
                        v.set_scroll_y(total * num(0) as f64);
                        cx.notify();
                    });
                    next_frame(&this, cx, 3000).await;
                    let e = t.elapsed().as_secs_f64() * 1000.0;
                    let s = this.update(cx, |v, _| format!("scroll fraction {}: {e:.1} ms; position {:?}", args[0], v.reading_position()));
                    log(&mut out, &s.unwrap_or_default());
                }
                "position" => {
                    let s = this.update(cx, |v, _| {
                        let (c, f, b) = v.reading_position();
                        format!("{rest}: chunk={c} frac={f:.4} byte={b} top={:?}", v.top_line_text(40))
                    });
                    log(&mut out, &s.unwrap_or_default());
                }
                // set font_size|padding|line_height|serif|wrap|theme VALUE
                "set" => {
                    let _ = this.update(cx, |v, cx| {
                        let mut s = v.settings_override.unwrap_or(v.settings);
                        match args[0] {
                            "font_size" => s.font_size = num(1),
                            "padding" => s.padding = num(1),
                            "line_height" => s.line_height = num(1),
                            "serif" => s.serif = args[1] == "1",
                            "wrap" => s.wrap = args[1] == "1",
                            "theme" => s.mode = if args[1] == "light" { mdv_theme::Mode::Light } else { mdv_theme::Mode::Dark },
                            _ => {}
                        }
                        v.settings_override = Some(s);
                        cx.notify();
                    });
                    next_frame(&this, cx, 2000).await;
                }
                "find" => {
                    let t = Instant::now();
                    let _ = this.update(cx, |v, cx| v.find_query(rest, false, cx));
                    loop {
                        cx.background_executor().timer(Duration::from_millis(1)).await;
                        if !this.update(cx, |v, _| v.find.searching).unwrap_or(false) {
                            break;
                        }
                    }
                    let took = t.elapsed().as_secs_f64() * 1000.0;
                    next_frame(&this, cx, 2000).await;
                    let s = this.update(cx, |v, _| format!("find {rest:?}: search+return {took:.1} ms; total={} chunks_with_matches={} text={:?} top={:?} rss_kb={}", v.find.total, v.find.counts.iter().filter(|c| **c > 0).count(), v.find_count_text(), v.top_line_text(40), rss_kb()));
                    log(&mut out, &s.unwrap_or_default());
                }
                "find_step" => {
                    let t = Instant::now();
                    let _ = this.update(cx, |v, cx| v.find_step(num(0) as i64, cx));
                    next_frame(&this, cx, 2000).await;
                    let e = t.elapsed().as_secs_f64() * 1000.0;
                    let s = this.update(cx, |v, _| format!("find step {}: {e:.1} ms; {:?}; position {:?}", args[0], v.find_count_text(), v.reading_position()));
                    log(&mut out, &s.unwrap_or_default());
                }
                "find_goto" => {
                    let _ = this.update(cx, |v, cx| {
                        v.find.pos = Some((num(0) as u64).saturating_sub(1));
                        v.find_step(1, cx);
                    });
                    next_frame(&this, cx, 2000).await;
                    let s = this.update(cx, |v, _| format!("find goto {}: {:?} position {:?} mounted={}", args[0], v.find_count_text(), v.reading_position(), v.mounted_chunks()));
                    log(&mut out, &s.unwrap_or_default());
                }
                // select CHUNK OFFSET CHUNK OFFSET
                "select" => {
                    let _ = this.update(cx, |v, cx| {
                        v.set_selection(DocPos::new(num(0) as u32, num(1) as u32), DocPos::new(num(2) as u32, num(3) as u32));
                        cx.notify();
                    });
                }
                "copy_info" => {
                    let s = this.update(cx, |v, _| {
                        let t = Instant::now();
                        let x = v.selected_text().unwrap_or_default();
                        let e = t.elapsed().as_secs_f64() * 1000.0;
                        let start: String = x.chars().take(30).collect();
                        let end: String = x.chars().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
                        format!("{rest}: selection text {} bytes in {e:.1} ms; start {start:?} end {end:?}; mounted={}", x.len(), v.mounted_chunks())
                    });
                    log(&mut out, &s.unwrap_or_default());
                }
                // What the system clipboard holds, read back through GPUI.
                "clip_info" => {
                    let t = this.update(cx, |_, cx| cx.read_from_clipboard().and_then(|c| c.text())).ok().flatten().unwrap_or_default();
                    let start: String = t.chars().take(40).collect();
                    let end: String = t.chars().rev().take(30).collect::<Vec<_>>().into_iter().rev().collect();
                    let sections = t.lines().filter(|l| l.starts_with("Section ")).count();
                    log(&mut out, &format!("{rest}: clipboard {} bytes, {sections} Section lines; start {start:?} end {end:?}", t.len()));
                }
                "fake_assets" => {
                    let _ = this.update(cx, |v, cx| {
                        v.set_assets(Some(std::sync::Arc::new(FakeAssets)));
                        cx.notify();
                    });
                    next_frame(&this, cx, 2000).await;
                }
                // Clicks the first visible link whose target contains the text, with real input.
                "click_link" => {
                    let at = this.update(cx, |v, _| v.link_point(rest)).ok().flatten();
                    match (at, &shot) {
                        (Some((x, y)), Some(c)) => {
                            let _ = std::process::Command::new("bash").arg(c).arg(format!("click:{x}:{y}")).status();
                        }
                        _ => {
                            let seen = this.update(cx, |v, _| v.visible.iter().map(|x| format!("y={} {:?}", x.y, x.layout.hits.iter().filter_map(|h| if let crate::layout::HitKind::Link(i) = h.kind { Some((i, h.rect.y, h.rect.w)) } else { None }).collect::<Vec<_>>())).collect::<Vec<_>>()).unwrap_or_default();
                            log(&mut out, &format!("no visible link to {rest}: {seen:?}"));
                        }
                    }
                    cx.background_executor().timer(Duration::from_millis(300)).await;
                }
                "shot" => {
                    if let Some(c) = &shot {
                        cx.background_executor().timer(Duration::from_millis(250)).await;
                        let _ = std::process::Command::new("bash").arg(c).arg(args[0]).status();
                    }
                }
                "quit" => {
                    let _ = cx.update(|cx| cx.quit());
                    return;
                }
                other => log(&mut out, &format!("unknown script command {other}")),
            }
        }
    })
    .detach();
}
