//! GitHub-style heading ids. Port of `internal/doc/slug.go`.

use std::collections::{HashMap, HashSet};
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

use crate::fold::go_lower;

/// Makes ids: lower case, punctuation removed, spaces turned into hyphens, duplicates suffixed
/// with -1, -2 and so on. `next` remembers the last suffix per base so repeats stay O(1).
#[derive(Default)]
pub struct Slugger {
    seen: HashSet<String>,
    next: HashMap<String, u32>,
}

impl Slugger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reserve(&mut self, id: &str) {
        self.seen.insert(id.to_string());
    }

    pub fn slug(&mut self, text: &str) -> String {
        let mut base = String::with_capacity(text.len());
        for r in text.chars().map(go_lower) {
            if r == '_' || r == '-' || matches!(r.general_category_group(), GeneralCategoryGroup::Letter | GeneralCategoryGroup::Number) {
                base.push(r);
            } else if r == ' ' {
                base.push('-');
            }
        }
        if base.is_empty() {
            base.push_str("section");
        }
        let mut id = base.clone();
        if self.seen.contains(&id) {
            let mut n = self.next.get(&base).copied().unwrap_or(0);
            while self.seen.contains(&id) {
                n += 1;
                id = format!("{base}-{n}");
            }
            self.next.insert(base, n);
        }
        self.seen.insert(id.clone());
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_get_suffixes() {
        let mut s = Slugger::new();
        assert_eq!(s.slug("Hello, World!"), "hello-world");
        assert_eq!(s.slug("Hello World"), "hello-world-1");
        assert_eq!(s.slug("Hello World"), "hello-world-2");
        assert_eq!(s.slug("code & more"), "code--more");
        assert_eq!(s.slug("!!!"), "section");
        assert_eq!(s.slug("Whāngārei"), "whāngārei");
    }

    #[test]
    fn reserved_ids_are_skipped() {
        let mut s = Slugger::new();
        s.reserve("mine");
        assert_eq!(s.slug("mine"), "mine-1");
    }

    /// MISTAKES 2: repeats must not rescan from 1.
    #[test]
    fn many_repeats_stay_fast() {
        let mut s = Slugger::new();
        let t = std::time::Instant::now();
        let mut last = String::new();
        for _ in 0..200_000 {
            last = s.slug("Same text");
        }
        assert_eq!(last, "same-text-199999");
        assert!(t.elapsed().as_secs() < 2, "{:?}", t.elapsed());
    }
}
