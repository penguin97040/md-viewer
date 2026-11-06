//! Cuts a large document into sections at safe block boundaries. Port of `internal/doc/split.go`.

/// Splits a section at the next blank line even when the usual safety rules say no, so one
/// section never grows unbounded.
const FORCE_SPLIT_BYTES: usize = 1 << 20;
/// Splits at any line where nothing better came, for example in a raw HTML block never closed.
const PANIC_SPLIT_BYTES: usize = 1 << 20;
/// The most text laid out in one go when nothing in the source offers a place to cut: a section is
/// measured and shaped whole, so this bounds both the memory and the stall of bringing it on screen.
pub const MAX_TEXT_BYTES: usize = 128 << 10;
/// The same where each part may be slow to shape or may take one row on screen.
const MAX_DENSE_BYTES: usize = 16 << 10;
/// A fenced code block or a table is cut later than prose, because each part has its own copy
/// button or sort. They are still measured whole, so they cannot go without a limit.
pub const MAX_BLOCK_BYTES: usize = 512 << 10;
/// A table's cells cost far more memory than their text, so it is also cut by rows.
const MAX_TABLE_ROWS: usize = 8 << 10;
/// A line above this length is not repeated in front of the parts of its block.
const MAX_PREFIX_BYTES: usize = 16 << 10;
/// How far back from a forced cut to look for a word boundary.
const CUT_SEARCH_BYTES: usize = 4 << 10;

/// What is parsed in front of a section so that it means what it meant in the whole document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefix {
    None,
    /// The opening line of the fenced code block this section is inside (a source range).
    Fence(u32, u32),
    /// The header and delimiter lines of the table this section is inside (a source range).
    /// The parser drops the header again: only the first part shows it.
    Table(u32, u32),
}

/// One cut of the source. `joined` means no block boundary was available: this section carries on
/// from the one before, and they are drawn with no gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub start: usize,
    pub end: usize,
    pub joined: bool,
    pub prefix: Prefix,
}

/// Cuts `src` into `[start, end)` ranges of at least `target` bytes.
pub fn split_sections(src: &str, target: usize) -> Vec<(usize, usize)> {
    sections(src, target).into_iter().map(|s| (s.start, s.end)).collect()
}

/// Cuts `src` into sections of at least `target` bytes where it can. A clean cut is only made
/// before a line that certainly begins a new top-level block: it follows a blank line, starts in
/// column 0, is not a list item or block quote, and is not inside a fenced code block, maths block
/// or raw HTML block. Where no such line comes, text is cut anyway at the limits above, because a
/// hostile file can be 25 MB of one paragraph, one line or one code block.
pub fn sections(src: &str, target: usize) -> Vec<Section> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut joined = false;
    let mut prefix = Prefix::None;
    let mut fence: Option<&[u8]> = None;
    let mut fence_line = (0usize, 0usize);
    let mut html_end: Option<&'static str> = None;
    let mut in_math = false;
    let mut depth = 0i32;
    let mut prev_blank = true;
    // The header and delimiter of the table the walk is inside.
    let mut table: Option<(usize, usize)> = None;
    let mut prev_line = (0usize, 0usize);
    let mut rows = 0usize;

    let mut pos = 0usize;
    while pos < bytes.len() {
        let (line_end, next) = match memchr::memchr(b'\n', &bytes[pos..]) {
            Some(i) => (pos + i, pos + i + 1),
            None => (bytes.len(), bytes.len()),
        };
        // A line too long to lay out is cut, whatever it is part of. A table does not survive it:
        // its cells cannot be matched up again across a cut.
        if too_long(src, pos, line_end) {
            table = None;
            if pos - start >= part_bytes(src, pos, fence.is_some()) {
                out.push(Section { start, end: pos, joined, prefix });
                start = pos;
                joined = true;
                prefix = fence.map_or(Prefix::None, |_| fence_prefix(fence_line));
            }
            loop {
                let part = part_bytes(src, start.max(pos), fence.is_some());
                if line_end - start <= part {
                    break;
                }
                let cut = forced_cut(src, start + part, pos, line_end);
                out.push(Section { start, end: cut, joined, prefix });
                start = cut;
                joined = true;
                prefix = fence.map_or(Prefix::None, |_| fence_prefix(fence_line));
            }
            pos = pos.max(start);
        }
        let line = &src[pos..line_end];
        let trimmed = line.trim_start_matches([' ', '\t']);
        let blank = trimmed.trim().is_empty();
        let size = pos - start;

        if fence.is_some() || html_end.is_some() || in_math {
            let closing = fence.is_some_and(|f| closes_fence(trimmed.as_bytes(), f));
            if let (true, false, true) = (fence.is_some(), closing, size >= MAX_BLOCK_BYTES) {
                // Still inside the fence: the next part opens it again.
                out.push(Section { start, end: pos, joined, prefix });
                start = pos;
                joined = true;
                prefix = fence_prefix(fence_line);
            } else if fence.is_none() && size >= PANIC_SPLIT_BYTES {
                out.push(Section { start, end: pos, joined, prefix });
                start = pos;
                joined = false;
                prefix = Prefix::None;
                depth = 0;
                html_end = None;
                in_math = false;
            } else if closing {
                fence = None;
            } else if let Some(end) = html_end {
                if contains_ignore_ascii_case(line, end) {
                    html_end = None;
                }
            } else if in_math && line.trim().ends_with("$$") {
                in_math = false;
            }
        } else {
            if prev_blank && !blank && size >= target && ((depth == 0 && safe_start(line.as_bytes())) || size >= FORCE_SPLIT_BYTES) {
                out.push(Section { start, end: pos, joined, prefix });
                start = pos;
                joined = false;
                prefix = Prefix::None;
                depth = 0;
                table = None;
            } else if !blank && !prev_blank && (rows >= MAX_TABLE_ROWS || soft_cut_due(size, depth, table.is_some(), line)) {
                // A paragraph, list, quote or table with no blank line in it for far too long.
                out.push(Section { start, end: pos, joined, prefix });
                start = pos;
                joined = true;
                prefix = table.map_or(Prefix::None, |(a, b)| Prefix::Table(a as u32, b as u32));
                rows = 0;
            }
            if table.is_some() {
                rows += 1;
            }
            if blank {
                table = None;
                rows = 0;
            } else if table.is_none() && next - prev_line.0 <= MAX_PREFIX_BYTES && table_header(&src[prev_line.0..prev_line.1], trimmed) {
                table = Some((prev_line.0, next));
            }
            if let Some(f) = open_fence(trimmed.as_bytes()) {
                fence = Some(f);
                fence_line = (pos, next);
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
        prev_line = (pos, line_end);
        pos = next;
    }
    if start < bytes.len() || out.is_empty() {
        out.push(Section { start, end: bytes.len(), joined, prefix });
    }
    out
}

fn fence_prefix((a, b): (usize, usize)) -> Prefix {
    if b - a <= MAX_PREFIX_BYTES {
        Prefix::Fence(a as u32, b as u32)
    } else {
        Prefix::None
    }
}

/// Whether a run of text with no blank line has gone on too long, and this line can start a part.
/// A line starting with white space is left alone, as on its own it could read as indented code.
fn soft_cut_due(size: usize, depth: i32, table: bool, line: &str) -> bool {
    let limit = if depth > 0 {
        PANIC_SPLIT_BYTES
    } else if table {
        MAX_BLOCK_BYTES
    } else {
        MAX_TEXT_BYTES
    };
    size >= limit && (size >= PANIC_SPLIT_BYTES || !line.starts_with([' ', '\t']))
}

/// Whether two lines open a table: the second is all dashes and pipes, with as many columns.
fn table_header(head: &str, delimiter: &str) -> bool {
    fn columns(t: &str) -> usize {
        let t = t.trim();
        let t = t.strip_prefix('|').unwrap_or(t);
        let t = t.strip_suffix('|').unwrap_or(t);
        t.split('|').count()
    }
    let d = delimiter.trim_end();
    d.contains('|') && d.contains('-') && d.bytes().all(|c| matches!(c, b'|' | b'-' | b':' | b' ' | b'\t')) && columns(head) == columns(d)
}

/// Whether most of the text from here is not ASCII. Such text often comes from a fallback font,
/// where shaping takes ten times as long, so less of it can be laid out in one go.
fn dense(src: &str, at: usize) -> bool {
    let sample = &src.as_bytes()[at..(at + 1024).min(src.len())];
    sample.iter().filter(|b| **b >= 0x80).count() * 2 > sample.len()
}

fn too_long(src: &str, start: usize, end: usize) -> bool {
    let len = end - start;
    len > MAX_TEXT_BYTES || (len > MAX_DENSE_BYTES && dense(src, start))
}

/// How much of a long line goes in one part. In a code block with wrap off each part is one row on
/// screen, so a screen can show dozens of them.
fn part_bytes(src: &str, at: usize, code: bool) -> usize {
    if code || dense(src, at) {
        MAX_DENSE_BYTES
    } else {
        MAX_TEXT_BYTES
    }
}

/// Whether a file has lines that have to be cut, whatever its size.
pub fn has_long_lines(src: &str) -> bool {
    let b = src.as_bytes();
    let mut pos = 0;
    while pos < b.len() {
        let end = memchr::memchr(b'\n', &b[pos..]).map_or(b.len(), |i| pos + i);
        if too_long(src, pos, end) {
            return true;
        }
        pos = end + 1;
    }
    false
}

/// A place at or near `mark`, within a line ending at `end`, to cut a line that has to be cut.
/// Prefers a word boundary, and text that cannot read as the start of a block when parsed alone.
fn forced_cut(src: &str, mark: usize, line_start: usize, end: usize) -> usize {
    let b = src.as_bytes();
    let starts_plain = |i: usize| b[i].is_ascii_alphabetic() || (b[i] >= 0x80 && src.is_char_boundary(i));
    let low = mark.saturating_sub(CUT_SEARCH_BYTES).max(line_start);
    if let Some(i) = (low..mark).rev().find(|&i| b[i] == b' ' && starts_plain(i + 1)) {
        return i + 1;
    }
    let high = (mark + CUT_SEARCH_BYTES).min(end);
    let boundary = |i: usize| unicode_segmentation::GraphemeCursor::new(i, src.len(), true).is_boundary(src, 0).unwrap_or(false);
    if let Some(i) = (mark..high).find(|&i| starts_plain(i) && boundary(i)) {
        return i;
    }
    // Nothing plain nearby: any character boundary will do, even inside a grapheme of a thousand
    // combining marks, because the limit matters more.
    let mut i = mark.min(end);
    while !src.is_char_boundary(i) {
        i += 1;
    }
    i
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
    fn long_fence_reopens_in_each_part() {
        let mut b = String::from("```rust\n");
        while b.len() < (3 << 20) {
            b.push_str("a line inside a fence that never closes\n");
        }
        let secs = sections(&b, 16 * 1024);
        assert!(secs.len() >= 5, "{}", secs.len());
        assert_eq!(secs[0].prefix, Prefix::None);
        for s in &secs[1..] {
            assert!(s.joined);
            assert_eq!(s.prefix, Prefix::Fence(0, 8));
            assert!(s.end - s.start <= MAX_BLOCK_BYTES + 64);
        }
        assert_eq!(secs.last().unwrap().end, b.len());
    }

    #[test]
    fn a_fence_that_closes_is_not_carried_on() {
        let mut b = String::from("```\n");
        while b.len() < (1 << 20) {
            b.push_str("code\n");
        }
        b.push_str("```\n\nAfter the block.\n");
        for _ in 0..4000 {
            b.push_str("\nAnother paragraph after it, with blank lines around it.\n");
        }
        let secs = sections(&b, 16 * 1024);
        let last = secs.last().unwrap();
        assert!(!last.joined);
        assert_eq!(last.prefix, Prefix::None);
    }

    fn check_whole(src: &str, secs: &[Section], limit: usize) {
        let mut prev = 0;
        for s in secs {
            assert_eq!(s.start, prev);
            assert!(s.end - s.start <= limit, "{} bytes", s.end - s.start);
            assert!(src.is_char_boundary(s.start) && src.is_char_boundary(s.end));
            prev = s.end;
        }
        assert_eq!(prev, src.len());
    }

    #[test]
    fn one_long_line_is_cut_and_stays_whole() {
        let limit = MAX_TEXT_BYTES + CUT_SEARCH_BYTES + 8;
        for src in ["word ".repeat(1 << 18), "x".repeat(1 << 21), "\u{6f22}".repeat(1 << 19), "*_".repeat(1 << 20), format!("a{}", "\u{301}".repeat(1 << 20))] {
            let secs = sections(&src, 16 * 1024);
            assert!(secs.len() > 4, "{}", secs.len());
            check_whole(&src, &secs, limit);
            assert!(!secs[0].joined);
            assert!(secs[1..].iter().all(|s| s.joined && s.prefix == Prefix::None));
        }
    }

    #[test]
    fn forced_cuts_start_with_plain_text_at_a_word() {
        let src = "- list marker words 1. two\n".replace('\n', " ").repeat(1 << 15);
        for s in &sections(&src, 16 * 1024)[1..] {
            assert_eq!(src.as_bytes()[s.start - 1], b' ');
            assert!(src.as_bytes()[s.start].is_ascii_alphabetic());
        }
    }

    #[test]
    fn a_paragraph_of_short_lines_is_cut_at_a_line() {
        let src = "word\n".repeat(1 << 18);
        let secs = sections(&src, 16 * 1024);
        assert!(secs.len() > 4);
        check_whole(&src, &secs, MAX_TEXT_BYTES + 8);
        assert!(secs[1..].iter().all(|s| s.joined && src.as_bytes()[s.start - 1] == b'\n'));
    }

    #[test]
    fn a_long_table_repeats_its_header_lines() {
        let src = format!("Intro\n\n| a | b |\n|---|---|\n{}", "| 1 | 2 |\n".repeat(1 << 18));
        let secs = sections(&src, 16 * 1024);
        assert!(secs.len() >= 4);
        assert_eq!(secs[0].prefix, Prefix::None);
        for s in &secs[1..] {
            assert!(s.joined);
            let Prefix::Table(a, b) = s.prefix else { panic!("{:?}", s.prefix) };
            assert_eq!(&src[a as usize..b as usize], "| a | b |\n|---|---|\n");
        }
    }

    #[test]
    fn long_lines_of_unspaced_text_and_code_go_in_small_parts() {
        let src = "\u{6f22}".repeat(1 << 16);
        let secs = sections(&src, 16 * 1024);
        check_whole(&src, &secs, MAX_DENSE_BYTES + CUT_SEARCH_BYTES + 8);
        let src = format!("```\n{}\n```\n", "x = 1; ".repeat(1 << 16));
        let secs = sections(&src, 16 * 1024);
        check_whole(&src, &secs, MAX_DENSE_BYTES + CUT_SEARCH_BYTES + 8);
        assert!(secs[1..].iter().all(|s| s.prefix == Prefix::Fence(0, 4)));
    }

    #[test]
    fn only_long_lines_count_as_long() {
        assert!(!has_long_lines(&"A short line.\n".repeat(1 << 16)));
        assert!(!has_long_lines(&"x".repeat(MAX_TEXT_BYTES)));
        assert!(has_long_lines(&"x".repeat(MAX_TEXT_BYTES + 1)));
        assert!(has_long_lines(&format!("short\n{}", "\u{6f22}".repeat(MAX_DENSE_BYTES))));
    }

    #[test]
    fn normal_text_has_no_forced_cuts() {
        let src = "A paragraph of ordinary length that goes on for a little while.\n\n".repeat(1 << 16);
        assert!(sections(&src, 16 * 1024).iter().all(|s| !s.joined && s.prefix == Prefix::None));
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
        assert!(sections(&b, 16 * 1024).iter().all(|s| !s.joined));
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
