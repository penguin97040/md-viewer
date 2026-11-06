//! Cuts laid-out chunks into pages. Rules, from the print block of `style.css`: never cut through
//! a line, keep `pre`, table rows, pictures, display maths, diagrams and quotes whole when they
//! fit on a page, and never leave a heading as the last thing on a page.

use std::collections::HashMap;

use mdv_reader::layout::ChunkLayout;
use mdv_reader::model::ChunkContent;

#[derive(Clone, Copy, Debug)]
pub struct Slice {
    pub chunk: usize,
    /// Chunk-space range, in CSS px.
    pub from: f32,
    pub to: f32,
    /// Where the slice starts down the page's content area, in CSS px.
    pub page_y: f32,
}

#[derive(Default, Debug)]
pub struct Page {
    pub slices: Vec<Slice>,
}

pub struct Plan {
    pub pages: Vec<Page>,
    /// Anchor id to (page, y down the page in px).
    pub anchors: HashMap<String, (usize, f32)>,
}

pub struct Planner {
    page_h: f32,
    pages: Vec<Page>,
    py: f32,
    anchors: HashMap<String, (usize, f32)>,
}

impl Plan {
    pub fn new(page_h: f32) -> Planner {
        Planner { page_h, pages: vec![Page::default()], py: 0.0, anchors: HashMap::new() }
    }
}

impl Planner {
    pub fn finish(mut self) -> Plan {
        if self.pages.len() > 1 && self.pages.last().is_some_and(|p| p.slices.is_empty()) {
            self.pages.pop();
        }
        Plan { pages: self.pages, anchors: self.anchors }
    }

    fn new_page(&mut self) {
        if self.pages.last().is_some_and(|p| !p.slices.is_empty()) {
            self.pages.push(Page::default());
        }
        self.py = 0.0;
    }

    pub fn add_chunk(&mut self, index: usize, c: &ChunkContent, l: &ChunkLayout) {
        let h = l.height;
        if !(h > 0.0) {
            return;
        }
        let mut lines: Vec<(f32, f32)> = Vec::new();
        for layer in &l.layers {
            for b in &layer.boxes {
                for ln in &b.lines {
                    lines.push((ln.top, ln.top + ln.height));
                }
            }
        }
        lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut slices: Vec<(usize, Slice)> = Vec::new();
        let mut s = 0.0f32;
        let mut guard = 0;
        while s < h - 0.01 {
            guard += 1;
            if guard > 100_000 {
                break;
            }
            let room = self.page_h - self.py;
            if s + room >= h - 0.01 {
                let sl = Slice { chunk: index, from: s, to: h, page_y: self.py };
                self.pages.last_mut().expect("page").slices.push(sl);
                slices.push((self.pages.len() - 1, sl));
                self.py += h - s;
                break;
            }
            let c_at = self.choose_cut(l, &lines, s, s + room);
            if c_at <= s + 0.5 {
                if self.py > 0.5 {
                    self.new_page();
                    continue;
                }
                // Nothing fits even on a fresh page: cut where the page ends.
                let cut = s + room;
                self.emit(index, s, cut, &mut slices);
                self.new_page();
                s = cut;
                continue;
            }
            self.emit(index, s, c_at, &mut slices);
            self.new_page();
            s = c_at;
        }
        for a in &c.anchors {
            if let Some(y) = l.y_of(a.offset) {
                if let Some((pi, sl)) = slices.iter().find(|(_, sl)| y >= sl.from && y < sl.to).or(slices.last()) {
                    self.anchors.entry(a.id.clone()).or_insert((*pi, sl.page_y + (y - sl.from).max(0.0)));
                }
            }
        }
    }

    fn emit(&mut self, chunk: usize, from: f32, to: f32, out: &mut Vec<(usize, Slice)>) {
        let sl = Slice { chunk, from, to, page_y: self.py };
        self.pages.last_mut().expect("page").slices.push(sl);
        out.push((self.pages.len() - 1, sl));
    }

    /// The best place at or before `limit` to end a page that starts at `s`.
    fn choose_cut(&self, l: &ChunkLayout, lines: &[(f32, f32)], s: f32, limit: f32) -> f32 {
        let mut c = limit;
        for _ in 0..8 {
            let before = c;
            // Whole blocks stay together if they are not taller than a page.
            for &(top, bottom) in &l.keep {
                if top < c - 0.01 && bottom > c + 0.01 && bottom - top <= self.page_h {
                    if top > s + 0.5 || self.py > 0.5 {
                        c = top.max(s);
                    }
                }
            }
            // A heading may not be the last thing on a page.
            for &(top, bottom) in &l.heads {
                if top >= s - 0.01 && bottom <= c + 0.01 {
                    let next_fits = lines.iter().any(|&(t, b)| t >= bottom - 0.5 && b <= c + 0.01);
                    let content_follows = lines.iter().any(|&(t, _)| t >= bottom - 0.5);
                    if !next_fits && content_follows && (top > s + 0.5 || self.py > 0.5) {
                        c = top.max(s);
                    }
                }
            }
            // Never through a line.
            let mut lowest = f32::MAX;
            for &(t, b) in lines {
                if t < c - 0.01 && b > c + 0.01 {
                    lowest = lowest.min(t);
                }
                if t >= c {
                    break;
                }
            }
            if lowest < f32::MAX && lowest > s + 0.5 {
                c = lowest;
            }
            if (c - before).abs() < 0.01 {
                break;
            }
        }
        c
    }
}
