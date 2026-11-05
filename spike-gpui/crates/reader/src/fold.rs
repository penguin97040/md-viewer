//! Find's text folding, matching Go's `foldText`: `strings.ToLower` (rune by rune, simple case
//! mapping) then `strings.Fields` joined by one space.

use crate::model::ByteSpan;

/// Go's `unicode.ToLower`. Rust's `char::to_lowercase` agrees except where it expands: U+0130
/// (dotted capital I) becomes two characters in Rust and plain `i` in Go.
#[inline]
pub fn go_lower(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    if c == '\u{130}' {
        return 'i';
    }
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(a), None) => a,
        _ => c,
    }
}

/// Go's `unicode.IsSpace`. Rust's `char::is_whitespace` is the same White_Space set.
#[inline]
pub fn go_space(c: char) -> bool {
    c.is_whitespace()
}

/// Folds a search query.
pub fn fold(s: &str) -> String {
    fold_skipping(s, &[]).0
}

/// Folded text and, when `want_map`, the source byte for each folded byte.
fn fold_inner(text: &str, skip: &[ByteSpan], want_map: bool) -> (String, Vec<u32>) {
    let mut out = String::with_capacity(text.len());
    let mut map = Vec::new();
    let mut pending_space: Option<u32> = None;
    let mut skip_i = 0;
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        while skip_i < skip.len() && skip[skip_i].end as usize <= i {
            skip_i += 1;
        }
        if skip_i < skip.len() && skip[skip_i].start as usize <= i {
            continue;
        }
        if go_space(c) {
            if !out.is_empty() && pending_space.is_none() {
                pending_space = Some(i as u32);
            }
            continue;
        }
        if let Some(sp) = pending_space.take() {
            out.push(' ');
            if want_map {
                map.push(sp);
            }
        }
        let l = go_lower(c);
        out.push(l);
        if want_map {
            map.extend(std::iter::repeat_n(i as u32, l.len_utf8()));
        }
    }
    (out, map)
}

pub fn fold_skipping(text: &str, skip: &[ByteSpan]) -> (String, Vec<u32>) {
    fold_inner(text, skip, false)
}

/// Non-overlapping matches of folded `q` in `text`, as spans of `text`, leftmost first.
pub fn find_spans(text: &str, skip: &[ByteSpan], q: &str) -> Vec<ByteSpan> {
    if q.is_empty() {
        return Vec::new();
    }
    let (folded, map) = fold_inner(text, skip, true);
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = folded[from..].find(q) {
        let start = from + at;
        let end = start + q.len();
        let src_start = map[start];
        // The source end is the end of the character holding the match's last folded byte.
        let last = map[end - 1] as usize;
        let src_end = last + text[last..].chars().next().map_or(0, char::len_utf8);
        out.push(ByteSpan::new(src_start, src_end as u32));
        from = end;
    }
    out
}

/// Counts matches like Go's `strings.Count`.
pub fn count(folded: &str, q: &str) -> u32 {
    if q.is_empty() {
        return 0;
    }
    folded.matches(q).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_case_and_whitespace_like_go() {
        assert_eq!(fold("  Hello\t\n  WORLD "), "hello world");
        assert_eq!(fold("A\u{a0}B\u{2003}C\u{85}D\u{3000}E"), "a b c d e");
        // Dotted capital I folds to plain i, not i plus a combining dot.
        assert_eq!(fold("\u{130}STANBUL"), "istanbul");
        // Final sigma: Go folds rune by rune, so Σ is always σ.
        assert_eq!(fold("ΣΑΣ"), "σασ");
        // Letters with no simple single lower-case form stay as they are in Go.
        assert_eq!(fold("ß"), "ß");
    }

    #[test]
    fn spans_map_back_through_folding() {
        let t = "One  \u{130}Stanbul two";
        let q = fold("istanbul two");
        let m = find_spans(t, &[], &q);
        assert_eq!(m.len(), 1);
        assert_eq!(&t[m[0].range()], "\u{130}Stanbul two");
    }

    #[test]
    fn non_bmp_ranges_keep_whole_characters() {
        let t = "a😀b 😀";
        let m = find_spans(t, &[], "😀");
        assert_eq!(m.len(), 2);
        assert_eq!(&t[m[0].range()], "😀");
        assert_eq!(&t[m[1].range()], "😀");
    }

    #[test]
    fn matching_is_non_overlapping_like_go() {
        assert_eq!(find_spans("aaaa", &[], "aa").len(), 2);
        assert_eq!(count("aaaa", "aa"), 2);
        assert_eq!(count("anything", ""), 0);
    }

    #[test]
    fn skipped_text_is_invisible_to_find() {
        let t = "foo$x$bar x";
        let skip = [ByteSpan::new(3, 6)];
        assert_eq!(fold_skipping(t, &skip).0, "foobar x");
        let m = find_spans(t, &skip, "foobar");
        assert_eq!(&t[m[0].range()], "foo$x$bar");
        assert_eq!(find_spans(t, &skip, "x").len(), 1);
    }

    /// Whole-Unicode comparison with a table dumped from Go. Run with
    /// `MDV_GO_FOLD=<file> cargo test -p mdv-reader go_table -- --ignored`.
    #[test]
    #[ignore]
    fn go_table() {
        let path = std::env::var("MDV_GO_FOLD").expect("MDV_GO_FOLD");
        let mut lower = std::collections::HashMap::new();
        let mut space = std::collections::HashSet::new();
        for line in std::fs::read_to_string(path).unwrap().lines() {
            let f: Vec<&str> = line.split(' ').collect();
            let cp = u32::from_str_radix(f[1], 16).unwrap();
            if f[0] == "L" {
                lower.insert(cp, u32::from_str_radix(f[2], 16).unwrap());
            } else {
                space.insert(cp);
            }
        }
        let mut diffs = Vec::new();
        for cp in 0..=0x10ffffu32 {
            let Some(c) = char::from_u32(cp) else { continue };
            let want = lower.get(&cp).copied().unwrap_or(cp);
            if go_lower(c) as u32 != want {
                diffs.push(format!("{cp:x}: go {want:x} rust {:x}", go_lower(c) as u32));
            }
            assert_eq!(go_space(c), space.contains(&cp), "space {cp:x}");
        }
        eprintln!("lower-case differences from Go: {}", diffs.len());
        eprintln!("{}", diffs.join("\n"));
    }
}
