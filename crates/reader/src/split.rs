//! Cuts a large document into sections at safe block boundaries. Port of `internal/doc/split.go`.

/// Splits a section at the next blank line even when the usual safety rules say no, so one
/// section never grows unbounded.
const FORCE_SPLIT_BYTES: usize = 1 << 20;
/// Splits at any line, for example inside a fence that is never closed.
const PANIC_SPLIT_BYTES: usize = 4 << 20;

/// Cuts `src` into `[start, end)` ranges of at least `target` bytes. A cut is only made before a
/// line that certainly begins a new top-level block: it follows a blank line, starts in column 0,
/// is not a list item or block quote, and is not inside a fenced code block, maths block or raw
/// HTML block.
pub fn split_sections(src: &str, target: usize) -> Vec<(usize, usize)> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut fence: Option<&[u8]> = None;
    let mut html_end: Option<&'static str> = None;
    let mut in_math = false;
    let mut depth = 0i32;
    let mut prev_blank = true;

    let mut pos = 0usize;
    while pos < bytes.len() {
        let (line_end, next) = match memchr::memchr(b'\n', &bytes[pos..]) {
            Some(i) => (pos + i, pos + i + 1),
            None => (bytes.len(), bytes.len()),
        };
        let line = &src[pos..line_end];
        let trimmed = line.trim_start_matches([' ', '\t']);
        let blank = trimmed.trim().is_empty();
        let size = pos - start;

        if fence.is_some() || html_end.is_some() || in_math {
            if size >= PANIC_SPLIT_BYTES {
                out.push((start, pos));
                start = pos;
                depth = 0;
                fence = None;
                html_end = None;
                in_math = false;
            } else if let Some(f) = fence {
                if closes_fence(trimmed.as_bytes(), f) {
                    fence = None;
                }
            } else if let Some(end) = html_end {
                if contains_ignore_ascii_case(line, end) {
                    html_end = None;
                }
            } else if line.trim().ends_with("$$") {
                in_math = false;
            }
        } else {
            if prev_blank && !blank && size >= target && ((depth == 0 && safe_start(line.as_bytes())) || size >= FORCE_SPLIT_BYTES) {
                out.push((start, pos));
                start = pos;
                depth = 0;
            }
            if let Some(f) = open_fence(trimmed.as_bytes()) {
                fence = Some(f);
            } else if let Some(end) = html_block_end(trimmed.as_bytes()).filter(|e| !contains_ignore_ascii_case(tail(trimmed, 4), e)) {
                html_end = Some(end);
            } else {
                let t = line.trim();
                if t.starts_with("$$") && (t.len() == 2 || !t[2..].ends_with("$$")) {
                    in_math = true;
                }
            }
            if trimmed.as_bytes().first() == Some(&b'<') {
                depth = (depth + html_depth_delta(line)).max(0);
            }
        }
        prev_blank = blank;
        pos = next;
    }
    if start < bytes.len() || out.is_empty() {
        out.push((start, bytes.len()));
    }
    out
}

fn tail(s: &str, n: usize) -> &str {
    let mut i = n.min(s.len());
    while !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}

fn contains_ignore_ascii_case(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    h.len() >= n.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// Reports whether a line in column 0 begins a fresh block rather than continuing a list or quote.
pub fn safe_start(line: &[u8]) -> bool {
    match line.first() {
        None => return false,
        Some(b' ' | b'\t' | b'>') => return false,
        Some(b'-' | b'*' | b'+') => return line.len() > 1 && line[1] != b' ' && line[1] != b'\t',
        _ => {}
    }
    let mut i = 0;
    while i < line.len() && i < 10 && line[i].is_ascii_digit() {
        i += 1;
    }
    !(i > 0 && i < line.len() && (line[i] == b'.' || line[i] == b')'))
}

fn open_fence(t: &[u8]) -> Option<&[u8]> {
    if t.len() < 3 || (t[0] != b'`' && t[0] != b'~') {
        return None;
    }
    let n = t.iter().take_while(|&&c| c == t[0]).count();
    if n < 3 || (t[0] == b'`' && t[n..].contains(&b'`')) {
        return None;
    }
    Some(&t[..n])
}

fn closes_fence(t: &[u8], fence: &[u8]) -> bool {
    let n = t.iter().take_while(|&&c| c == fence[0]).count();
    n >= fence.len() && std::str::from_utf8(&t[n..]).is_ok_and(|s| s.trim().is_empty())
}

const HTML_BLOCKS: [(&str, &str); 5] = [("<pre", "</pre>"), ("<script", "</script>"), ("<style", "</style>"), ("<textarea", "</textarea>"), ("<!--", "-->")];

fn html_block_end(t: &[u8]) -> Option<&'static str> {
    if t.len() < 4 || t[0] != b'<' {
        return None;
    }
    HTML_BLOCKS
        .iter()
        .find(|(open, _)| t.len() >= open.len() && t[..open.len()].eq_ignore_ascii_case(open.as_bytes()))
        .map(|(_, end)| *end)
}

const CONTAINERS: [&str; 6] = ["details", "div", "table", "blockquote", "section", "center"];

/// Counts container tags opened minus closed, so the chunker avoids splitting `<details>` in two.
pub fn html_depth_delta(text: &str) -> i32 {
    let b = text.as_bytes();
    let mut delta = 0;
    let mut i = 0;
    while let Some(off) = memchr::memchr(b'<', &b[i..]) {
        let at = i + off + 1;
        i = at;
        let closing = b.get(at) == Some(&b'/');
        let name_at = at + closing as usize;
        let Some(name) = CONTAINERS.iter().find(|n| b.len() >= name_at + n.len() && b[name_at..name_at + n.len()].eq_ignore_ascii_case(n.as_bytes())) else {
            continue;
        };
        let after = name_at + name.len();
        if closing {
            let mut j = after;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if b.get(j) == Some(&b'>') {
                delta -= 1;
            }
        } else {
            let word_end = b.get(after).is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_'));
            if word_end && memchr::memchr(b'>', &b[after..]).is_some() {
                delta += 1;
            }
        }
    }
    delta
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    /// `TestSplitSections` from `internal/doc/doc_test.go`.
    #[test]
    fn test_split_sections() {
        let mut b = String::new();
        for i in 0..200 {
            write!(b, "# H{i}\n\n- list item\n\n- second item after blank\n\n```\n# not a heading\n\nstill code\n```\n\n<!--\n\ncomment\n\n-->\n\nPara {i}.\n\n").unwrap();
        }
        let secs = split_sections(&b, 1024);
        assert!(secs.len() >= 10, "expected many sections, got {}", secs.len());
        let mut prev = 0;
        for &(s, e) in &secs {
            assert_eq!(s, prev, "gap at {s}");
            prev = e;
            let rest = &b[s..];
            assert!(
                !(rest.starts_with("- ") || rest.starts_with("still") || rest.starts_with("comment") || rest.starts_with("-->")),
                "unsafe cut before {:?}",
                &rest[..20.min(rest.len())]
            );
        }
        assert_eq!(prev, b.len());
    }

    #[test]
    fn safe_start_rejects_list_items_and_quotes() {
        for bad in ["- item", "* item", "+ item", "1. one", "12) two", "> quote", "  indented", "\tcode", ""] {
            assert!(!safe_start(bad.as_bytes()), "{bad:?}");
        }
        for good in ["# heading", "Para", "-not a list", "**bold**", "1.5 is a number? no", "<div>"] {
            let want = good != "1.5 is a number? no";
            assert_eq!(safe_start(good.as_bytes()), want, "{good:?}");
        }
    }

    #[test]
    fn open_details_is_not_cut() {
        let mut b = String::from("<details>\n<summary>More</summary>\n\n");
        for _ in 0..1000 {
            b.push_str("Line of text inside details.\n\n");
        }
        b.push_str("</details>\n\nAfter.\n");
        let secs = split_sections(&b, 1024);
        assert!(b[secs[0].0..secs[0].1].contains("</details>"), "details split");
    }

    #[test]
    fn unclosed_fence_is_forced_apart_at_four_mb() {
        let mut b = String::from("```\n");
        while b.len() < (9 << 20) {
            b.push_str("a line inside a fence that never closes\n");
        }
        let secs = split_sections(&b, 16 * 1024);
        assert!(secs.len() >= 2 && secs[0].1 - secs[0].0 >= PANIC_SPLIT_BYTES);
        assert_eq!(secs.last().unwrap().1, b.len());
    }

    #[test]
    fn forced_cut_at_one_mb_needs_a_blank_line() {
        let mut b = String::from("<div>\n\n");
        while b.len() < (3 << 20) {
            b.push_str("Text inside an open div.\n\n");
        }
        let secs = split_sections(&b, 16 * 1024);
        assert!(secs.len() >= 2);
        assert!(secs[0].1 - secs[0].0 >= FORCE_SPLIT_BYTES);
    }

    #[test]
    fn depth_counts_openers_and_closers() {
        assert_eq!(html_depth_delta("<details><summary>x</summary>"), 1);
        assert_eq!(html_depth_delta("</details>"), -1);
        assert_eq!(html_depth_delta("<div><div></div>"), 1);
        assert_eq!(html_depth_delta("<divider>"), 0);
        assert_eq!(html_depth_delta("<DIV class=x>"), 1);
    }
}
