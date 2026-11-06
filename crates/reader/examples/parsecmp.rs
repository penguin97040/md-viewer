//! `parsecmp FILE WORD WORD`: parse and search times, for comparison with the Go reader.
use mdv_reader::doc::Document;
use std::time::Instant;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    for _ in 0..7 {
        let t = Instant::now();
        let d = Document::load(std::path::Path::new(&a[1])).expect("load");
        let parse = t.elapsed();
        let t = Instant::now();
        let first = d.search(&a[2]).len();
        let s1 = t.elapsed();
        let t = Instant::now();
        let second = d.search(&a[3]).len();
        let s2 = t.elapsed();
        println!("parse_ms={:.1} search_first_ms={:.1} hits={first} search_second_ms={:.1} hits={second}", parse.as_secs_f64() * 1e3, s1.as_secs_f64() * 1e3, s2.as_secs_f64() * 1e3);
    }
}
