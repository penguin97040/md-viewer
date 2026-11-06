//! Table sorting rules, ported from `frontend/extras.js`.

use std::cmp::Ordering;
use std::sync::OnceLock;

use regex::Regex;

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("valid pattern"))
}

/// Text sorts naturally: "item 2" before "item 10", and versions such as 1.2.0 before 1.10.0.
/// This stands in for `Intl.Collator(undefined, { numeric: true, sensitivity: 'base' })`: case
/// and accents do not count, digit runs compare as numbers, and punctuation sorts before digits
/// before letters. It is not full locale collation.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let ka = keys(a);
    let kb = keys(b);
    ka.cmp(&kb)
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Space,
    Punct(char),
    Num(Vec<u8>),
    Letter(char),
}

fn base_letter(c: char) -> char {
    use unicode_normalization::UnicodeNormalization;
    let mut d = std::iter::once(c).nfd();
    let first = d.next().unwrap_or(c);
    first.to_lowercase().next().unwrap_or(first)
}

fn keys(s: &str) -> Vec<Key> {
    let mut out = Vec::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c.is_ascii_digit() {
            let mut n = vec![c as u8];
            while let Some(&d) = it.peek() {
                if d.is_ascii_digit() {
                    n.push(d as u8);
                    it.next();
                } else {
                    break;
                }
            }
            // Numeric comparison: drop leading zeros, then shorter is smaller.
            let trimmed: Vec<u8> = n.iter().copied().skip_while(|d| *d == b'0').collect();
            let mut key = vec![trimmed.len().min(255) as u8];
            key.extend(trimmed);
            out.push(Key::Num(key));
        } else if c.is_whitespace() {
            out.push(Key::Space);
        } else if c.is_alphanumeric() {
            out.push(Key::Letter(base_letter(c)));
        } else if !matches!(c as u32, 0x300..=0x36f) {
            out.push(Key::Punct(c));
        }
    }
    out
}

/// `asNumber`: a cell's value, or `None` if it is not a number.
fn as_number(s: &str, decimal_comma: bool) -> Option<f64> {
    static NUMBER: OnceLock<Regex> = OnceLock::new();
    static THOUSANDS: OnceLock<Regex> = OnceLock::new();
    let mut t: String = s.chars().filter(|c| !c.is_whitespace() && !matches!(c, '$' | '€' | '£' | '¥' | '%')).collect();
    if decimal_comma {
        // Remove dots that sit before exactly three digits and then a non-digit or the end.
        let b: Vec<char> = t.chars().collect();
        let mut o = String::with_capacity(t.len());
        for (i, &c) in b.iter().enumerate() {
            let three = c == '.' && b.len() > i + 3 && b[i + 1..i + 4].iter().all(char::is_ascii_digit) && b.get(i + 4).is_none_or(|n| !n.is_ascii_digit());
            if !three {
                o.push(c);
            }
        }
        t = o.replacen(',', ".", 1);
    } else if re(&THOUSANDS, r"^[-+]?\d{1,3}(,\d{3})+(\.\d*)?$").is_match(&t) {
        t = t.replace(',', "");
    }
    if re(&NUMBER, r"(?i)^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$").is_match(&t) {
        t.parse().ok()
    } else {
        None
    }
}

/// `columnNumbers`: the cells as numbers, or `None` to sort the column as text. The choice is made
/// for the whole column so the order is consistent.
pub fn column_numbers(cells: &[String]) -> Option<Vec<Option<f64>>> {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    static DECIMAL: OnceLock<Regex> = OnceLock::new();
    let vals: Vec<&String> = cells.iter().filter(|c| !c.is_empty()).collect();
    if vals.is_empty() || vals.iter().any(|v| re(&VERSION, r"(?i)^v\d|\d\.\d+\.").is_match(v)) {
        return None;
    }
    let decimal_comma = vals.iter().any(|v| re(&DECIMAL, r"\d,(\d{1,2}|\d{4,})(\D|$)|\d\.\d{3},\d").is_match(v));
    let nums: Vec<Option<f64>> = cells.iter().map(|c| if c.is_empty() { None } else { as_number(c, decimal_comma) }).collect();
    nums.iter().zip(cells).all(|(n, c)| n.is_some() || c.is_empty()).then_some(nums)
}

/// The sort cycle: ascending, descending, then the original order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Ascending,
    Descending,
}

/// The next state when a header is clicked.
pub fn next_sort(cur: Option<(usize, SortDir)>, col: usize) -> Option<(usize, SortDir)> {
    match cur {
        Some((c, SortDir::Ascending)) if c == col => Some((col, SortDir::Descending)),
        Some((c, SortDir::Descending)) if c == col => None,
        _ => Some((col, SortDir::Ascending)),
    }
}

/// Row order for a column's cell texts. Empty cells always go last. The sort is stable, as
/// `Array.prototype.sort` is.
pub fn sorted_order(cells: &[String], dir: SortDir) -> Vec<usize> {
    let nums = column_numbers(cells);
    let d = if dir == SortDir::Ascending { 1 } else { -1 };
    let keys: Vec<Vec<Key>> = if nums.is_none() { cells.iter().map(|c| keys(c)).collect() } else { Vec::new() };
    let mut idx: Vec<usize> = (0..cells.len()).collect();
    idx.sort_by(|&x, &y| {
        let (ex, ey) = (cells[x].is_empty(), cells[y].is_empty());
        if ex || ey {
            return ex.cmp(&ey);
        }
        let ord = match &nums {
            Some(n) => n[x].partial_cmp(&n[y]).unwrap_or(Ordering::Equal),
            None => keys[x].cmp(&keys[y]),
        };
        if d > 0 { ord } else { ord.reverse() }
    });
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(cells: &[&str], dir: SortDir) -> Vec<String> {
        let c: Vec<String> = cells.iter().map(|s| s.to_string()).collect();
        sorted_order(&c, dir).into_iter().map(|i| c[i].clone()).collect()
    }

    #[test]
    fn numbers_with_thousands_and_currency() {
        assert_eq!(order(&["1,798,300", "215,200", "396,200"], SortDir::Ascending), ["215,200", "396,200", "1,798,300"]);
        assert_eq!(order(&["$5", "€10", "£1.50", "20%"], SortDir::Ascending), ["£1.50", "$5", "€10", "20%"]);
    }

    #[test]
    fn decimal_commas_are_detected_per_column() {
        assert_eq!(order(&["1.234,5", "2,5", "10,25"], SortDir::Ascending), ["2,5", "10,25", "1.234,5"]);
    }

    #[test]
    fn versions_and_mixed_columns_sort_as_text() {
        assert_eq!(order(&["1.10.0", "1.2.0", "1.9.3"], SortDir::Ascending), ["1.2.0", "1.9.3", "1.10.0"]);
        assert_eq!(order(&["v10", "v2"], SortDir::Ascending), ["v2", "v10"]);
        assert_eq!(order(&["10", "9", "abc"], SortDir::Ascending), ["9", "10", "abc"]);
    }

    #[test]
    fn text_is_natural_and_ignores_case_and_accents() {
        assert_eq!(order(&["item 10", "item 2", "Item 1"], SortDir::Ascending), ["Item 1", "item 2", "item 10"]);
        assert_eq!(order(&["Zebra", "éclair", "apple"], SortDir::Ascending), ["apple", "éclair", "Zebra"]);
    }

    #[test]
    fn empty_cells_stay_last_in_both_directions() {
        assert_eq!(order(&["2", "", "1"], SortDir::Ascending), ["1", "2", ""]);
        assert_eq!(order(&["2", "", "1"], SortDir::Descending), ["2", "1", ""]);
    }

    #[test]
    fn descending_keeps_equal_rows_in_order() {
        let c: Vec<String> = ["1", "2", "1"].iter().map(|s| s.to_string()).collect();
        assert_eq!(sorted_order(&c, SortDir::Descending), [1, 0, 2]);
    }

    #[test]
    fn the_cycle_goes_asc_desc_original() {
        let a = next_sort(None, 2);
        assert_eq!(a, Some((2, SortDir::Ascending)));
        let b = next_sort(a, 2);
        assert_eq!(b, Some((2, SortDir::Descending)));
        assert_eq!(next_sort(b, 2), None);
        assert_eq!(next_sort(b, 1), Some((1, SortDir::Ascending)));
    }
}
