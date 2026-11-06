//! Prints chunk ranges, headings and search counts, for comparison with the Go reference.
use mdv_reader::doc::Document;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let d = Document::load(std::path::Path::new(&args[1])).unwrap();
    println!("parse_ms\t{}", d.parse_time.as_secs_f64() * 1000.0);
    for m in &d.chunks {
        println!("R\t{}\t{}", m.start, m.end);
    }
    for h in &d.headings {
        println!("H\t{}\t{}\t{}\t{}", h.level, h.text.replace('\t', " "), h.id, h.chunk);
    }
    for q in &args[2..] {
        let c = d.search(q);
        println!("Q\t{}\t{}", q, c.iter().map(u32::to_string).collect::<Vec<_>>().join(","));
    }
}
