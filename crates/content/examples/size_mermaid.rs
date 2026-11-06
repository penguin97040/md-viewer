//! Size probe: Mermaid only (no rasteriser, no maths).
fn main() {
    let a = mdv_content::render_mermaid(&std::env::args().nth(1).unwrap_or_default(), &Default::default());
    println!("{}", a.map(|a| a.svg.len()).unwrap_or(0));
}
