//! The reader pane: a custom element holding the virtual chunk list, painting and all pointer
//! handling. GPUI's `list` is not used: its items have no per-item height hints, and its scrollbar
//! values only count measured items, so a 1,590-chunk document could not show a stable scrollbar
//! without measuring every chunk (`elements/list.rs`, `ListItem::size_hint`, `max_offset_for_scrollbar`).
//!
//! Position is an anchor (chunk, pixels into it), so a chunk above the view changing height cannot
//! move what is on screen. Only the scrollbar thumb, which uses estimates, moves.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, fill, point, prelude::*, px, quad, rgb, size, App, Bounds, BorderStyle, ClipboardItem, ContentMask, Context, Corners, CursorStyle, DispatchPhase, Edges, Element, ElementId, Entity, EventEmitter, FontId, GlobalElementId,
    GlyphId, Hitbox, HitboxBehavior, Hsla, InspectorElementId, IntoElement, KeyDownEvent, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Path, Pixels, Render, RenderImage, ScrollEvent, Style, Window,
};
use mdv_theme::Mode;

use super::content::{ContentAssets, ContentStore};
use super::images::ImageStore;
use super::shaper::GpuiShaper;
use crate::doc::Document;
use crate::fold;
use crate::layout::{self, Area, Assets, ChunkLayout, ChunkState, Hit, HitKind, ImageState, Layer, Options, TextBox, FLAG_ATOM, FLAG_EMOJI, FLAG_FAKE_BOLD, FLAG_SPACE};
use crate::model::{Block, ByteSpan, DocPos, ImageSource, LinkTarget};
use crate::paths;
use crate::sort;

/// Live reader settings: what the settings popover drives.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub padding: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub serif: bool,
    pub wrap: bool,
    pub mode: Mode,
}

pub enum ReaderEvent {
    /// A local markdown link was followed.
    OpenLocal { path: PathBuf, fragment: Option<String> },
    Toast(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Granularity {
    Char,
    Word,
    Para,
}

#[derive(Clone, Copy, Debug)]
pub struct Selection {
    pub anchor: DocPos,
    pub head: DocPos,
    /// With word or paragraph granularity: the end of what the first press selected.
    anchor_end: DocPos,
}

impl Selection {
    pub fn ordered(&self) -> (DocPos, DocPos) {
        let lo = self.anchor.min(self.head).min(self.anchor_end);
        let hi = self.anchor.max(self.head).max(self.anchor_end);
        (lo, hi)
    }
}

struct Drag {
    gran: Granularity,
    start: gpui::Point<Pixels>,
    moved: bool,
    hit: Option<(usize, HitKind)>,
    autoscrolling: bool,
}

#[derive(Default)]
pub struct FindState {
    /// The folded query.
    pub q: String,
    pub counts: Vec<u32>,
    pub total: u64,
    /// The current match, counted across the whole document.
    pub pos: Option<u64>,
    gen: u64,
    current: Option<(usize, ByteSpan)>,
    spans: HashMap<usize, Arc<Vec<ByteSpan>>>,
    pub search_time: Option<Duration>,
    pub searching: bool,
}

/// Frame timing, kept only while a performance script is running.
#[derive(Default)]
pub struct Perf {
    pub enabled: bool,
    pub work_ms: Vec<f32>,
    pub interval_ms: Vec<f32>,
    pub mounts: u32,
    pub layout_ms: Vec<f32>,
    last_paint: Option<Instant>,
    pub first_content: Option<Duration>,
    pub opened_at: Option<Instant>,
}

impl Perf {
    pub fn reset_interval(&mut self) {
        self.last_paint = None;
    }
}

#[derive(Clone)]
pub struct Visible {
    pub chunk: usize,
    pub layout: Arc<ChunkLayout>,
    pub y: f32,
}

pub struct ReaderView {
    pub doc: Option<Arc<Document>>,
    pub settings: Settings,
    pub settings_override: Option<Settings>,
    cur: Option<Options>,
    layouts: HashMap<usize, Arc<ChunkLayout>>,
    raw: Vec<f32>,
    measured: Vec<Option<f32>>,
    sum_est: f32,
    sum_real: f32,
    prefix: Vec<f64>,
    prefix_dirty: bool,
    pub anchor: (usize, f32),
    text_anchor: Option<(usize, u32, f32)>,
    pending_jump: Option<(usize, u32, f32)>,
    pending_reveal: Option<(usize, u32)>,
    states: HashMap<usize, ChunkState>,
    area_scroll: HashMap<(usize, u32), f32>,
    pub selection: Option<Selection>,
    drag: Option<Drag>,
    thumb_drag: Option<(f32, f64)>,
    area_thumb_drag: Option<(usize, u32, f32, f32)>,
    mouse: gpui::Point<Pixels>,
    hover: Option<(usize, u32)>,
    hover_thumb: bool,
    pub find: FindState,
    pub visible: Vec<Visible>,
    bounds: Bounds<Pixels>,
    images: Arc<ImageStore>,
    images_seen: u64,
    content: ContentStore,
    content_scale: f32,
    /// Chunk -> keys of maths and diagrams its layout was waiting for.
    waiting: HashMap<usize, Vec<u64>>,
    watching: bool,
    prefetching: Option<usize>,
    prefetch_queue_gen: u64,
    last_heading: Option<usize>,
    pub perf: Perf,
    pub assets: Option<Arc<dyn Assets + Send + Sync>>,
}

impl EventEmitter<ReaderEvent> for ReaderView {}

struct ViewAssets<'a> {
    images: &'a ImageStore,
    extra: Option<&'a (dyn Assets + Send + Sync)>,
    content: ContentAssets<'a>,
}

impl ViewAssets<'_> {
    fn waiting(self) -> Vec<u64> {
        self.content.waiting.into_inner()
    }
}

impl Assets for ViewAssets<'_> {
    fn image(&self, source: &ImageSource) -> ImageState {
        self.images.image(source)
    }
    fn math(&self, src: &str, display: bool, font_size: f32) -> Option<layout::AssetBox> {
        match self.extra {
            Some(e) => e.math(src, display, font_size),
            None => self.content.math(src, display, font_size),
        }
    }
    fn mermaid(&self, src: &str) -> Option<layout::AssetBox> {
        match self.extra {
            Some(e) => e.mermaid(src),
            None => self.content.mermaid(src),
        }
    }
    fn math_error(&self, src: &str, display: bool, font_size: f32) -> Option<String> {
        if self.extra.is_some() {
            return None;
        }
        self.content.math_error(src, display, font_size)
    }
    fn mermaid_error(&self, src: &str) -> Option<String> {
        if self.extra.is_some() {
            return None;
        }
        self.content.mermaid_error(src)
    }
}

const SCROLLBAR: f32 = 12.0;

/// One wheel notch's worth of lines, in pixels, near Chromium's.
const WHEEL_LINE: f32 = 33.3;

impl ReaderView {
    pub fn new(settings: Settings, _cx: &mut Context<Self>) -> Self {
        Self {
            doc: None,
            settings,
            settings_override: None,
            cur: None,
            layouts: HashMap::new(),
            raw: Vec::new(),
            measured: Vec::new(),
            sum_est: 0.0,
            sum_real: 0.0,
            prefix: Vec::new(),
            prefix_dirty: true,
            anchor: (0, 0.0),
            text_anchor: None,
            pending_jump: None,
            pending_reveal: None,
            states: HashMap::new(),
            area_scroll: HashMap::new(),
            selection: None,
            drag: None,
            thumb_drag: None,
            area_thumb_drag: None,
            mouse: point(px(0.0), px(0.0)),
            hover: None,
            hover_thumb: false,
            find: FindState::default(),
            visible: Vec::new(),
            bounds: Bounds::default(),
            images: Arc::new(ImageStore::new(None)),
            images_seen: 0,
            content: ContentStore::new(),
            content_scale: 1.0,
            waiting: HashMap::new(),
            watching: false,
            prefetching: None,
            prefetch_queue_gen: 0,
            last_heading: None,
            perf: Perf { enabled: std::env::var("MDV_PERF").is_ok(), ..Default::default() },
            assets: None,
        }
    }

    /// Reads and parses a file off the main thread. A later open replaces an earlier one.
    pub fn open(&mut self, path: PathBuf, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Result<(), String>, &mut Context<Self>) + 'static) {
        self.perf.opened_at = Some(Instant::now());
        // On Windows these tasks run in an `extern "system"` thread pool callback, where a panic
        // aborts the process and so closes every window. See also the prefetch and search tasks.
        let task = cx.background_spawn(async move {
            std::panic::catch_unwind(|| Document::load(&path)).unwrap_or_else(|_| Err("the viewer could not read this file".to_string()))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(d) => {
                    this.show(Arc::new(d));
                    done(this, Ok(()), cx);
                    cx.notify();
                }
                Err(e) => done(this, Err(e), cx),
            });
        })
        .detach();
    }

    pub fn show(&mut self, doc: Arc<Document>) {
        self.images = Arc::new(ImageStore::new(doc.path.clone()));
        self.images_seen = 0;
        self.layouts.clear();
        self.waiting.clear();
        self.content.cancel_queued();
        self.states.clear();
        self.area_scroll.clear();
        self.selection = None;
        self.drag = None;
        self.anchor = (0, 0.0);
        self.text_anchor = None;
        self.pending_jump = None;
        self.pending_reveal = None;
        self.find = FindState { gen: self.find.gen + 1, ..Default::default() };
        self.prefetching = None;
        self.prefetch_queue_gen += 1;
        self.cur = None;
        self.visible.clear();
        self.doc = Some(doc);
    }

    /// Installs the asset provider. Everything laid out with placeholders is redone, keeping the
    /// text at the top of the view where it is. This is the path an async render arrival takes.
    pub fn set_assets(&mut self, assets: Option<Arc<dyn Assets + Send + Sync>>) {
        self.capture_text_anchor();
        self.assets = assets;
        self.layouts.clear();
        self.prefetch_queue_gen += 1;
    }

    fn effective(&self) -> Settings {
        self.settings_override.unwrap_or(self.settings)
    }

    fn scroll_of(&self, chunk: usize) -> impl Fn(u32) -> f32 + '_ {
        move |key| self.area_scroll.get(&(chunk, key)).copied().unwrap_or(0.0)
    }

    // ---- heights ----------------------------------------------------------------------------

    fn ratio(&self) -> f32 {
        if self.sum_est > 0.0 {
            self.sum_real / self.sum_est
        } else {
            1.0
        }
    }

    fn height(&self, i: usize) -> f32 {
        self.measured[i].unwrap_or_else(|| (self.raw[i] * self.ratio()).round().max(1.0))
    }

    fn known_height(&self, i: usize) -> f32 {
        self.layouts.get(&i).map_or_else(|| self.height(i), |l| l.height)
    }

    fn ensure_prefix(&mut self) {
        if !self.prefix_dirty && self.prefix.len() == self.raw.len() + 1 {
            return;
        }
        self.prefix_dirty = false;
        let n = self.raw.len();
        self.prefix.clear();
        self.prefix.reserve(n + 1);
        let mut s = 0.0f64;
        self.prefix.push(0.0);
        for i in 0..n {
            s += self.height(i) as f64;
            self.prefix.push(s);
        }
    }

    pub fn scroll_y(&mut self) -> f64 {
        self.ensure_prefix();
        self.prefix[self.anchor.0.min(self.prefix.len() - 1)] + self.anchor.1 as f64
    }

    pub fn set_scroll_y(&mut self, y: f64) {
        self.ensure_prefix();
        let n = self.prefix.len() - 1;
        if n == 0 {
            return;
        }
        let i = self.prefix.partition_point(|v| *v <= y).saturating_sub(1).min(n - 1);
        self.anchor = (i, (y - self.prefix[i]) as f32);
        self.text_anchor = None;
    }

    pub fn total_height(&mut self) -> f64 {
        self.ensure_prefix();
        *self.prefix.last().unwrap_or(&0.0)
    }

    pub fn scroll_by(&mut self, dy: f32) {
        self.anchor.1 += dy;
        self.text_anchor = None;
    }

    /// Records the text at the top of the view, so the next layout can restore it exactly.
    fn capture_text_anchor(&mut self) {
        if self.text_anchor.is_some() {
            return;
        }
        if let Some(l) = self.layouts.get(&self.anchor.0) {
            if let Some((byte, top)) = l.text_at_y(self.anchor.1) {
                self.text_anchor = Some((self.anchor.0, byte, self.anchor.1 - top));
            }
        }
    }

    fn invalidate_chunk(&mut self, i: usize) {
        if i == self.anchor.0 {
            self.capture_text_anchor();
        }
        self.layouts.remove(&i);
    }

    // ---- laying out -------------------------------------------------------------------------

    fn build_layout(&mut self, i: usize, opts: &Options, window: &mut Window) -> Arc<ChunkLayout> {
        let doc = self.doc.clone().expect("document");
        let t = Instant::now();
        let default = ChunkState::default();
        let st = self.states.get(&i).unwrap_or(&default);
        let n = doc.chunks.len();
        let mut shaper = GpuiShaper::new(window.text_system().clone());
        let extra = self.assets.as_deref();
        let assets = ViewAssets { images: &self.images, extra, content: ContentAssets::new(&self.content, opts.mode, self.content_scale) };
        // On the UI thread a panic ends the process and so closes every window. If a document finds
        // a bug here, its section comes up empty instead.
        let laid = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let c = doc.chunk(i);
            layout::layout_chunk(&c, opts, st, i == 0, i + 1 == n, &mut shaper, &assets)
        }));
        let l = Arc::new(laid.unwrap_or_else(|_| layout::ChunkLayout { height: opts.font_size * opts.line_height, layers: vec![Default::default()], ..Default::default() }));
        let waits = assets.waiting();
        if waits.is_empty() {
            self.waiting.remove(&i);
        } else {
            self.waiting.insert(i, waits);
        }
        for id in self.images.take_started() {
            self.images.load(id);
        }
        if self.perf.enabled {
            self.perf.layout_ms.push(t.elapsed().as_secs_f32() * 1000.0);
            self.perf.mounts += 1;
        }
        self.note_measure(i, l.height);
        self.layouts.insert(i, l.clone());
        l
    }

    fn note_measure(&mut self, i: usize, h: f32) {
        if self.measured[i].is_none() {
            self.sum_est += self.raw[i];
            self.sum_real += h;
        }
        if self.measured[i] != Some(h) {
            self.measured[i] = Some(h);
            self.prefix_dirty = true;
        }
    }

    fn layout_of(&mut self, i: usize, opts: &Options, window: &mut Window) -> Arc<ChunkLayout> {
        match self.layouts.get(&i) {
            Some(l) => l.clone(),
            None => self.build_layout(i, opts, window),
        }
    }

    /// Everything that must be true before painting: options, anchor, and the visible chunks.
    /// Repaints when maths, diagrams or images finish. Called after layout, which is what asks for
    /// them: nothing else wakes an idle window, so otherwise they wait for the next mouse move.
    fn watch_assets(&mut self, cx: &mut Context<Self>) {
        if self.watching || (self.content.outstanding() == 0 && self.images.pending() == 0) {
            return;
        }
        self.watching = true;
        cx.spawn(async move |this, cx| {
            let mut seen = (0u64, 0u64);
            loop {
                cx.background_executor().timer(Duration::from_millis(33)).await;
                let go = this
                    .update(cx, |v, cx| {
                        let fin = (v.content.finished(), v.images.finished());
                        if fin != seen {
                            seen = fin;
                            cx.notify();
                        }
                        let busy = v.content.outstanding() > 0 || v.images.pending() > 0;
                        v.watching = busy;
                        busy
                    })
                    .unwrap_or(false);
                if !go {
                    break;
                }
            }
        })
        .detach();
    }

    fn prepare_frame(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.bounds = bounds;
        self.visible.clear();
        let Some(doc) = self.doc.clone() else { return };
        let n = doc.chunks.len();
        if n == 0 {
            return;
        }
        let s = self.effective();
        let view_h = f32::from(bounds.size.height);
        let width = f32::from(bounds.size.width);
        let opts = Options { width: (width - SCROLLBAR).max(1.0), padding: s.padding, font_size: s.font_size, line_height: s.line_height, serif: s.serif, wrap: s.wrap, mode: s.mode };
        if self.cur != Some(opts) {
            let keep_measures = self.cur.is_some_and(|c| c.width == opts.width && c.padding == opts.padding && c.font_size == opts.font_size && c.line_height == opts.line_height && c.serif == opts.serif && c.wrap == opts.wrap);
            if self.cur.is_some() {
                self.capture_text_anchor();
            }
            self.layouts.clear();
            if !keep_measures {
                // `relayout()`: estimates change; what was learnt about heights stays as a ratio.
                let r = self.ratio();
                self.raw = doc.chunks.iter().map(|m| layout::estimate(m.bytes(), m.lines, &opts)).collect();
                self.measured = vec![None; n];
                self.sum_est = 1000.0;
                self.sum_real = 1000.0 * r;
            }
            self.prefix_dirty = true;
            self.cur = Some(opts);
            self.prefetch_queue_gen += 1;
        }
        // The window moved to a screen of another density: pictures are redrawn at the new one.
        let scale = window.scale_factor();
        if (scale - self.content_scale).abs() > 1e-3 {
            self.content_scale = scale;
            self.capture_text_anchor();
            self.layouts.clear();
            self.waiting.clear();
            self.prefetch_queue_gen += 1;
        }
        // Maths and diagrams that finished: only the chunks that waited for them are redone.
        let done = self.content.take_done();
        if !done.is_empty() {
            let stale: Vec<usize> = self.waiting.iter().filter(|(_, ks)| ks.iter().any(|k| done.contains(k))).map(|(i, _)| *i).collect();
            for i in stale {
                self.waiting.remove(&i);
                self.invalidate_chunk(i);
            }
        }
        // An image finished: its chunk needs its real size.
        let finished = self.images.finished();
        if finished != self.images_seen {
            self.images_seen = finished;
            self.capture_text_anchor();
            self.layouts.clear();
        }

        // Jumps, then the text anchor, then plain offsets.
        if let Some((i, byte, delta)) = self.pending_jump.take() {
            let l = self.layout_of(i, &opts, window);
            let y = l.y_of(byte).unwrap_or(0.0);
            self.anchor = (i, y + delta);
            self.text_anchor = None;
        }
        if let Some((i, byte)) = self.pending_reveal.take() {
            let l = self.layout_of(i, &opts, window);
            let y = l.y_of(byte).unwrap_or(0.0);
            let on_screen = self.anchor_y_of(i, y, &opts, view_h, window);
            if on_screen.is_none_or(|sy| sy < 40.0 || sy + 20.0 > view_h - 40.0) {
                self.anchor = (i, y - view_h / 3.0);
                self.text_anchor = None;
            }
            self.reveal_in_area(i, byte, &l);
        }
        if let Some((i, byte, delta)) = self.text_anchor {
            let l = self.layout_of(i, &opts, window);
            if let Some(y) = l.y_of(byte) {
                self.anchor = (i, y + delta);
            }
            self.text_anchor = None;
        }

        // Normalise the anchor into its chunk. A short move uses real layouts; a long one steps
        // over chunks on their known or estimated heights, without laying out what it passes.
        let (mut i, mut off) = self.anchor;
        i = i.min(n - 1);
        let near = 3.0 * view_h;
        while off < 0.0 && i > 0 {
            i -= 1;
            let h = if -off < near { self.layout_of(i, &opts, window).height } else { self.known_height(i) };
            off += h;
        }
        off = off.max(0.0);
        loop {
            let h = if off < near { self.layout_of(i, &opts, window).height } else { self.known_height(i) };
            if off < h || i + 1 >= n {
                if off >= near {
                    // Landed on an estimate: settle on the real height.
                    if off < self.layout_of(i, &opts, window).height || i + 1 >= n {
                        break;
                    }
                    continue;
                }
                break;
            }
            off -= h;
            i += 1;
        }
        // Walk down until the view is full, then pull back if the end of the document came first.
        let mut bottom;
        loop {
            let mut y = -off;
            let mut k = i;
            self.visible.clear();
            while k < n && y < view_h {
                let l = self.layout_of(k, &opts, window);
                self.visible.push(Visible { chunk: k, layout: l.clone(), y });
                y += l.height;
                k += 1;
            }
            bottom = y;
            if bottom >= view_h || (i == 0 && off <= 0.0) {
                break;
            }
            let lack = view_h - bottom;
            if off >= lack {
                off -= lack;
                continue;
            }
            let rest = lack - off;
            if i == 0 {
                off = 0.0;
                continue;
            }
            i -= 1;
            off = self.layout_of(i, &opts, window).height - rest;
            if off < 0.0 {
                off = 0.0;
            }
        }
        self.anchor = (i, off);
        self.ensure_prefix();
        self.evict(view_h);
        self.prefetch(&doc, view_h, &opts, window, cx);
        if self.perf.enabled && self.perf.first_content.is_none() {
            if let Some(t) = self.perf.opened_at {
                self.perf.first_content = Some(t.elapsed());
            }
        }
    }

    /// Where `y` in chunk `i` is on screen, if `i` is within a screen of the anchor.
    fn anchor_y_of(&mut self, i: usize, y: f32, opts: &Options, view_h: f32, window: &mut Window) -> Option<f32> {
        let (a, off) = self.anchor;
        if i == a {
            return Some(y - off);
        }
        if i > a {
            let mut acc = -off;
            for k in a..i {
                acc += self.layout_of(k, opts, window).height;
                if acc > view_h {
                    return None;
                }
            }
            return Some(acc + y);
        }
        None
    }

    fn reveal_in_area(&mut self, i: usize, byte: u32, l: &ChunkLayout) {
        for (li, layer) in l.layers.iter().enumerate().skip(1) {
            for b in &layer.boxes {
                if byte < b.byte0 || byte > b.byte1 {
                    continue;
                }
                let Some(line) = b.lines.iter().find(|x| byte >= x.byte0 && byte <= x.byte1) else { continue };
                let area = l.areas[li - 1];
                let (x0, _) = b.x_range(line, byte, byte + 1).unwrap_or((line.x, line.x));
                let cur = self.area_scroll.get(&(i, area.key)).copied().unwrap_or(0.0);
                if x0 - cur < area.view.x + 20.0 || x0 - cur > area.view.x + area.view.w - 40.0 {
                    let max = (area.content_w - area.view.w).max(0.0);
                    self.area_scroll.insert((i, area.key), (x0 - area.view.x - area.view.w / 2.0).clamp(0.0, max));
                }
            }
        }
    }

    /// `unmount`: layouts beyond about six screens go, so memory follows the view, not the file.
    fn evict(&mut self, view_h: f32) {
        if self.layouts.len() < 8 {
            return;
        }
        let (first, last) = match (self.visible.first(), self.visible.last()) {
            (Some(a), Some(b)) => (a.chunk, b.chunk),
            _ => return,
        };
        let keep = 6.0 * view_h as f64;
        let (top, bottom) = (self.prefix[first] - keep, self.prefix[last + 1] + keep);
        let prefix = &self.prefix;
        self.layouts.retain(|i, _| prefix[*i + 1] > top && prefix[*i] < bottom);
    }

    /// Parses the chunks about to come into view on a background thread, and lays out one more
    /// per idle frame, so that scrolling meets finished work.
    fn prefetch(&mut self, doc: &Arc<Document>, view_h: f32, opts: &Options, _window: &mut Window, cx: &mut Context<Self>) {
        let (Some(a), Some(b)) = (self.visible.first(), self.visible.last()) else { return };
        let (first, last) = (a.chunk, b.chunk);
        let margin = 2.0 * view_h as f64;
        let (top, bottom) = (self.prefix[first] - margin, self.prefix[last + 1] + margin);
        let n = doc.chunks.len();
        let mut wanted = None;
        // Below first, then above: reading goes down.
        for k in (last + 1)..n {
            if self.prefix[k] > bottom {
                break;
            }
            if !self.layouts.contains_key(&k) {
                wanted = Some(k);
                break;
            }
        }
        if wanted.is_none() {
            for k in (0..first).rev() {
                if self.prefix[k + 1] < top {
                    break;
                }
                if !self.layouts.contains_key(&k) {
                    wanted = Some(k);
                    break;
                }
            }
        }
        let Some(k) = wanted else { return };
        if self.prefetching.is_some() {
            return;
        }
        self.prefetching = Some(k);
        let gen = self.prefetch_queue_gen;
        let doc2 = doc.clone();
        let opts = *opts;
        let st = self.states.get(&k).cloned().unwrap_or_default();
        let images = self.images.clone();
        let extra = self.assets.clone();
        let content = self.content.clone();
        let content_scale = self.content_scale;
        let text_system = cx.text_system().clone();
        let enabled = self.perf.enabled;
        let task = cx.background_spawn(async move {
            let t = Instant::now();
            let height = opts.font_size * opts.line_height;
            let laid = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let c = doc2.chunk(k);
            // Each worker has its own window text system: GPUI's frame-based layout cache is not
            // shared across threads, and the platform text system underneath is thread-safe.
            let wts = std::rc::Rc::new(gpui::WindowTextSystem::new(text_system));
            let mut shaper = GpuiShaper::new(wts);
            let assets = ViewAssets { images: &images, extra: extra.as_deref(), content: ContentAssets::new(&content, opts.mode, content_scale) };
            let l = layout::layout_chunk(&c, &opts, &st, k == 0, k + 1 == doc2.chunks.len(), &mut shaper, &assets);
            (l, assets.waiting())
            }));
            let (l, waits) = laid.unwrap_or_else(|_| (layout::ChunkLayout { height, layers: vec![Default::default()], ..Default::default() }, Vec::new()));
            (Arc::new(l), t.elapsed(), waits)
        });
        let doc_id = doc.id;
        cx.spawn(async move |this, cx| {
            let (l, took, waits) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.prefetching = None;
                // The result must belong to exactly this parse and these settings.
                let current = this.doc.as_ref().is_some_and(|d| d.id == doc_id) && this.prefetch_queue_gen == gen && this.cur == Some(opts);
                // A formula that finished while this was being laid out leaves it stale.
                let fresh = waits.iter().all(|w| this.content.is_pending(*w));
                if current && fresh && !this.layouts.contains_key(&k) && !this.states.contains_key(&k) {
                    if waits.is_empty() {
                        this.waiting.remove(&k);
                    } else {
                        this.waiting.insert(k, waits);
                    }
                    if enabled {
                        this.perf.layout_ms.push(took.as_secs_f32() * 1000.0);
                    }
                    this.note_measure(k, l.height);
                    this.layouts.insert(k, l);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- navigation -------------------------------------------------------------------------

    /// Scrolls to a heading by its index in the outline: 12 px above it, as `goTo` does.
    pub fn jump_to_heading(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone() else { return };
        let Some(h) = doc.headings.get(index) else { return };
        let i = h.chunk as usize;
        let id = h.id.clone();
        let c = doc.chunk(i);
        let offset = c.anchors.iter().find(|a| a.id == id).map_or(0, |a| a.offset);
        self.pending_jump = Some((i, offset, -12.0));
        cx.notify();
    }

    pub fn jump_to_fragment(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone() else { return };
        if let Some((i, offset)) = doc.locate_id(id, self.anchor.0) {
            self.pending_jump = Some((i, offset, -12.0));
            cx.notify();
        }
    }

    /// The outline entry being read (`currentHeading` in `tabs.js`): the last one at or above a
    /// line 80 px down the view.
    pub fn current_heading(&self) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        if doc.headings.is_empty() {
            return None;
        }
        let v = self.visible.iter().rev().find(|v| v.y <= 80.0).or(self.visible.first())?;
        let m = doc.chunks[v.chunk];
        let mut cur = m.first_heading as i64 - 1;
        if let Some(c) = doc.cached(v.chunk) {
            for k in m.first_heading..m.first_heading + m.heading_count {
                let id = &doc.headings[k as usize].id;
                let Some(a) = c.anchors.iter().find(|a| a.id == *id) else { break };
                let Some(y) = v.layout.y_of(a.offset) else { break };
                if v.y + y > 80.0 {
                    break;
                }
                cur = k as i64;
            }
        }
        (cur >= 0).then_some(cur as usize)
    }

    // ---- selection --------------------------------------------------------------------------

    fn pos_at(&self, p: gpui::Point<Pixels>) -> Option<DocPos> {
        let x = f32::from(p.x - self.bounds.origin.x);
        let y = f32::from(p.y - self.bounds.origin.y);
        // Above or below the view: use the first or last chunk on screen.
        let v = self.visible.iter().find(|v| y < v.y + v.layout.height).or(self.visible.last())?;
        let off = v.layout.hit_test(x, y - v.y, &self.scroll_of(v.chunk))?;
        Some(DocPos::new(v.chunk as u32, off))
    }

    /// The text of the visible chunks, a line at a time, in window coordinates.
    fn a11y_lines(&self, bounds: Bounds<Pixels>) -> Vec<A11yLine> {
        let Some(doc) = self.doc.as_ref() else { return Vec::new() };
        let (ox, oy) = (f64::from(f32::from(bounds.origin.x)), f64::from(f32::from(bounds.origin.y)));
        let mut out = Vec::new();
        for vis in &self.visible {
            let content = doc.chunk(vis.chunk);
            for (li, layer) in vis.layout.layers.iter().enumerate() {
                for (bi, b) in layer.boxes.iter().enumerate() {
                    for (ni, line) in b.lines.iter().enumerate() {
                        let text = content.text.get(line.byte0 as usize..line.byte1 as usize).unwrap_or("").trim_end_matches('\n');
                        if text.is_empty() {
                            continue;
                        }
                        let y0 = oy + f64::from(vis.y + line.top);
                        let x0 = ox + f64::from(line.x);
                        out.push(A11yLine {
                            key: (vis.chunk, li, bi, ni),
                            text: text.to_string(),
                            rect: gpui::accesskit::Rect { x0, y0, x1: x0 + f64::from(line.width), y1: y0 + f64::from(line.height) },
                        });
                    }
                }
            }
        }
        out
    }

    pub fn selected_text(&self) -> Option<String> {
        let s = self.selection?;
        let (a, b) = s.ordered();
        if a == b {
            return None;
        }
        Some(self.doc.as_ref()?.text_between(a, b))
    }

    pub fn copy_selection(&self, cx: &mut Context<Self>) -> bool {
        match self.selected_text() {
            Some(t) => {
                cx.write_to_clipboard(ClipboardItem::new_string(t));
                true
            }
            None => false,
        }
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let Some(doc) = self.doc.clone() else { return };
        let n = doc.chunks.len();
        if n == 0 {
            return;
        }
        // The last chunk's text length needs it parsed; everything between stays unparsed.
        let end = doc.chunk(n - 1).text.len() as u32;
        let (a, b) = (DocPos::new(0, 0), DocPos::new(n as u32 - 1, end));
        self.selection = Some(Selection { anchor: a, head: b, anchor_end: a });
        cx.notify();
    }

    pub fn set_selection(&mut self, a: DocPos, b: DocPos) {
        self.selection = Some(Selection { anchor: a, head: b, anchor_end: a });
    }

    fn select_unit(&self, pos: DocPos, gran: Granularity) -> Option<(DocPos, DocPos)> {
        let doc = self.doc.as_ref()?;
        let v = self.visible.iter().find(|v| v.chunk == pos.chunk as usize)?;
        let c = doc.chunk(pos.chunk as usize);
        let span = match gran {
            Granularity::Word => layout::word_at(&c.text, pos.offset),
            Granularity::Para => layout::paragraph_at(&c.text, &v.layout, pos.offset),
            Granularity::Char => ByteSpan::new(pos.offset, pos.offset),
        };
        Some((DocPos::new(pos.chunk, span.start), DocPos::new(pos.chunk, span.end)))
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.mouse = e.position;
        let x = f32::from(e.position.x - self.bounds.origin.x);
        let y = f32::from(e.position.y - self.bounds.origin.y);
        let w = f32::from(self.bounds.size.width);
        // The page scrollbar.
        if x >= w - SCROLLBAR {
            let (top, thumb, _max) = self.thumb_metrics();
            if y >= top && y < top + thumb {
                self.thumb_drag = Some((f32::from(e.position.y), self.scroll_y()));
            } else {
                let page = f32::from(self.bounds.size.height) * 0.9;
                self.scroll_by(if y < top { -page } else { page });
            }
            cx.notify();
            return;
        }
        // An area's scrollbar.
        for v in &self.visible {
            for area in &v.layout.areas {
                let track = layout::Rect::new(area.view.x, area.scrollbar_y, area.view.w, SCROLLBAR);
                if track.contains(x, y - v.y) {
                    let cur = self.area_scroll.get(&(v.chunk, area.key)).copied().unwrap_or(0.0);
                    self.area_thumb_drag = Some((v.chunk, area.key, f32::from(e.position.x), cur));
                    return;
                }
            }
        }
        let hit = self.hit_under(e.position);
        let hit = hit.map(|(c, h)| (c, h.kind));
        let gran = match e.click_count {
            2 => Granularity::Word,
            n if n >= 3 => Granularity::Para,
            _ => Granularity::Char,
        };
        let button_like = matches!(&hit, Some((_, HitKind::Copy { .. } | HitKind::Sort { .. } | HitKind::Details { .. })));
        self.drag = Some(Drag { gran, start: e.position, moved: false, hit, autoscrolling: false });
        if !button_like {
            if let Some(pos) = self.pos_at(e.position) {
                self.selection = match (gran, e.modifiers.shift, self.selection) {
                    (Granularity::Char, true, Some(mut s)) => {
                        s.head = pos;
                        Some(s)
                    }
                    (Granularity::Char, _, _) => Some(Selection { anchor: pos, head: pos, anchor_end: pos }),
                    _ => self.select_unit(pos, gran).map(|(a, b)| Selection { anchor: a, head: b, anchor_end: a }),
                };
            }
        }
        let _ = window;
        cx.notify();
    }

    fn hit_under(&self, p: gpui::Point<Pixels>) -> Option<(usize, Hit)> {
        let x = f32::from(p.x - self.bounds.origin.x);
        let y = f32::from(p.y - self.bounds.origin.y);
        let v = self.visible.iter().find(|v| y >= v.y && y < v.y + v.layout.height)?;
        v.layout.hit_at(x, y - v.y, &self.scroll_of(v.chunk)).map(|h| (v.chunk, h.clone()))
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.mouse = e.position;
        if let Some((y0, start)) = self.thumb_drag {
            let (_, thumb, max) = self.thumb_metrics();
            let travel = (f32::from(self.bounds.size.height) - thumb).max(1.0) as f64;
            let dy = f32::from(e.position.y) as f64 - y0 as f64;
            self.set_scroll_y((start + dy / travel * max).clamp(0.0, max));
            cx.notify();
            return;
        }
        if let Some((chunk, key, x0, start)) = self.area_thumb_drag {
            if let Some(v) = self.visible.iter().find(|v| v.chunk == chunk) {
                if let Some(a) = v.layout.areas.iter().find(|a| a.key == key) {
                    let max = (a.content_w - a.view.w).max(0.0);
                    let thumb = (a.view.w / a.content_w * a.view.w).max(24.0);
                    let travel = (a.view.w - thumb).max(1.0);
                    let dx = f32::from(e.position.x) - x0;
                    self.area_scroll.insert((chunk, key), (start + dx / travel * max).clamp(0.0, max));
                    cx.notify();
                }
            }
            return;
        }
        if e.pressed_button == Some(MouseButton::Left) && self.drag.is_some() {
            self.drag_to(e.position, cx);
            return;
        }
        // Hover: re-render only when what is under the pointer changes.
        let inside = self.bounds.contains(&e.position);
        let hover = if inside { self.hit_under(e.position).map(|(c, h)| (c, hit_id(&h))) } else { None };
        let x = f32::from(e.position.x - self.bounds.origin.x);
        let thumb = inside && x >= f32::from(self.bounds.size.width) - SCROLLBAR;
        if hover != self.hover || thumb != self.hover_thumb {
            self.hover = hover;
            self.hover_thumb = thumb;
            cx.notify();
        }
        let _ = window;
    }

    fn drag_to(&mut self, p: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(d) = self.drag.as_mut() else { return };
        let dist = (f32::from(p.x - d.start.x)).abs() + (f32::from(p.y - d.start.y)).abs();
        if dist > 3.0 {
            d.moved = true;
        }
        if !d.moved {
            return;
        }
        let gran = d.gran;
        let needs_auto = !d.autoscrolling && (p.y < self.bounds.origin.y || p.y > self.bounds.origin.y + self.bounds.size.height);
        if matches!(d.hit, Some((_, HitKind::Copy { .. } | HitKind::Sort { .. } | HitKind::Details { .. }))) {
            return;
        }
        if needs_auto {
            d.autoscrolling = true;
            self.start_autoscroll(cx);
        }
        self.extend_selection(p, gran);
        cx.notify();
    }

    fn extend_selection(&mut self, p: gpui::Point<Pixels>, gran: Granularity) {
        let Some(pos) = self.pos_at(p) else { return };
        let Some(mut s) = self.selection else { return };
        match gran {
            Granularity::Char => s.head = pos,
            _ => {
                if let Some((a, b)) = self.select_unit(pos, gran) {
                    let anchor_lo = s.anchor.min(s.anchor_end);
                    s.head = if a < anchor_lo { a } else { b };
                }
            }
        }
        self.selection = Some(s);
    }

    /// While dragging past the edge of the view the selection keeps growing as the page moves,
    /// across chunks that are parsed only when it is copied.
    fn start_autoscroll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(30)).await;
            let go = this.update(cx, |this, cx| {
                let Some(d) = this.drag.as_ref() else { return false };
                let gran = d.gran;
                let (top, bottom) = (this.bounds.origin.y, this.bounds.origin.y + this.bounds.size.height);
                let p = this.mouse;
                let over = if p.y < top {
                    f32::from(p.y - top)
                } else if p.y > bottom {
                    f32::from(p.y - bottom)
                } else {
                    0.0
                };
                if over == 0.0 {
                    if let Some(d) = this.drag.as_mut() {
                        d.autoscrolling = false;
                    }
                    return false;
                }
                this.scroll_by(over.clamp(-120.0, 120.0));
                this.extend_selection(p, gran);
                cx.notify();
                true
            });
            if !matches!(go, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.thumb_drag = None;
        self.area_thumb_drag = None;
        let Some(d) = self.drag.take() else { return };
        cx.notify();
        if d.moved {
            return;
        }
        // A click without a drag leaves a collapsed selection, the caret a shift-click extends,
        // and a target may fire.
        if let (Some((chunk, kind)), Some((c2, h2))) = (d.hit, self.hit_under(e.position)) {
            if c2 == chunk && same_target(&kind, &h2.kind) {
                self.activate(chunk, &kind, window, cx);
            }
        }
    }

    fn activate(&mut self, chunk: usize, kind: &HitKind, _window: &mut Window, cx: &mut Context<Self>) {
        match kind {
            HitKind::Link(i) => {
                let target = self.visible.iter().find(|v| v.chunk == chunk).and_then(|v| v.layout.links.get(*i as usize).cloned());
                if let Some(t) = target {
                    self.follow(&t, cx);
                }
            }
            HitKind::Copy { key, text } => {
                let Some(doc) = self.doc.clone() else { return };
                let c = doc.chunk(chunk);
                cx.write_to_clipboard(ClipboardItem::new_string(c.slice(*text).to_string()));
                self.states.entry(chunk).or_default().copied = Some(*key);
                self.invalidate_chunk(chunk);
                let key = *key;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(1200)).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(s) = this.states.get_mut(&chunk) {
                            if s.copied == Some(key) {
                                s.copied = None;
                                this.invalidate_chunk(chunk);
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
            HitKind::Sort { key, col } => {
                let st = self.states.entry(chunk).or_default();
                match sort::next_sort(st.sorts.get(key).copied(), *col) {
                    Some(v) => {
                        st.sorts.insert(*key, v);
                    }
                    None => {
                        st.sorts.remove(key);
                    }
                }
                self.invalidate_chunk(chunk);
            }
            HitKind::Details { key } => {
                let open = self.details_open(chunk, *key);
                self.states.entry(chunk).or_default().details.insert(*key, !open);
                self.invalidate_chunk(chunk);
            }
        }
        cx.notify();
    }

    fn details_open(&self, chunk: usize, key: u32) -> bool {
        if let Some(v) = self.states.get(&chunk).and_then(|s| s.details.get(&key)) {
            return *v;
        }
        let Some(doc) = self.doc.as_ref() else { return false };
        fn find(b: &[Block], key: u32) -> Option<bool> {
            b.iter().find_map(|x| match x {
                Block::Details(d) if d.span.text.start == key => Some(d.open),
                Block::Details(d) => find(&d.blocks, key),
                Block::Quote { blocks, .. } | Block::Aligned { blocks, .. } => find(blocks, key),
                Block::List(l) => l.items.iter().find_map(|i| find(&i.blocks, key)),
                _ => None,
            })
        }
        find(&doc.chunk(chunk).blocks, key).unwrap_or(false)
    }

    fn follow(&mut self, t: &LinkTarget, cx: &mut Context<Self>) {
        match t {
            LinkTarget::Web(u) | LinkTarget::Mail(u) => cx.open_url(u),
            LinkTarget::Fragment(id) => self.jump_to_fragment(id, cx),
            LinkTarget::Local { path, fragment } => {
                if !paths::is_markdown(path) {
                    return;
                }
                let Some(doc_path) = self.doc.as_ref().and_then(|d| d.path.clone()) else { return };
                match paths::resolve(&doc_path, path) {
                    Ok(p) => cx.emit(ReaderEvent::OpenLocal { path: p, fragment: fragment.clone() }),
                    Err(e) => cx.emit(ReaderEvent::Toast(e.to_string())),
                }
            }
        }
    }

    /// Keys for the document itself. Returns whether the key was used.
    pub fn key_down(&mut self, e: &KeyDownEvent, mac: bool, cx: &mut Context<Self>) -> bool {
        let m = &e.keystroke.modifiers;
        let command = if mac { m.platform } else { m.control };
        let view = f32::from(self.bounds.size.height);
        match e.keystroke.key.as_str() {
            "c" if command => return self.copy_selection(cx),
            "a" if command => self.select_all(cx),
            "up" if !command => self.scroll_by(-40.0),
            "down" if !command => self.scroll_by(40.0),
            "pageup" => self.scroll_by(-view * 0.9),
            "pagedown" => self.scroll_by(view * 0.9),
            "space" if !command => self.scroll_by(if m.shift { -view * 0.9 } else { view * 0.9 }),
            "home" => self.set_scroll_y(0.0),
            "end" => {
                let t = self.total_height();
                self.set_scroll_y(t);
            }
            _ => return false,
        }
        cx.notify();
        true
    }

    // ---- find -------------------------------------------------------------------------------

    /// Searches after the 160 ms the web reader waits while typing.
    pub fn find_query(&mut self, q: &str, delay: bool, cx: &mut Context<Self>) {
        let folded = fold::fold(q);
        self.find.gen += 1;
        let gen = self.find.gen;
        self.find.q = folded.clone();
        self.find.counts.clear();
        self.find.total = 0;
        self.find.pos = None;
        self.find.current = None;
        self.find.spans.clear();
        self.find.search_time = None;
        self.find.searching = !folded.is_empty();
        cx.notify();
        let Some(doc) = self.doc.clone() else { return };
        if folded.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            if delay {
                cx.background_executor().timer(Duration::from_millis(160)).await;
            }
            let still = this.update(cx, |this, _| this.find.gen == gen).unwrap_or(false);
            if !still {
                return;
            }
            let q2 = folded.clone();
            let doc2 = doc.clone();
            let t = Instant::now();
            let chunks = doc2.chunks.len();
            let counts = cx
                .background_spawn(async move {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| doc2.search(&q2))).unwrap_or_else(|_| vec![0; chunks])
                })
                .await;
            let took = t.elapsed();
            let _ = this.update(cx, |this, cx| {
                // Only for exactly this query on exactly this parse.
                if this.find.gen != gen || !this.doc.as_ref().is_some_and(|d| d.id == doc.id) {
                    return;
                }
                this.find.searching = false;
                this.find.search_time = Some(took);
                this.find.total = counts.iter().map(|c| *c as u64).sum();
                this.find.counts = counts;
                this.find_start(cx);
            });
        })
        .detach();
    }

    /// The first match at or after the top of the view.
    fn find_start(&mut self, cx: &mut Context<Self>) {
        if self.find.total == 0 {
            cx.notify();
            return;
        }
        let here = self.anchor.0.min(self.find.counts.len() - 1);
        let mut k: u64 = self.find.counts[..here].iter().map(|c| *c as u64).sum();
        if self.find.counts[here] > 0 {
            let spans = self.spans_for(here);
            let idx = self.visible.iter().find(|v| v.chunk == here).and_then(|v| spans.iter().position(|s| v.layout.y_of(s.start).is_some_and(|y| v.y + y >= 0.0)));
            k += idx.unwrap_or(spans.len().saturating_sub(1)) as u64;
        }
        if k >= self.find.total {
            k = 0;
        }
        self.find_show(k, cx);
    }

    fn spans_for(&mut self, chunk: usize) -> Arc<Vec<ByteSpan>> {
        if let Some(s) = self.find.spans.get(&chunk) {
            return s.clone();
        }
        let Some(doc) = self.doc.clone() else { return Arc::default() };
        let c = doc.chunk(chunk);
        let spans = Arc::new(fold::find_spans(&c.text, &c.no_find, &self.find.q));
        if self.find.spans.len() > 32 {
            self.find.spans.clear();
        }
        self.find.spans.insert(chunk, spans.clone());
        spans
    }

    /// Next or previous, wrapping around.
    pub fn find_step(&mut self, dir: i64, cx: &mut Context<Self>) {
        let total = self.find.total as i64;
        if total == 0 {
            return;
        }
        let cur = self.find.pos.map_or(-1, |p| p as i64);
        let next = if cur < 0 && dir < 0 { total - 1 } else { (cur + dir).rem_euclid(total) };
        self.find_show(next as u64, cx);
    }

    fn find_show(&mut self, k: u64, cx: &mut Context<Self>) {
        let mut rest = k;
        let mut chunk = 0;
        for (i, c) in self.find.counts.iter().enumerate() {
            if rest < *c as u64 {
                chunk = i;
                break;
            }
            rest -= *c as u64;
        }
        let spans = self.spans_for(chunk);
        let Some(&span) = spans.get(rest as usize) else { return };
        self.find.pos = Some(k);
        self.find.current = Some((chunk, span));
        self.open_details_for(chunk, span.start);
        self.pending_reveal = Some((chunk, span.start));
        cx.notify();
    }

    /// A match in a closed `<details>` opens it, as a browser's find-in-page does.
    fn open_details_for(&mut self, chunk: usize, offset: u32) {
        let Some(doc) = self.doc.clone() else { return };
        let c = doc.chunk(chunk);
        fn walk(b: &[Block], off: u32, out: &mut Vec<(u32, bool)>) {
            for x in b {
                if !x.span().text.contains(off) && x.span().text.end != off {
                    continue;
                }
                match x {
                    Block::Details(d) => {
                        if !d.summary_text.contains(off) {
                            out.push((d.span.text.start, d.open));
                        }
                        walk(&d.blocks, off, out);
                    }
                    Block::Quote { blocks, .. } | Block::Aligned { blocks, .. } => walk(blocks, off, out),
                    Block::List(l) => l.items.iter().for_each(|i| walk(&i.blocks, off, out)),
                    _ => {}
                }
            }
        }
        let mut found = Vec::new();
        walk(&c.blocks, offset, &mut found);
        let mut changed = false;
        for (key, default_open) in found {
            let st = self.states.entry(chunk).or_default();
            if !st.details.get(&key).copied().unwrap_or(default_open) {
                st.details.insert(key, true);
                changed = true;
            }
        }
        if changed {
            self.invalidate_chunk(chunk);
        }
    }

    pub fn find_close(&mut self, cx: &mut Context<Self>) {
        self.find = FindState { gen: self.find.gen + 1, ..Default::default() };
        cx.notify();
    }

    pub fn find_count_text(&self) -> String {
        if self.find.q.is_empty() || self.find.searching {
            String::new()
        } else if self.find.total == 0 {
            "No results".into()
        } else {
            format!("{} of {}", self.find.pos.map_or(0, |p| p + 1), self.find.total)
        }
    }

    // ---- scrollbar --------------------------------------------------------------------------

    /// (thumb top, thumb height, maximum scroll).
    fn thumb_metrics(&mut self) -> (f32, f32, f64) {
        let view = f32::from(self.bounds.size.height);
        let total = self.total_height();
        let max = (total - view as f64).max(0.0);
        let thumb = ((view as f64 / total.max(1.0) * view as f64) as f32).clamp(24.0, view);
        let travel = (view - thumb).max(0.0);
        let y = self.scroll_y();
        let top = if max > 0.0 { ((y / max).clamp(0.0, 1.0) as f32) * travel } else { 0.0 };
        (top, thumb, max)
    }

    // ---- diagnostics ------------------------------------------------------------------------

    pub fn mounted_chunks(&self) -> usize {
        self.layouts.len()
    }

    pub fn top_line_text(&self, n: usize) -> String {
        let Some(v) = self.visible.first() else { return String::new() };
        let Some(doc) = self.doc.as_ref() else { return String::new() };
        let c = doc.chunk(v.chunk);
        let Some((byte, _)) = v.layout.text_at_y(self.anchor.1) else { return String::new() };
        c.text[byte as usize..].chars().take(n).collect::<String>().replace('\n', "⏎")
    }

    /// Window coordinates of a visible link's first fragment, for scripted clicks.
    pub fn link_point(&self, target: &str) -> Option<(i32, i32)> {
        let view_h = f32::from(self.bounds.size.height);
        for v in &self.visible {
            for h in &v.layout.hits {
                let HitKind::Link(i) = h.kind else { continue };
                let matches = v.layout.links.get(i as usize).is_some_and(|l| format!("{l:?}").contains(target));
                let y = v.y + h.rect.y + h.rect.h / 2.0;
                if matches && y > 0.0 && y < view_h {
                    let x = f32::from(self.bounds.origin.x) + h.rect.x + h.rect.w / 2.0;
                    return Some((x as i32, (f32::from(self.bounds.origin.y) + y) as i32));
                }
            }
        }
        None
    }

    /// Chunk, fractional offset, and the first text at the top of the view.
    pub fn reading_position(&self) -> (usize, f32, u32) {
        let Some(v) = self.visible.first() else { return (0, 0.0, 0) };
        let frac = self.anchor.1 / v.layout.height.max(1.0);
        let byte = v.layout.text_at_y(self.anchor.1).map_or(0, |x| x.0);
        (v.chunk, frac, byte)
    }
}

fn hit_id(h: &Hit) -> u32 {
    match &h.kind {
        HitKind::Link(i) => *i * 4,
        HitKind::Copy { key, .. } => key.wrapping_mul(4) + 1,
        HitKind::Sort { key, col } => (key.wrapping_add(*col as u32 * 7919)).wrapping_mul(4) + 2,
        HitKind::Details { key } => key.wrapping_mul(4) + 3,
    }
}

fn same_target(a: &HitKind, b: &HitKind) -> bool {
    match (a, b) {
        (HitKind::Link(x), HitKind::Link(y)) => x == y,
        (HitKind::Copy { key: x, .. }, HitKind::Copy { key: y, .. }) => x == y,
        (HitKind::Sort { key: x, col: p }, HitKind::Sort { key: y, col: q }) => x == y && p == q,
        (HitKind::Details { key: x }, HitKind::Details { key: y }) => x == y,
        _ => false,
    }
}

impl Render for ReaderView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex_1().min_w_0().h_full().child(ReaderElement { a11y_lines: Vec::new(), view: cx.entity() })
    }
}

/// The element GPUI lays out and paints. It holds no state of its own.
pub struct ReaderElement {
    view: Entity<ReaderView>,
    a11y_lines: Vec<A11yLine>,
}

/// One laid-out line of text, for assistive technology. A route check, not the full exposure: only
/// the chunks on screen, flat, with no heading structure, and the Document is not focusable yet.
struct A11yLine {
    key: (usize, usize, usize, usize),
    text: String,
    rect: gpui::accesskit::Rect,
}

impl IntoElement for ReaderElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub struct FrameState {
    hitbox: Hitbox,
    started: Instant,
}

impl Element for ReaderElement {
    type RequestLayoutState = ();
    type PrepaintState = FrameState;

    fn id(&self) -> Option<ElementId> {
        Some("reader-pane".into())
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn a11y_role(&self) -> Option<gpui::accesskit::Role> {
        Some(gpui::accesskit::Role::Document)
    }
    fn a11y_synthetic_children(&mut self, _prepaint: &mut Self::PrepaintState, builder: &mut gpui::A11ySubtreeBuilder) {
        use gpui::accesskit::{Node, Role};
        for line in self.a11y_lines.drain(..) {
            let mut run = Node::new(Role::TextRun);
            run.set_bounds(line.rect);
            run.set_character_lengths(line.text.chars().map(|c| c.len_utf8() as u8).collect::<Vec<u8>>());
            run.set_value(line.text);
            let id = builder.synthetic_node_id(line.key);
            builder.push_child(id, run);
        }
    }

    fn request_layout(&mut self, _id: Option<&GlobalElementId>, _i: Option<&InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let style = Style { size: size(gpui::relative(1.0).into(), gpui::relative(1.0).into()), ..Default::default() };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(&mut self, _id: Option<&GlobalElementId>, _i: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _r: &mut (), window: &mut Window, cx: &mut App) -> FrameState {
        let started = Instant::now();
        self.view.update(cx, |v, cx| {
            v.prepare_frame(bounds, window, cx);
            v.watch_assets(cx);
        });
        if window.is_a11y_active() {
            let t0 = Instant::now();
            self.a11y_lines = self.view.read(cx).a11y_lines(bounds);
            if std::env::var_os("MDV_A11Y_PERF").is_some() {
                eprintln!("[a11y] {} lines in {:?}", self.a11y_lines.len(), t0.elapsed());
            }
        }
        FrameState { hitbox: window.insert_hitbox(bounds, HitboxBehavior::Normal), started }
    }

    fn paint(&mut self, _id: Option<&GlobalElementId>, _i: Option<&InspectorElementId>, bounds: Bounds<Pixels>, _r: &mut (), state: &mut FrameState, window: &mut Window, cx: &mut App) {
        let view = self.view.clone();
        let cursor = self.view.update(cx, |v, cx| {
            let c = paint_reader(v, bounds, window, cx);
            if v.perf.enabled {
                let now = Instant::now();
                v.perf.work_ms.push((now - state.started).as_secs_f32() * 1000.0);
                if let Some(last) = v.perf.last_paint {
                    v.perf.interval_ms.push((now - last).as_secs_f32() * 1000.0);
                }
                v.perf.last_paint = Some(now);
            }
            c
        });
        window.set_cursor_style(cursor, &state.hitbox);
        // The outline reads this before the reader lays out, so a change needs one more frame.
        let changed = self.view.update(cx, |v, _| {
            let cur = v.current_heading();
            let changed = cur != v.last_heading;
            v.last_heading = cur;
            changed
        });
        if changed {
            window.on_next_frame(|w, _| w.refresh());
        }

        let (hb, v2) = (state.hitbox.clone(), view.clone());
        window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                v2.update(cx, |v, cx| {
                    let d = e.delta.pixel_delta(px(WHEEL_LINE));
                    let (dx, dy) = (f32::from(d.x), f32::from(d.y));
                    if !wheel_area(v, e.position, if e.modifiers.shift && dx == 0.0 { dy } else { dx }) {
                        v.scroll_by(-dy);
                    }
                    cx.notify();
                });
            }
        });
        let (hb, v2) = (state.hitbox.clone(), view.clone());
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left && hb.is_hovered(window) {
                v2.update(cx, |v, cx| v.mouse_down(e, window, cx));
            }
        });
        let v2 = view.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                v2.update(cx, |v, cx| v.mouse_move(e, window, cx));
            }
        });
        let v2 = view;
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                v2.update(cx, |v, cx| v.mouse_up(e, window, cx));
            }
        });
    }
}

fn wheel_area(v: &mut ReaderView, p: gpui::Point<Pixels>, dx: f32) -> bool {
    if dx == 0.0 {
        return false;
    }
    let (x, y) = (f32::from(p.x - v.bounds.origin.x), f32::from(p.y - v.bounds.origin.y));
    for vis in &v.visible {
        for a in &vis.layout.areas {
            if a.view.contains(x, y - vis.y) {
                let max = (a.content_w - a.view.w).max(0.0);
                let e = v.area_scroll.entry((vis.chunk, a.key)).or_insert(0.0);
                *e = (*e - dx).clamp(0.0, max);
                return true;
            }
        }
    }
    false
}

fn hsla(c: u32) -> Hsla {
    rgb(c).into()
}

/// What a paint needs to know about one frame.
struct Paint<'a> {
    origin: gpui::Point<Pixels>,
    sel: Option<(DocPos, DocPos)>,
    current: Option<(usize, ByteSpan)>,
    spans: Option<&'a [ByteSpan]>,
    chunk: usize,
    scale: f32,
    content: &'a ContentStore,
    view_h: f32,
    y: f32,
    selection_bg: Hsla,
    selection_fg: Hsla,
    mark: Hsla,
    mark_current: Hsla,
    current_fg: Hsla,
    hover_bg: Hsla,
}

fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

fn paint_reader(v: &mut ReaderView, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<ReaderView>) -> CursorStyle {
    let s = v.effective();
    let p = *s.mode.palette();
    let scale = window.scale_factor();
    let view_h = f32::from(bounds.size.height);
    let visible = v.visible.clone();
    let sel = v.selection.map(|x| x.ordered()).filter(|(a, b)| a != b);
    let find_q = !v.find.q.is_empty() && v.find.total > 0;
    let current = v.find.current;
    let mouse = window.mouse_position();
    let (mx, my) = (f32::from(mouse.x - bounds.origin.x), f32::from(mouse.y - bounds.origin.y));
    let mut cursor = CursorStyle::Arrow;
    let hover = v.hover;
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for vis in &visible {
            let spans = if find_q && v.find.counts.get(vis.chunk).copied().unwrap_or(0) > 0 { Some(v.spans_for(vis.chunk)) } else { None };
            let scrolls: Vec<f32> = vis.layout.areas.iter().map(|a| v.area_scroll.get(&(vis.chunk, a.key)).copied().unwrap_or(0.0)).collect();
            let ctx = Paint {
                origin: bounds.origin,
                sel,
                current,
                spans: spans.as_deref().map(|x| x.as_slice()),
                chunk: vis.chunk,
                scale,
                content: &v.content,
                view_h,
                y: vis.y,
                selection_bg: hsla(p.accent),
                selection_fg: hsla(p.bg),
                mark: hsla(p.mark),
                mark_current: hsla(p.mark_current),
                current_fg: hsla(mdv_theme::BRAND_BLACK),
                hover_bg: hsla(p.bg_hover),
            };
            let hovered = hover.filter(|h| h.0 == vis.chunk).map(|h| h.1);
            paint_layer(&vis.layout.layers[0], 0.0, &ctx, &vis.layout, hovered, 0, &v.images, window, cx);
            for (i, area) in vis.layout.areas.iter().enumerate() {
                let sx = scrolls[i];
                let clip = Bounds::new(point(bounds.origin.x + px(area.view.x), bounds.origin.y + px(vis.y + area.view.y)), size(px(area.view.w), px(area.view.h)));
                window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                    paint_layer(&vis.layout.layers[i + 1], sx, &ctx, &vis.layout, hovered, i as u32 + 1, &v.images, window, cx);
                });
                paint_area_scrollbar(area, sx, &ctx, s.mode, window);
            }
            // The cursor: a pointer on targets, an I-beam on text.
            if my >= vis.y && my < vis.y + vis.layout.height {
                let scroll = |key: u32| v.area_scroll.get(&(vis.chunk, key)).copied().unwrap_or(0.0);
                if vis.layout.hit_at(mx, my - vis.y, &scroll).is_some() {
                    cursor = CursorStyle::PointingHand;
                } else if mx < f32::from(bounds.size.width) - SCROLLBAR {
                    let over_text = vis.layout.layers.iter().flat_map(|l| &l.boxes).any(|b| b.selectable && b.lines.iter().any(|l| {
                        my - vis.y >= l.top && my - vis.y < l.top + l.height && mx >= l.x && mx < l.x + l.width.max(4.0)
                    }));
                    if over_text {
                        cursor = CursorStyle::IBeam;
                    }
                }
            }
        }
        // The page scrollbar: `::-webkit-scrollbar`, 12 px with a thumb inset by 3.
        let (top, thumb, max) = v.thumb_metrics();
        if max > 0.0 {
            let x = bounds.origin.x + bounds.size.width - px(SCROLLBAR) + px(3.0);
            let dragging = v.thumb_drag.is_some();
            let color = if dragging || (v.hover_thumb && my >= top && my < top + thumb) { p.muted } else { p.border };
            window.paint_quad(fill(Bounds::new(point(x, bounds.origin.y + px(top + 3.0)), size(px(SCROLLBAR - 6.0), px((thumb - 6.0).max(1.0)))), hsla(color)));
        }
    });
    if v.drag.is_some() {
        cursor = CursorStyle::IBeam;
    }
    cursor
}

fn paint_area_scrollbar(a: &Area, sx: f32, c: &Paint, mode: Mode, window: &mut Window) {
    let p = mode.palette();
    let max = (a.content_w - a.view.w).max(0.0);
    if max <= 0.0 {
        return;
    }
    let thumb = (a.view.w / a.content_w * a.view.w).max(24.0);
    let travel = (a.view.w - thumb).max(0.0);
    let x = a.view.x + sx / max * travel;
    let y = c.y + a.scrollbar_y;
    let o = c.origin;
    window.paint_quad(fill(Bounds::new(point(o.x + px(x + 3.0), o.y + px(y + 3.0)), size(px((thumb - 6.0).max(1.0)), px(SCROLLBAR - 6.0))), hsla(p.border)));
}

#[allow(clippy::too_many_arguments)]
fn paint_layer(layer: &Layer, sx: f32, c: &Paint, layout: &ChunkLayout, hovered: Option<u32>, layer_ix: u32, images: &ImageStore, window: &mut Window, cx: &mut App) {
    let (ox, oy) = (f32::from(c.origin.x) - sx, f32::from(c.origin.y) + c.y);
    let view_top = -c.y;
    let view_bottom = c.view_h - c.y;
    for r in &layer.rects {
        if r.rect.y > view_bottom || r.rect.bottom() < view_top {
            continue;
        }
        let x0 = snap(ox + r.rect.x, c.scale);
        let y0 = snap(oy + r.rect.y, c.scale);
        let x1 = snap(ox + r.rect.x + r.rect.w, c.scale);
        let y1 = snap(oy + r.rect.y + r.rect.h, c.scale);
        let (w, h) = ((x1 - x0).max(1.0 / c.scale), (y1 - y0).max(1.0 / c.scale));
        let bounds = Bounds::new(point(px(x0), px(y0)), size(px(w), px(h)));
        let bg = r.fill.map_or(gpui::transparent_black(), hsla);
        let radii: Corners<Pixels> = Corners::all(px(r.radius));
        match r.border {
            Some(b) => {
                let style = if b.dashed { BorderStyle::Dashed } else { BorderStyle::Solid };
                let widths = Edges { top: px(b.top), right: px(b.right), bottom: px(b.bottom), left: px(b.left) };
                window.paint_quad(quad(bounds, radii, bg, widths, hsla(b.color), style));
            }
            None => window.paint_quad(quad(bounds, radii, bg, Edges::all(px(0.0)), gpui::transparent_black(), BorderStyle::Solid)),
        }
    }
    // A hovered sortable header takes `--bg-hover`.
    if let Some(id) = hovered {
        for h in &layout.hits {
            if h.layer != layer_ix || hit_id(h) != id {
                continue;
            }
            if let HitKind::Sort { .. } = h.kind {
                let p = Bounds::new(point(px(ox + h.rect.x + 1.0), px(oy + h.rect.y + 1.0)), size(px(h.rect.w - 1.0), px(h.rect.h - 1.0)));
                window.paint_quad(fill(p, c.hover_bg));
            }
        }
    }
    for b in &layer.boxes {
        if b.rect.y > view_bottom || b.rect.bottom() < view_top {
            continue;
        }
        paint_box(b, ox, oy, view_top, view_bottom, c, images, window, cx);
    }
    for poly in &layer.polys {
        let Some(&(x0, y0)) = poly.points.first() else { continue };
        if y0 > view_bottom + 40.0 || y0 < view_top - 40.0 {
            continue;
        }
        let mut path = Path::new(point(px(ox + x0), px(oy + y0)));
        for &(x, y) in &poly.points[1..] {
            path.line_to(point(px(ox + x), px(oy + y)));
        }
        window.paint_path(path, hsla(poly.color));
    }
    if let Some(id) = hovered {
        for h in &layout.hits {
            if h.layer == layer_ix && hit_id(h) == id {
                if let Some((y, color)) = h.underline {
                    let p = Bounds::new(point(px(snap(ox + h.rect.x, c.scale)), px(snap(oy + y, c.scale))), size(px(h.rect.w), px(1.0)));
                    window.paint_quad(fill(p, hsla(color)));
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_box(b: &TextBox, ox: f32, oy: f32, view_top: f32, view_bottom: f32, c: &Paint, images: &ImageStore, window: &mut Window, cx: &mut App) {
    let chunk = c.chunk as u32;
    // The selected bytes of this chunk.
    let sel = c.sel.and_then(|(a, z)| {
        if chunk < a.chunk || chunk > z.chunk || !b.selectable {
            return None;
        }
        let lo = if chunk == a.chunk { a.offset } else { 0 };
        let hi = if chunk == z.chunk { z.offset } else { u32::MAX };
        (lo < hi).then_some((lo, hi))
    });
    let current = c.current.filter(|(ci, _)| *ci == c.chunk).map(|x| x.1).filter(|_| b.selectable);
    let first = b.lines.partition_point(|l| l.top + l.height < view_top);
    let mut last_color = u32::MAX;
    let mut hsl = Hsla::default();
    for l in &b.lines[first..] {
        if l.top > view_bottom {
            break;
        }
        let baseline = snap(oy + l.baseline, c.scale);
        // `::highlight(find)` and `::selection` backgrounds come first.
        if b.selectable {
            if let Some(spans) = c.spans {
                let from = spans.partition_point(|s| s.end <= l.byte0);
                for s in &spans[from..] {
                    if s.start >= l.byte1 {
                        break;
                    }
                    if let Some((x0, x1)) = b.x_range(l, s.start, s.end) {
                        let g = text_box_on_line(b, l, s.start);
                        let top = baseline + g.0;
                        window.paint_quad(fill(Bounds::new(point(px(ox + x0), px(top)), size(px(x1 - x0), px(g.1))), c.mark));
                    }
                }
            }
            if let Some(s) = current {
                if s.start < l.byte1 && s.end > l.byte0 {
                    if let Some((x0, x1)) = b.x_range(l, s.start, s.end) {
                        let g = text_box_on_line(b, l, s.start);
                        window.paint_quad(fill(Bounds::new(point(px(ox + x0), px(baseline + g.0)), size(px(x1 - x0), px(g.1))), c.mark_current));
                    }
                }
            }
            if let Some((lo, hi)) = sel {
                if hi > l.byte0 && lo <= l.byte1 {
                    let range = b.x_range(l, lo, hi);
                    let (x0, x1) = match range {
                        Some(r) => r,
                        // An empty line inside the selection shows a sliver, as browsers do.
                        None if l.g0 == l.g1 && lo <= l.byte0 && hi > l.byte0 => (l.x, l.x + 6.0),
                        None => (0.0, 0.0),
                    };
                    if x1 > x0 {
                        window.paint_quad(fill(Bounds::new(point(px(ox + x0), px(snap(oy + l.top, c.scale))), size(px(x1 - x0), px(l.height))), c.selection_bg));
                    }
                }
            }
        }
        for g in &b.glyphs[l.g0 as usize..l.g1 as usize] {
            if g.flags & FLAG_SPACE != 0 {
                continue;
            }
            let x = ox + l.x + g.x0 - l.x_shift;
            let s = &b.spans[g.span as usize];
            if g.flags & FLAG_ATOM != 0 {
                paint_atom(&b.atoms[g.id as usize], x, baseline + s.descent, s.color, images, c.content, window);
                continue;
            }
            let selected = sel.is_some_and(|(lo, hi)| g.byte >= lo && g.byte < hi);
            let is_current = current.is_some_and(|m| g.byte >= m.start && g.byte < m.end);
            let color = if selected {
                c.selection_fg
            } else if is_current {
                c.current_fg
            } else {
                if s.color != last_color {
                    last_color = s.color;
                    hsl = hsla(s.color);
                }
                hsl
            };
            let origin = point(px(x), px(baseline + s.shift));
            let (font, id) = (FontId(g.font as usize), GlyphId(g.id));
            if g.flags & FLAG_EMOJI != 0 {
                let _ = window.paint_emoji(origin, font, id, px(s.size));
            } else {
                let _ = window.paint_glyph(origin, font, id, px(s.size), color);
                if g.flags & FLAG_FAKE_BOLD != 0 {
                    // Share Tech Mono has no bold, and GPUI does not embolden. A second pass a
                    // fraction of a pixel across is what browsers' synthetic bold looks like.
                    let _ = window.paint_glyph(point(px(x + (s.size / 36.0).max(0.25)), px(baseline + s.shift)), font, id, px(s.size), color);
                }
            }
        }
    }
    let _ = cx;
}

/// (offset above the baseline, height) of the text at a byte, for `::highlight` boxes.
fn text_box_on_line(b: &TextBox, l: &layout::Line, byte: u32) -> (f32, f32) {
    let g = b.glyphs[l.g0 as usize..l.g1 as usize].iter().find(|g| g.byte >= byte);
    match g {
        Some(g) => {
            let s = &b.spans[g.span as usize];
            (s.shift - s.ascent, s.ascent + s.descent)
        }
        None => (-l.height * 0.75, l.height),
    }
}

/// Where to paint a maths or diagram raster so that every raster pixel lands on exactly one device
/// pixel. The raster is `ceil(size * scale)` pixels, so painting it into the layout size (a
/// fractional number of device pixels, at a fractional offset) makes the GPU resample it with a
/// bilinear filter, which softens thin strokes. A raster reduced for size (a different scale from
/// the window's) is painted at the layout size instead.
fn device_aligned(x: f32, top: f32, w: f32, h: f32, img: &RenderImage, scale: f32) -> Bounds<Pixels> {
    let sz = img.size(0);
    let (iw, ih) = (sz.width.0 as f32, sz.height.0 as f32);
    let exact = (iw - (w * scale).ceil()).abs() < 1.0 && (ih - (h * scale).ceil()).abs() < 1.0;
    if !exact {
        return Bounds::new(point(px(x), px(top)), size(px(w), px(h)));
    }
    let (l, t) = ((x * scale).round(), (top * scale).round());
    Bounds::new(point(px(l / scale), px(t / scale)), size(px(iw / scale), px(ih / scale)))
}

fn paint_atom(a: &layout::AtomDraw, x: f32, bottom: f32, color: u32, images: &ImageStore, content: &ContentStore, window: &mut Window) {
    match a {
        layout::AtomDraw::Image { key, w, h } => {
            if let Some(img) = images.by_key(*key) {
                let bounds = Bounds::new(point(px(x), px(bottom - h)), size(px(*w), px(*h)));
                let _ = window.paint_image(bounds, bounds, Corners::all(px(0.0)), img, 0, false);
            }
        }
        // A held web image with no alt text: `min-width: 2em; min-height: 1.4em` in a dashed box.
        layout::AtomDraw::EmptyBox { w, h } if *w > 0.0 => {
            let bounds = Bounds::new(point(px(x), px(bottom - h)), size(px(*w), px(*h)));
            window.paint_quad(quad(bounds, Corners::all(px(0.0)), gpui::transparent_black(), Edges::all(px(1.0)), hsla(color), BorderStyle::Dashed));
        }
        layout::AtomDraw::Asset { key, w, h } => {
            let scale = window.scale_factor();
            // A diagram shown narrower than it was drawn is drawn again at the size it is shown.
            let (img, queued) = if *w > 0.0 { content.for_display(*key, *w, scale) } else { (content.by_key(*key), false) };
            if queued {
                // Render again next frame, which starts the watcher that repaints when it is done.
                window.request_animation_frame();
            }
            if let Some(img) = img {
                let bounds = device_aligned(x, bottom - h, *w, *h, &img, window.scale_factor());
                let _ = window.paint_image(bounds, bounds, Corners::all(px(0.0)), img, 0, false);
            }
        }
        layout::AtomDraw::EmptyBox { .. } => {}
    }
}
