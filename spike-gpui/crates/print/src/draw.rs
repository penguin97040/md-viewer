//! Draws laid-out slices into a PDF with krilla.

use std::cell::RefCell;
use std::sync::Arc;

use krilla::color::rgb;
use krilla::geom::{Path, PathBuilder, Point, Rect, Size, Transform};
use krilla::image::Image;
use krilla::action::{Action, LinkAction};
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::destination::{Destination, XyzDestination};
use krilla::metadata::Metadata;
use krilla::outline::{Outline, OutlineNode};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, FillRule, Stroke, StrokeDash};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla::Data;
use krilla::Document as KDocument;
use krilla_svg::{SurfaceExt, SvgSettings};
use mdv_reader::doc::Document;
use mdv_reader::layout::{AtomDraw, Border, ChunkLayout, HitKind, RectItem, TextBox, FLAG_ATOM, FLAG_EMOJI, FLAG_FAKE_BOLD};
use mdv_reader::model::{ChunkContent, LinkTarget};

use crate::assets::{Item, RasterKind, Store};
use crate::fonts::FontSet;
use crate::paginate::{Plan, Slice};
use crate::{Geometry, PrintError, MARGIN_X_PT, MARGIN_Y_PT};

const PT: f32 = 0.75;

pub struct Pdf<'a> {
    doc: KDocument,
    geo: &'a Geometry,
    fonts: Vec<Font>,
    set: &'a FontSet,
    usvg: usvg::Options<'static>,
    pages_made: usize,
}

struct Env<'e> {
    fonts: &'e [Font],
    set: &'e FontSet,
    usvg: &'e usvg::Options<'static>,
    store: &'e RefCell<Store>,
}

impl<'a> Pdf<'a> {
    pub fn new(geo: &'a Geometry, set: &'a FontSet) -> Result<Self, PrintError> {
        let mut fonts = Vec::new();
        for f in &set.fonts {
            let data: &'static [u8] = f.data;
            fonts.push(Font::new(data.into(), 0).ok_or_else(|| PrintError::Pdf(format!("font {} could not be read", f.name)))?);
        }
        let mut db = usvg::fontdb::Database::new();
        for f in set.fonts.iter().take(6) {
            db.load_font_data(f.data.to_vec());
        }
        db.set_sans_serif_family("Chakra Petch");
        db.set_serif_family("Chakra Petch");
        db.set_monospace_family("Share Tech Mono");
        let usvg = usvg::Options { font_family: "Chakra Petch".into(), fontdb: Arc::new(db), ..usvg::Options::default() };
        Ok(Self { doc: KDocument::new(), geo, fonts, set, usvg, pages_made: 0 })
    }

    /// Draws one page from its slices. `get` returns the chunk and layout a slice cuts from.
    /// Returns how many link annotations the page got.
    pub fn draw_page(&mut self, slices: &[Slice], get: &mut dyn FnMut(usize) -> (Arc<ChunkContent>, Arc<ChunkLayout>), store: &RefCell<Store>, plan: &Plan) -> usize {
        let Some(settings) = PageSettings::from_wh(self.geo.page_w_pt, self.geo.page_h_pt) else { return 0 };
        let env = Env { fonts: &self.fonts, set: self.set, usvg: &self.usvg, store };
        let mut links: Vec<([f32; 4], LinkTarget)> = Vec::new();
        let mut page = self.doc.start_page_with(settings);
        {
            let mut s = page.surface();
            for sl in slices {
                let (c, l) = get(sl.chunk);
                draw_slice(&mut s, &env, &c, &l, sl, self.geo.content_w_px, &mut links);
            }
            s.finish();
        }
        let mut n = 0;
        for (r, t) in links {
            let Some(rect) = Rect::from_xywh(r[0], r[1], r[2].max(0.5), r[3].max(0.5)) else { continue };
            let Some(target) = target_of(&t, plan, plan.pages.len()) else { continue };
            page.add_annotation(Annotation::new_link(LinkAnnotation::new(rect, target), None));
            n += 1;
        }
        page.finish();
        self.pages_made += 1;
        n
    }

    pub fn set_outline(&mut self, doc: &Document, plan: &Plan, limit: usize) -> usize {
        struct N {
            level: u8,
            node: OutlineNode,
            kids: Vec<N>,
        }
        fn close(stack: &mut Vec<N>, roots: &mut Vec<N>, to_level: u8) {
            while stack.last().is_some_and(|t| t.level >= to_level) {
                let done = stack.pop().expect("node");
                match stack.last_mut() {
                    Some(p) => p.kids.push(done),
                    None => roots.push(done),
                }
            }
        }
        fn build(n: N) -> OutlineNode {
            let mut node = n.node;
            for k in n.kids {
                node.push_child(build(k));
            }
            node
        }
        let mut roots: Vec<N> = Vec::new();
        let mut stack: Vec<N> = Vec::new();
        let mut count = 0usize;
        for h in doc.headings.iter().take(limit) {
            let Some(&(page, y)) = plan.anchors.get(&h.id) else { continue };
            if page >= self.pages_made {
                continue;
            }
            let dest = XyzDestination::new(page, Point::from_xy(MARGIN_X_PT, (MARGIN_Y_PT + y * PT - 4.0).max(0.0)));
            close(&mut stack, &mut roots, h.level);
            let text: String = h.text.chars().filter(|c| !c.is_control()).take(200).collect();
            stack.push(N { level: h.level, node: OutlineNode::new(text, dest), kids: Vec::new() });
            count += 1;
        }
        close(&mut stack, &mut roots, 0);
        if !roots.is_empty() {
            let mut outline = Outline::new();
            for r in roots {
                outline.push_child(build(r));
            }
            self.doc.set_outline(outline);
        }
        count
    }

    pub fn finish(mut self, title: Option<String>) -> Result<Vec<u8>, PrintError> {
        if self.pages_made == 0 {
            if let Some(s) = PageSettings::from_wh(self.geo.page_w_pt, self.geo.page_h_pt) {
                self.doc.start_page_with(s).finish();
            }
        }
        let mut md = Metadata::new().creator("MD Viewer".to_string());
        if let Some(t) = title {
            let t: String = t.chars().filter(|c| !c.is_control()).take(300).collect();
            md = md.title(t);
        }
        self.doc.set_metadata(md);
        self.doc.finish().map_err(|e| PrintError::Pdf(format!("{e:?}")))
    }
}

fn target_of(t: &LinkTarget, plan: &Plan, pages: usize) -> Option<Target> {
    let clean = |s: &str| -> Option<String> {
        (s.len() <= 2000 && !s.chars().any(|c| c.is_control() || c == ' ')).then(|| s.to_string())
    };
    match t {
        LinkTarget::Web(u) => {
            let l = u.to_ascii_lowercase();
            (l.starts_with("http://") || l.starts_with("https://")).then(|| clean(u)).flatten().map(|u| Target::Action(Action::Link(LinkAction::new(u))))
        }
        LinkTarget::Mail(m) => clean(m).filter(|m| m.to_ascii_lowercase().starts_with("mailto:")).map(|m| Target::Action(Action::Link(LinkAction::new(m)))),
        LinkTarget::Fragment(id) => {
            let &(page, y) = plan.anchors.get(id.trim_start_matches('#'))?;
            (page < pages).then(|| Target::Destination(Destination::Xyz(XyzDestination::new(page, Point::from_xy(MARGIN_X_PT, (MARGIN_Y_PT + y * PT - 4.0).max(0.0))))))
        }
        // A link to another file means nothing on paper.
        LinkTarget::Local { .. } => None,
    }
}

fn col(c: u32) -> rgb::Color {
    rgb::Color::new((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn fill(c: u32) -> Fill {
    Fill { paint: col(c).into(), opacity: NormalizedF32::ONE, rule: FillRule::NonZero }
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let mut b = PathBuilder::new();
    if r <= 0.01 {
        b.push_rect(Rect::from_xywh(x, y, w, h)?);
    } else {
        let r = r.min(w / 2.0).min(h / 2.0);
        let k = 0.552_284_7 * r;
        b.move_to(x + r, y);
        b.line_to(x + w - r, y);
        b.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
        b.line_to(x + w, y + h - r);
        b.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
        b.line_to(x + r, y + h);
        b.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
        b.line_to(x, y + r);
        b.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
        b.close();
    }
    b.finish()
}

fn draw_slice(s: &mut Surface<'_>, env: &Env<'_>, c: &ChunkContent, l: &ChunkLayout, sl: &Slice, content_w_px: f32, links: &mut Vec<([f32; 4], LinkTarget)>) {
    let top_pt = MARGIN_Y_PT + sl.page_y * PT;
    let Some(clip) = rrect(MARGIN_X_PT - 3.0, top_pt, content_w_px * PT + 6.0, (sl.to - sl.from) * PT, 0.0) else { return };
    s.push_clip_path(&clip, &FillRule::NonZero);
    s.push_transform(&Transform::from_row(PT, 0.0, 0.0, PT, MARGIN_X_PT, top_pt - sl.from * PT));
    let (from, to) = (sl.from, sl.to);
    for layer in &l.layers {
        for r in &layer.rects {
            draw_rect(s, r, from, to);
        }
        for p in &layer.polys {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for q in &p.points {
                lo = lo.min(q.1);
                hi = hi.max(q.1);
            }
            if hi < from || lo > to || p.points.len() < 3 {
                continue;
            }
            let mut b = PathBuilder::new();
            for (i, q) in p.points.iter().enumerate() {
                if i == 0 {
                    b.move_to(q.0, q.1);
                } else {
                    b.line_to(q.0, q.1);
                }
            }
            b.close();
            if let Some(path) = b.finish() {
                s.set_stroke(None);
                s.set_fill(Some(fill(p.color)));
                s.draw_path(&path);
            }
        }
        for b in &layer.boxes {
            if b.rect.y >= to - 0.25 || b.rect.bottom() <= from + 0.25 {
                continue;
            }
            draw_box(s, env, c, b, from, to);
        }
    }
    s.pop();
    s.pop();
    for h in &l.hits {
        if let HitKind::Link(i) = h.kind {
            if h.layer != 0 || h.rect.y > to || h.rect.bottom() < from {
                continue;
            }
            let Some(t) = l.links.get(i as usize) else { continue };
            let (y0, y1) = (h.rect.y.max(from), h.rect.bottom().min(to));
            if y1 <= y0 {
                continue;
            }
            links.push(([MARGIN_X_PT + h.rect.x * PT, top_pt + (y0 - from) * PT, h.rect.w * PT, (y1 - y0) * PT], t.clone()));
        }
    }
}

fn draw_rect(s: &mut Surface<'_>, r: &RectItem, from: f32, to: f32) {
    let q = r.rect;
    if q.y >= to - 0.01 || q.bottom() <= from + 0.01 || q.w <= 0.0 || q.h <= 0.0 {
        return;
    }
    if let Some(f) = r.fill {
        if let Some(p) = rrect(q.x, q.y, q.w, q.h, r.radius) {
            s.set_stroke(None);
            s.set_fill(Some(fill(f)));
            s.draw_path(&p);
        }
    }
    if let Some(b) = &r.border {
        draw_border(s, q, b, r.radius);
    }
}

fn draw_border(s: &mut Surface<'_>, q: mdv_reader::layout::Rect, b: &Border, radius: f32) {
    let uniform = (b.top - b.right).abs() < 0.01 && (b.top - b.bottom).abs() < 0.01 && (b.top - b.left).abs() < 0.01;
    if uniform {
        let w = b.top;
        if w <= 0.0 {
            return;
        }
        if let Some(p) = rrect(q.x + w / 2.0, q.y + w / 2.0, q.w - w, q.h - w, (radius - w / 2.0).max(0.0)) {
            let dash = b.dashed.then(|| StrokeDash { array: vec![3.0 * w.max(1.0), 3.0 * w.max(1.0)], offset: 0.0 });
            s.set_fill(None);
            s.set_stroke(Some(Stroke { paint: col(b.color).into(), width: w, dash, ..Stroke::default() }));
            s.draw_path(&p);
            s.set_stroke(None);
        }
        return;
    }
    s.set_stroke(None);
    s.set_fill(Some(fill(b.color)));
    let sides = [(b.top, q.x, q.y, q.w, b.top), (b.bottom, q.x, q.bottom() - b.bottom, q.w, b.bottom), (b.left, q.x, q.y, b.left, q.h), (b.right, q.x + q.w - b.right, q.y, b.right, q.h)];
    for (w, x, y, rw, rh) in sides {
        if w > 0.0 {
            if let Some(p) = rrect(x, y, rw, rh, 0.0) {
                s.draw_path(&p);
            }
        }
    }
}

fn draw_box(s: &mut Surface<'_>, env: &Env<'_>, c: &ChunkContent, b: &TextBox, from: f32, to: f32) {
    for l in &b.lines {
        if l.top + l.height <= from + 0.25 || l.top >= to - 0.25 {
            continue;
        }
        let glyphs = &b.glyphs[l.g0 as usize..(l.g1 as usize).min(b.glyphs.len())];
        let mut i = 0;
        while i < glyphs.len() {
            let g = glyphs[i];
            let span = &b.spans[g.span as usize];
            if g.flags & FLAG_ATOM != 0 {
                let x = l.x + g.x0 - l.x_shift;
                if let Some(a) = b.atoms.get(g.id as usize) {
                    draw_atom(s, env, a, x, l.baseline + span.descent);
                }
                i += 1;
                continue;
            }
            let mut j = i;
            while j < glyphs.len() && glyphs[j].span == g.span && glyphs[j].font == g.font && glyphs[j].flags & FLAG_ATOM == 0 {
                j += 1;
            }
            if g.flags & FLAG_EMOJI == 0 {
                let after = glyphs.get(j).map(|n| n.byte);
                draw_run(s, env, c, &glyphs[i..j], after, span.size, span.color, (l.x, l.x_shift, l.baseline), span.shift, span.chrome);
            }
            i = j;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_run(s: &mut Surface<'_>, env: &Env<'_>, c: &ChunkContent, run: &[mdv_reader::layout::Glyph], after: Option<u32>, size: f32, color: u32, line: (f32, f32, f32), shift: f32, chrome: bool) {
    let (line_x, line_shift, baseline) = line;
    let Some(first) = run.first() else { return };
    let (Some(font), Some(loaded)) = (env.fonts.get(first.font as usize), env.set.fonts.get(first.font as usize)) else { return };
    if size <= 0.0 {
        return;
    }
    let mut text = String::new();
    let mut kg: Vec<KrillaGlyph> = Vec::with_capacity(run.len());
    let mut k = 0;
    while k < run.len() {
        let g = run[k];
        let start = text.len();
        let continuation = k > 0 && run[k - 1].byte == g.byte && !chrome;
        if !continuation {
            let mut done = false;
            if !chrome {
                let b0 = g.byte as usize;
                if b0 < c.text.len() && c.text.is_char_boundary(b0) {
                    let next = run[k + 1..].iter().map(|n| n.byte as usize).find(|&nb| nb != b0);
                    let mut end = match next {
                        Some(nb) if nb > b0 => nb,
                        _ => {
                            let line_end = after.map_or(0, |a| a as usize);
                            if line_end > b0 && line_end - b0 <= 8 {
                                line_end
                            } else {
                                b0 + c.text[b0..].chars().next().map_or(1, char::len_utf8)
                            }
                        }
                    };
                    end = end.min(c.text.len());
                    if end > b0 + 16 {
                        end = b0 + c.text[b0..].chars().next().map_or(1, char::len_utf8);
                    }
                    while end < c.text.len() && !c.text.is_char_boundary(end) {
                        end += 1;
                    }
                    if end > b0 {
                        text.push_str(&c.text[b0..end]);
                        done = true;
                    }
                }
            }
            if !done {
                if let Some(ch) = loaded.char_of(g.id as u16) {
                    text.push(ch);
                }
            }
        }
        kg.push(KrillaGlyph::new(GlyphId::new(g.id), ((g.x1 - g.x0) / size).max(0.0), 0.0, 0.0, 0.0, start..text.len(), None));
        k += 1;
    }
    let x = line_x + first.x0 - line_shift;
    let y = baseline + shift;
    s.set_stroke(None);
    s.set_fill(Some(fill(color)));
    s.draw_glyphs(Point::from_xy(x, y), &kg, font.clone(), &text, size, false);
    if first.flags & FLAG_FAKE_BOLD != 0 {
        s.draw_glyphs(Point::from_xy(x + (size / 36.0).max(0.25), y), &kg, font.clone(), &text, size, false);
    }
}

fn draw_atom(s: &mut Surface<'_>, env: &Env<'_>, a: &AtomDraw, x: f32, bottom: f32) {
    match a {
        AtomDraw::Asset { key, w, h } => {
            let Some(asset) = env.store.borrow_mut().svg(*key) else { return };
            let Ok(tree) = usvg::Tree::from_str(&asset.svg, env.usvg) else { return };
            let Some(size) = Size::from_wh(*w, *h) else { return };
            s.push_transform(&Transform::from_translate(x, bottom - h));
            let _ = s.draw_svg(&tree, size, SvgSettings::default());
            s.pop();
        }
        AtomDraw::Image { key, w, h } => {
            let (bytes, kind) = match env.store.borrow().items.get(key) {
                Some(Item::Raster { bytes, kind, .. }) => (bytes.clone(), *kind),
                _ => return,
            };
            let data: Data = bytes.into();
            let img = match kind {
                RasterKind::Png => Image::from_png(data, true),
                RasterKind::Jpeg => Image::from_jpeg(data, true),
                RasterKind::Gif => Image::from_gif(data, true),
                RasterKind::Webp => Image::from_webp(data, true),
            };
            let (Ok(img), Some(size)) = (img, Size::from_wh(*w, *h)) else { return };
            s.push_transform(&Transform::from_translate(x, bottom - h));
            s.draw_image(img, size);
            s.pop();
        }
        AtomDraw::EmptyBox { w, h } if *w > 0.0 => {
            if let Some(p) = rrect(x + 0.5, bottom - h + 0.5, w - 1.0, h - 1.0, 0.0) {
                s.set_fill(None);
                s.set_stroke(Some(Stroke { paint: col(0xd0d7de).into(), width: 1.0, dash: Some(StrokeDash { array: vec![3.0, 3.0], offset: 0.0 }), ..Stroke::default() }));
                s.draw_path(&p);
                s.set_stroke(None);
            }
        }
        AtomDraw::EmptyBox { .. } => {}
    }
}
