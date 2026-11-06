//! A small tag scanner for raw HTML in markdown. This is not an HTML parser: it recognises tags,
//! comments and text so that the reader can map allowed elements to the document model and show
//! everything else as safe plain text. It never runs anything or fetches anything.
//!
//! The allowlist is v0.2's sanitiser policy (bluemonday's `UGCPolicy` plus its additions).

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tok<'a> {
    Text(&'a str),
    Open { name: String, attrs: Vec<(String, String)>, self_closing: bool },
    Close { name: String },
}

/// Scans raw HTML. An unfinished tag at the end becomes text, so it cannot join up with what
/// follows, and comments, declarations and processing instructions are dropped.
pub fn scan(s: &str) -> Vec<Tok<'_>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut text_from = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        let rest = &s[i..];
        let skip_to = if rest.starts_with("<!--") {
            Some(rest.find("-->").map_or(s.len(), |e| i + e + 3))
        } else if rest.starts_with("<!") || rest.starts_with("<?") {
            Some(rest.find('>').map_or(s.len(), |e| i + e + 1))
        } else {
            None
        };
        if let Some(to) = skip_to {
            if text_from < i {
                out.push(Tok::Text(&s[text_from..i]));
            }
            i = to;
            text_from = i;
            continue;
        }
        let closing = b.get(i + 1) == Some(&b'/');
        let name_at = i + 1 + closing as usize;
        if !b.get(name_at).is_some_and(u8::is_ascii_alphabetic) {
            i += 1;
            continue;
        }
        let Some((tok, end)) = parse_tag(s, name_at, closing) else {
            // Unfinished: leave it as text.
            i += 1;
            continue;
        };
        if text_from < i {
            out.push(Tok::Text(&s[text_from..i]));
        }
        out.push(tok);
        i = end;
        text_from = i;
    }
    if text_from < s.len() {
        out.push(Tok::Text(&s[text_from..]));
    }
    out
}

fn parse_tag(s: &str, name_at: usize, closing: bool) -> Option<(Tok<'_>, usize)> {
    let b = s.as_bytes();
    let mut i = name_at;
    while i < b.len() && !matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'/' | b'>') {
        i += 1;
    }
    let name = s[name_at..i].to_ascii_lowercase();
    let mut attrs = Vec::new();
    let mut self_closing = false;
    loop {
        while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c') {
            i += 1;
        }
        match b.get(i)? {
            b'>' => {
                i += 1;
                break;
            }
            b'/' => {
                self_closing = b.get(i + 1) == Some(&b'>');
                i += 1;
                continue;
            }
            _ => {}
        }
        let start = i;
        while i < b.len() && !matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'/' | b'>' | b'=') {
            i += 1;
        }
        let key = s[start..i].to_ascii_lowercase();
        while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        let mut val = String::new();
        if b.get(i) == Some(&b'=') {
            i += 1;
            while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
                i += 1;
            }
            match b.get(i)? {
                q @ (b'"' | b'\'') => {
                    let close = memchr::memchr(*q, &b[i + 1..])? + i + 1;
                    val = decode_entities(&s[i + 1..close]);
                    i = close + 1;
                }
                _ => {
                    let st = i;
                    while i < b.len() && !matches!(b[i], b' ' | b'\t' | b'\n' | b'\r' | b'>') {
                        i += 1;
                    }
                    val = decode_entities(&s[st..i]);
                }
            }
        }
        if !key.is_empty() {
            attrs.push((key, val));
        }
    }
    Some((if closing { Tok::Close { name } } else { Tok::Open { name, attrs, self_closing } }, i))
}

pub fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// HTML elements whose content is dropped along with their tags.
pub fn skips_content(name: &str) -> bool {
    matches!(name, "script" | "style" | "iframe" | "frame" | "frameset" | "noembed" | "noframes" | "noscript" | "nostyle" | "object" | "title" | "template" | "svg" | "math" | "textarea" | "select" | "video" | "audio" | "canvas" | "applet" | "embed")
}

/// Whether the sanitiser would keep this element. Text of other elements stays, tags go.
pub fn allowed(name: &str) -> bool {
    matches!(
        name,
        "article" | "aside" | "figure" | "figcaption" | "section" | "summary" | "details" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "hgroup" | "blockquote"
            | "br" | "div" | "hr" | "p" | "span" | "wbr" | "a" | "map" | "area" | "abbr" | "acronym" | "cite" | "code" | "dfn" | "em" | "mark" | "q" | "samp" | "s"
            | "strong" | "sub" | "sup" | "time" | "var" | "b" | "i" | "pre" | "small" | "strike" | "tt" | "u" | "bdi" | "bdo" | "rp" | "rt" | "ruby" | "del" | "ins"
            | "dd" | "dl" | "dt" | "li" | "ol" | "ul" | "table" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr" | "caption" | "col" | "colgroup" | "meter" | "progress"
            | "img" | "kbd" | "picture" | "source" | "center" | "input" | "nav" | "main" | "header" | "footer" | "address"
    )
}

/// Decodes character references. Numeric ones and the commonly used names are handled; anything
/// else stays as written. Control characters and invalid code points become U+FFFD, as HTML does.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let semi = tail[1..].find(';').filter(|&n| n <= 32);
        let decoded = semi.and_then(|n| decode_one(&tail[1..1 + n]).map(|c| (c, n + 2)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_one(e: &str) -> Option<char> {
    if let Some(n) = e.strip_prefix('#') {
        let cp = if let Some(h) = n.strip_prefix(['x', 'X']) { u32::from_str_radix(h, 16).ok()? } else { n.parse().ok()? };
        return Some(char::from_u32(cp).filter(|c| *c != '\0' && (!c.is_control() || c.is_whitespace())).unwrap_or('\u{fffd}'));
    }
    Some(match e {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "hellip" => '…',
        "mdash" => '—',
        "ndash" => '–',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "laquo" => '«',
        "raquo" => '»',
        "middot" => '·',
        "bull" => '•',
        "deg" => '°',
        "times" => '×',
        "divide" => '÷',
        "plusmn" => '±',
        "larr" => '←',
        "rarr" => '→',
        "uarr" => '↑',
        "darr" => '↓',
        "check" => '✓',
        "sect" => '§',
        "para" => '¶',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_tags_and_text() {
        let t = scan("a<b class=\"x\" id='y' checked>c</b ><br/><!-- gone -->d");
        assert_eq!(t[0], Tok::Text("a"));
        match &t[1] {
            Tok::Open { name, attrs, self_closing } => {
                assert_eq!(name, "b");
                assert_eq!(attr(attrs, "class"), Some("x"));
                assert_eq!(attr(attrs, "id"), Some("y"));
                assert_eq!(attr(attrs, "checked"), Some(""));
                assert!(!self_closing);
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(t[2], Tok::Text("c"));
        assert_eq!(t[3], Tok::Close { name: "b".into() });
        assert!(matches!(&t[4], Tok::Open { name, self_closing: true, .. } if name == "br"));
        assert_eq!(t[5], Tok::Text("d"));
    }

    #[test]
    fn unfinished_tag_is_text() {
        let t = scan(r#"<p>a</p><div class="code""#);
        assert_eq!(t.last().unwrap(), &Tok::Text(r#"<div class="code""#));
    }

    #[test]
    fn entities_decode() {
        assert_eq!(decode_entities("a&amp;b &#x41;&#66; &nosuch; &"), "a&b AB &nosuch; &");
        assert_eq!(decode_entities("&#0;"), "\u{fffd}");
    }
}
