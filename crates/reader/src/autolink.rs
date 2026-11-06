//! GFM extended autolinks (`www.`, `http://`, `https://` and bare email addresses). The parser
//! has no such extension, so these are found in text runs after parsing.

/// One found link: its bytes within the searched text and the target it points at.
#[derive(Debug, PartialEq, Eq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

fn boundary_before(b: &[u8], at: usize) -> bool {
    at == 0 || matches!(b[at - 1], b' ' | b'\t' | b'\n' | b'\r' | b'*' | b'_' | b'~' | b'(') || (b[at - 1] >= 0x80 && !prev_char_alnum(b, at))
}

fn prev_char_alnum(b: &[u8], at: usize) -> bool {
    let mut i = at - 1;
    while i > 0 && b[i] & 0xC0 == 0x80 {
        i -= 1;
    }
    std::str::from_utf8(&b[i..at]).ok().and_then(|s| s.chars().next()).is_some_and(char::is_alphanumeric)
}

/// Length of a valid domain at the start of `s`: segments of alphanumerics, `_` and `-`, separated
/// by dots, at least one dot, with no underscore in the last two segments.
fn domain_len(s: &[u8]) -> Option<usize> {
    let mut i = 0;
    let mut dots = 0;
    let mut under = [false; 2]; // the last two segments
    let mut cur_under = false;
    let mut seg_len = 0;
    while i < s.len() {
        let c = s[i];
        if c == b'.' {
            if seg_len == 0 {
                break;
            }
            under = [under[1], cur_under];
            cur_under = false;
            seg_len = 0;
            dots += 1;
        } else if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c >= 0x80 {
            if c == b'_' {
                cur_under = true;
            }
            seg_len += 1;
        } else {
            break;
        }
        i += 1;
    }
    if seg_len == 0 && i > 0 && s[i - 1] == b'.' {
        i -= 1;
        dots -= 1;
        cur_under = under[1];
        under[1] = under[0];
    }
    if dots < 1 || under[1] || cur_under {
        return None;
    }
    Some(i)
}

/// Trims the end of a found address per the GFM rules, returning its new end.
fn trim_tail(t: &[u8], mut end: usize, start: usize) -> usize {
    loop {
        if end <= start {
            return end;
        }
        match t[end - 1] {
            b'?' | b'!' | b'.' | b',' | b':' | b'*' | b'_' | b'~' | b'\'' | b'"' => end -= 1,
            b')' => {
                let s = &t[start..end];
                let open = s.iter().filter(|&&c| c == b'(').count();
                let close = s.iter().filter(|&&c| c == b')').count();
                if close > open {
                    end -= 1;
                } else {
                    return end;
                }
            }
            b';' => {
                let mut j = end - 1;
                while j > start && t[j - 1].is_ascii_alphanumeric() {
                    j -= 1;
                }
                if j > start && t[j - 1] == b'&' && j < end - 1 {
                    end = j - 1;
                } else {
                    return end;
                }
            }
            _ => return end,
        }
    }
}

/// Finds every extended autolink in a run of text.
pub fn find(text: &str) -> Vec<Found> {
    let t = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    // A quick filter: nothing can match without one of these.
    if !(memchr::memchr3(b':', b'.', b'@', t).is_some()) {
        return out;
    }
    while i < t.len() {
        let rest = &t[i..];
        let lower_starts = |p: &[u8]| rest.len() >= p.len() && rest[..p.len()].eq_ignore_ascii_case(p);
        let prefix = if lower_starts(b"https://") {
            Some((8, false))
        } else if lower_starts(b"http://") {
            Some((7, false))
        } else if lower_starts(b"www.") {
            Some((0, true))
        } else {
            None
        };
        if let Some((skip, www)) = prefix.filter(|_| boundary_before(t, i)) {
            if let Some(d) = domain_len(&t[i + skip..]) {
                let mut end = i + skip + d;
                while end < t.len() && !t[end].is_ascii_whitespace() && t[end] != b'<' {
                    end += 1;
                }
                let end = trim_tail(t, end, i);
                if end > i + skip && text.is_char_boundary(end) {
                    let s = &text[i..end];
                    out.push(Found { start: i, end, url: if www { format!("http://{s}") } else { s.to_string() } });
                    i = end;
                    continue;
                }
            }
        }
        if t[i] == b'@' && i > 0 {
            if let Some(f) = email_at(text, i, out.last().map_or(0, |f: &Found| f.end)) {
                i = f.end;
                out.push(f);
                continue;
            }
        }
        i += 1;
    }
    out
}

fn email_at(text: &str, at: usize, floor: usize) -> Option<Found> {
    let t = text.as_bytes();
    let mut start = at;
    while start > floor && (t[start - 1].is_ascii_alphanumeric() || matches!(t[start - 1], b'.' | b'-' | b'_' | b'+')) {
        start -= 1;
    }
    if start == at {
        return None;
    }
    let mut end = at + 1;
    let mut dot = false;
    while end < t.len() && (t[end].is_ascii_alphanumeric() || matches!(t[end], b'.' | b'-' | b'_')) {
        dot |= t[end] == b'.';
        end += 1;
    }
    while end > at + 1 && t[end - 1] == b'.' {
        end -= 1;
    }
    if !dot || !t[at + 1..end].contains(&b'.') || matches!(t[end - 1], b'-' | b'_') {
        return None;
    }
    let s = &text[start..end];
    Some(Found { start, end, url: format!("mailto:{s}") })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(s: &str) -> Vec<(String, String)> {
        find(s).into_iter().map(|f| (s[f.start..f.end].to_string(), f.url)).collect()
    }
    fn one(s: &str, text: &str, url: &str) {
        assert_eq!(urls(s), vec![(text.to_string(), url.to_string())], "{s}");
    }

    /// The Autolinks (extension) section of the GFM spec.
    #[test]
    fn gfm_spec_examples() {
        one("www.commonmark.org", "www.commonmark.org", "http://www.commonmark.org");
        one("Visit www.commonmark.org/help for more information.", "www.commonmark.org/help", "http://www.commonmark.org/help");
        one("Visit www.commonmark.org.", "www.commonmark.org", "http://www.commonmark.org");
        one("Visit www.commonmark.org/a.b.", "www.commonmark.org/a.b", "http://www.commonmark.org/a.b");
        one("www.google.com/search?q=Markup+(business)", "www.google.com/search?q=Markup+(business)", "http://www.google.com/search?q=Markup+(business)");
        one("(www.google.com/search?q=Markup+(business))", "www.google.com/search?q=Markup+(business)", "http://www.google.com/search?q=Markup+(business)");
        one("www.google.com/search?q=(business))+ok", "www.google.com/search?q=(business))+ok", "http://www.google.com/search?q=(business))+ok");
        one("www.google.com/search?q=commonmark&hl=en", "www.google.com/search?q=commonmark&hl=en", "http://www.google.com/search?q=commonmark&hl=en");
        one("www.google.com/search?q=commonmark&hl;", "www.google.com/search?q=commonmark", "http://www.google.com/search?q=commonmark");
        one("www.commonmark.org/he<lp", "www.commonmark.org/he", "http://www.commonmark.org/he");
        one("http://commonmark.org", "http://commonmark.org", "http://commonmark.org");
        one("(Visit https://encrypted.google.com/search?q=Markup+(business))", "https://encrypted.google.com/search?q=Markup+(business)", "https://encrypted.google.com/search?q=Markup+(business)");
        one("foo@bar.baz", "foo@bar.baz", "mailto:foo@bar.baz");
        one("hello@mail+xyz.example isn't valid, but hello+xyz@mail.example is.", "hello+xyz@mail.example", "mailto:hello+xyz@mail.example");
        one("a.b-c_d@a.b.", "a.b-c_d@a.b", "mailto:a.b-c_d@a.b");
        assert!(find("a.b-c_d@a.b-").is_empty());
        assert!(find("a.b-c_d@a.b_").is_empty());
    }

    #[test]
    fn not_links() {
        assert!(find("www.").is_empty());
        assert!(find("xhttp://example.com").is_empty());
        assert!(find("www.example_bad.com is fine? no www.bad_.example_.com").len() <= 1);
        assert!(find("plain text").is_empty());
    }

    #[test]
    fn utf8_around_links() {
        one("kia ora, https://example.com — ka pai", "https://example.com", "https://example.com");
    }
}
