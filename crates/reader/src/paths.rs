//! Paths built from document text. A document is untrusted, and on Windows reading
//! `\\server\share\...` connects to that server and signs in as the user, handing it a hash of
//! the user's password. Port of `store.Resolve`, `ReachableFrom` and `windowsShare`, plus refusal
//! of the `\??\` namespace form, which the Go check does not cover.
//!
//! The Go reader applies these rules on Windows only. They apply on every platform here: a
//! document that does this is hostile wherever it is opened, and the logic stays testable.

use std::path::{Path, PathBuf};

const DEVICE: &str = r"\\?";

/// `\\server\share` (lower-cased) for a network path, `\\?` for a device path, or empty for a
/// local one. Either slash is accepted, as Windows does.
pub fn windows_share(p: &str) -> String {
    let p = p.replace('/', "\\");
    if p.starts_with(r"\??\") {
        return DEVICE.to_string();
    }
    let Some(rest) = p.strip_prefix(r"\\") else {
        return String::new();
    };
    let mut parts = rest.splitn(3, '\\');
    let (a, b) = (parts.next().unwrap_or(""), parts.next());
    match b {
        Some(b) if !b.is_empty() && !a.is_empty() && a != "?" && a != "." => format!(r"\\{a}\{b}").to_lowercase(),
        _ => DEVICE.to_string(),
    }
}

/// Whether a document at `document` may read path `p`.
pub fn reachable_from(document: &str, p: &str) -> bool {
    let share = windows_share(p);
    share.is_empty() || (share != DEVICE && share == windows_share(document))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = b.get(i + 1..i + 3)?;
            let hi = (hex[0] as char).to_digit(16)?;
            let lo = (hex[1] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Lexical clean: removes `.` and resolves `..`, like Go's `filepath.Clean`.
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn from_slash(s: &str) -> String {
    if cfg!(windows) {
        s.replace('/', "\\")
    } else {
        s.to_string()
    }
}

/// Resolves a link or image address against the document, refusing other computers and devices.
pub fn resolve(document: &Path, href: &str) -> Result<PathBuf, &'static str> {
    let href = href.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode(href).ok_or("the address is not valid")?;
    if decoded.contains('\0') {
        return Err("the address is not valid");
    }
    let doc = document.to_string_lossy();
    // Check the address as written and again once resolved: `a/../\\host\x` must not slip through.
    if !reachable_from(&doc, &decoded) {
        return Err("links to files on other computers are not followed");
    }
    let p = PathBuf::from(from_slash(&decoded));
    let joined = if p.is_absolute() {
        p
    } else {
        document.parent().unwrap_or(Path::new("")).join(p)
    };
    let cleaned = clean(&joined);
    if !reachable_from(&doc, &cleaned.to_string_lossy()) {
        return Err("links to files on other computers are not followed");
    }
    Ok(cleaned)
}

/// Whether a link's file extension opens in the viewer (`MD_EXT` in `app.js`).
pub fn is_markdown(path: &str) -> bool {
    let p = path.split('?').next().unwrap_or("").to_ascii_lowercase();
    [".md", ".markdown", ".mdown", ".mkd", ".mkdn", ".mdx", ".txt"].iter().any(|e| p.ends_with(e))
}

/// Reads an address the way a browser does before deciding where it points: tabs and newlines
/// are ignored, leading spaces and control characters are dropped, and `\` counts as `/`.
fn url_normalise(u: &str) -> String {
    let u: String = u.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).map(|c| if c == '\\' { '/' } else { c }).collect();
    u.trim_start_matches(|c: char| c <= ' ').to_string()
}

/// Port of `isWebURL`: http, https or protocol-relative. Security filters follow the browser's
/// URL reading, not intuition: `https:host/x.png` has no `//` and still goes out.
pub fn is_web_url(u: &str) -> bool {
    let u = url_normalise(u);
    if u.starts_with("//") {
        return true;
    }
    match u.split_once(':') {
        Some((scheme, _)) => scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"),
        None => false,
    }
}

/// The scheme of an address, if it has one (`^[a-z][a-z0-9+.-]*:`).
pub fn scheme(u: &str) -> Option<String> {
    let u = url_normalise(u);
    let (s, _) = u.split_once(':')?;
    let mut ch = s.chars();
    if ch.next()?.is_ascii_alphabetic() && ch.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) {
        Some(s.to_ascii_lowercase())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TestReachable` from `internal/store/store_test.go`, with the document on a share.
    #[test]
    fn reachable_table() {
        let doc = r"\\nas\docs\readme.md";
        for (p, want) in [
            (r"C:\pics\a.png", true),
            (r"\\nas\docs\img\a.png", true),
            (r"\\NAS\Docs\a.png", true),
            (r"\\nas\other\a.png", false),
            (r"\\attacker\share\a.png", false),
            (r"\\?\UNC\nas\docs\a.png", false),
            (r"//attacker/share/x.md", false),
            (r"\\.\GLOBALROOT\x\a.png", false),
            (r"\??\UNC\attacker\share\a.png", false),
            (r"/??/C:/x.png", false),
        ] {
            assert_eq!(reachable_from(doc, p), want, "{p}");
        }
    }

    #[test]
    fn local_document_reaches_no_share() {
        for p in [r"\\attacker\share\a.png", "//attacker/share/a.png", r"\\?\C:\x", r"\\.\PIPE\x", r"\\attacker", r"\\"] {
            assert!(!reachable_from("/home/me/a.md", p), "{p}");
            assert!(resolve(Path::new("/home/me/a.md"), p).is_err(), "{p}");
        }
    }

    #[test]
    fn encoded_and_mixed_network_paths_are_refused() {
        let d = Path::new("/home/me/a.md");
        for p in ["%5C%5Cattacker%5Cshare%5Ca.png", "%2F%2Fattacker%2Fshare%2Fa.png", r"/\attacker\share\a.png", r"\/attacker/share/a.png"] {
            assert!(resolve(d, p).is_err(), "{p}");
        }
    }

    #[test]
    fn relative_paths_resolve_against_the_document() {
        let d = Path::new("/home/me/docs/a.md");
        assert_eq!(resolve(d, "b.md").unwrap(), PathBuf::from("/home/me/docs/b.md"));
        assert_eq!(resolve(d, "../b%20c.md#frag").unwrap(), PathBuf::from("/home/me/b c.md"));
        assert_eq!(resolve(d, "sub/./b.md?x=1").unwrap(), PathBuf::from("/home/me/docs/sub/b.md"));
    }

    /// `TestIsWebURL` from `internal/doc/security_test.go`.
    #[test]
    fn web_url_table() {
        for (u, want) in [
            ("https://a/x.png", true),
            ("HTTP://a/x.png", true),
            ("//a/x.png", true),
            (r"\\a\x.png", true),
            (r"/\a/x.png", true),
            (" \u{1}https://a", true),
            ("ht\ttps://a", true),
            ("/\t/a/x.png", true),
            ("https:t.example/x.png", true),
            ("http:x", true),
            ("x.png", false),
            ("/local/c/x.png", false),
            ("../x.png", false),
            ("data:image/png;base64,AA", false),
            ("#x", false),
        ] {
            assert_eq!(is_web_url(u), want, "{u:?}");
        }
    }
}
