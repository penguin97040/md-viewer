//! A single-line text field for the find bar: caret, selection, clipboard and word movement.

use std::time::Instant;

use gpui::{
    canvas, fill, point, px, size, App, Bounds, ClipboardItem, Font, Hsla, IntoElement, KeyDownEvent, Pixels, ShapedLine, Styled, TextRun, Window,
};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Default)]
pub struct FieldEdit {
    pub changed: bool,
    pub handled: bool,
}

/// The state. It is plain data so it can be tested without a window.
#[derive(Clone, Debug, Default)]
pub struct Field {
    pub text: String,
    pub caret: usize,
    pub anchor: usize,
}

impl Field {
    pub fn selection(&self) -> (usize, usize) {
        (self.caret.min(self.anchor), self.caret.max(self.anchor))
    }

    pub fn selected(&self) -> &str {
        let (a, b) = self.selection();
        &self.text[a..b]
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    pub fn set(&mut self, t: &str) {
        self.text = t.to_string();
        self.select_all();
    }

    pub fn insert(&mut self, t: &str) {
        let (a, b) = self.selection();
        // A field is one line: pasted line breaks become spaces.
        let clean: String = t.chars().map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c }).collect();
        self.text.replace_range(a..b, &clean);
        self.caret = a + clean.len();
        self.anchor = self.caret;
    }

    fn prev(&self, at: usize) -> usize {
        self.text[..at].grapheme_indices(true).next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self, at: usize) -> usize {
        self.text[at..].graphemes(true).next().map_or(at, |g| at + g.len())
    }

    fn prev_word(&self, at: usize) -> usize {
        let mut last = 0;
        for (i, w) in self.text[..at].split_word_bound_indices() {
            if !w.trim().is_empty() {
                last = i;
            }
        }
        last
    }

    fn next_word(&self, at: usize) -> usize {
        let mut seen = false;
        for (i, w) in self.text[at..].split_word_bound_indices() {
            if w.trim().is_empty() {
                if seen {
                    return at + i;
                }
            } else {
                seen = true;
            }
        }
        self.text.len()
    }

    /// Selects the word around a position, for double-click.
    pub fn select_word(&mut self, at: usize) {
        for (i, w) in self.text.split_word_bound_indices() {
            if at >= i && at <= i + w.len() {
                self.anchor = i;
                self.caret = i + w.len();
                return;
            }
        }
    }

    /// Applies a key. `word` is the word-movement modifier, `command` the shortcut modifier.
    pub fn key(&mut self, key: &str, shift: bool, word: bool, command: bool, typed: Option<&str>, clipboard: &mut dyn FnMut(Option<String>) -> Option<String>) -> FieldEdit {
        let mut e = FieldEdit { changed: false, handled: true };
        let before = self.text.len();
        let has_sel = self.caret != self.anchor;
        let target = match key {
            "left" if !shift && has_sel && !word => Some(self.selection().0),
            "right" if !shift && has_sel && !word => Some(self.selection().1),
            "left" => Some(if word { self.prev_word(self.caret) } else { self.prev(self.caret) }),
            "right" => Some(if word { self.next_word(self.caret) } else { self.next(self.caret) }),
            "home" => Some(0),
            "end" => Some(self.text.len()),
            _ => None,
        };
        if let Some(t) = target {
            self.caret = t;
            if !shift {
                self.anchor = t;
            }
            return e;
        }
        match key {
            "backspace" | "delete" => {
                if has_sel {
                    self.insert("");
                } else if key == "backspace" && self.caret > 0 {
                    let p = if word { self.prev_word(self.caret) } else { self.prev(self.caret) };
                    self.text.replace_range(p..self.caret, "");
                    self.caret = p;
                    self.anchor = p;
                } else if key == "delete" && self.caret < self.text.len() {
                    let n = if word { self.next_word(self.caret) } else { self.next(self.caret) };
                    self.text.replace_range(self.caret..n, "");
                }
                e.changed = true;
            }
            "a" if command => self.select_all(),
            "c" if command => {
                if has_sel {
                    clipboard(Some(self.selected().to_string()));
                }
            }
            "x" if command => {
                if has_sel {
                    clipboard(Some(self.selected().to_string()));
                    self.insert("");
                    e.changed = true;
                }
            }
            "v" if command => {
                if let Some(t) = clipboard(None) {
                    self.insert(&t);
                    e.changed = true;
                }
            }
            _ => match typed {
                Some(t) if !command && !t.is_empty() => {
                    self.insert(t);
                    e.changed = true;
                }
                _ => e.handled = false,
            },
        }
        let _ = before;
        e
    }
}

/// Applies a real key event, with the platform's clipboard.
pub fn apply_key(f: &mut Field, e: &KeyDownEvent, mac: bool, cx: &mut App) -> FieldEdit {
    let m = &e.keystroke.modifiers;
    let command = if mac { m.platform } else { m.control };
    let word = if mac { m.alt } else { m.control };
    let mut clip = |w: Option<String>| -> Option<String> {
        match w {
            Some(t) => {
                cx.write_to_clipboard(ClipboardItem::new_string(t));
                None
            }
            None => cx.read_from_clipboard().and_then(|c| c.text()),
        }
    };
    f.key(e.keystroke.key.as_str(), m.shift, word, command, e.keystroke.key_char.as_deref(), &mut clip)
}

pub struct FieldLook {
    pub font: Font,
    pub size: f32,
    pub text: Hsla,
    pub placeholder: Hsla,
    pub selection: Hsla,
    pub selected_text: Hsla,
    pub caret: Hsla,
    pub started: Instant,
    pub focused: bool,
}

fn shape(text: &str, look: &FieldLook, color: Hsla, sel: Option<(usize, usize, Hsla)>, window: &Window) -> ShapedLine {
    let run = |len: usize, c: Hsla| TextRun { len, font: look.font.clone(), color: c, background_color: None, underline: None, strikethrough: None };
    let runs: Vec<TextRun> = match sel {
        Some((a, b, sc)) if a < b => [(a, color), (b - a, sc), (text.len() - b, color)].into_iter().filter(|r| r.0 > 0).map(|(l, c)| run(l, c)).collect(),
        _ => vec![run(text.len(), color)],
    };
    window.text_system().shape_line(text.to_string().into(), px(look.size), &runs, None)
}

/// The caret position for a click at `x` inside a field showing `text`.
pub fn offset_at(text: &str, x: f32, scroll: f32, look: &FieldLook, window: &Window) -> usize {
    if text.is_empty() {
        return 0;
    }
    shape(text, look, look.text, None, window).closest_index_for_x(px((x + scroll).max(0.0)))
}

/// Draws the field's text, selection and caret, scrolling so that the caret stays visible.
pub fn field_canvas(f: &Field, look: FieldLook, width: f32) -> impl IntoElement {
    let f = f.clone();
    canvas(
        move |_, _, _| {},
        move |bounds: Bounds<Pixels>, _, window, _cx| {
            let h = f32::from(bounds.size.height);
            let y = bounds.origin.y + px(((h - look.size * 1.2) / 2.0).max(0.0));
            let line_h = px(look.size * 1.2);
            let empty = f.text.is_empty();
            let (a, b) = f.selection();
            let shown = if empty { "Find" } else { f.text.as_str() };
            let color = if empty { look.placeholder } else { look.text };
            let line = shape(shown, &look, color, (!empty && look.focused).then_some((a, b, look.selected_text)), window);
            let caret_x = if empty { 0.0 } else { f32::from(line.x_for_index(f.caret)) };
            let scroll = (caret_x - width + 2.0).max(0.0);
            let ox = bounds.origin.x - px(scroll);
            window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
                if look.focused && a < b {
                    let (xa, xb) = (f32::from(line.x_for_index(a)), f32::from(line.x_for_index(b)));
                    window.paint_quad(fill(Bounds::new(point(ox + px(xa), y), size(px(xb - xa), line_h)), look.selection));
                }
                let _ = line.paint(point(ox, y), line_h, gpui::TextAlign::Left, None, window, _cx);
                // The caret blinks at 530 ms like a native field.
                let on = (look.started.elapsed().as_millis() / 530) % 2 == 0;
                if look.focused && on && a == b {
                    window.paint_quad(fill(Bounds::new(point(ox + px(caret_x), y), size(px(1.0), line_h)), look.caret));
                }
            });
        },
    )
    .w(px(width))
    .h_full()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: &mut Field, key: &str, shift: bool, word: bool, command: bool, typed: Option<&str>) -> FieldEdit {
        let mut clip = |_w: Option<String>| -> Option<String> { Some("pasted\ntext".to_string()) };
        f.key(key, shift, word, command, typed, &mut clip)
    }

    #[test]
    fn typing_and_caret_movement() {
        let mut f = Field::default();
        for c in ["a", "b", "c"] {
            assert!(press(&mut f, c, false, false, false, Some(c)).changed);
        }
        assert_eq!((f.text.as_str(), f.caret), ("abc", 3));
        press(&mut f, "left", false, false, false, None);
        press(&mut f, "x", false, false, false, Some("x"));
        assert_eq!((f.text.as_str(), f.caret), ("abxc", 3));
        press(&mut f, "home", false, false, false, None);
        assert_eq!(f.caret, 0);
    }

    #[test]
    fn shift_selects_and_typing_replaces() {
        let mut f = Field::default();
        f.set("hello world");
        press(&mut f, "home", false, false, false, None);
        press(&mut f, "right", true, true, false, None);
        assert_eq!(f.selected(), "hello");
        press(&mut f, "J", false, false, false, Some("J"));
        assert_eq!(f.text, "J world");
    }

    #[test]
    fn graphemes_move_as_one_and_backspace_keeps_utf8_valid() {
        let mut f = Field::default();
        f.text = "ae\u{301}😀".into();
        f.caret = f.text.len();
        f.anchor = f.caret;
        press(&mut f, "backspace", false, false, false, None);
        assert_eq!(f.text, "ae\u{301}");
        press(&mut f, "backspace", false, false, false, None);
        assert_eq!(f.text, "a");
    }

    #[test]
    fn paste_flattens_lines() {
        let mut f = Field::default();
        assert!(press(&mut f, "v", false, false, true, None).changed);
        assert_eq!(f.text, "pasted text");
    }

    #[test]
    fn word_keys() {
        let mut f = Field::default();
        f.set("one two three");
        press(&mut f, "end", false, false, false, None);
        press(&mut f, "backspace", false, true, false, None);
        assert_eq!(f.text, "one two ");
        f.select_word(1);
        assert_eq!(f.selected(), "one");
    }
}
