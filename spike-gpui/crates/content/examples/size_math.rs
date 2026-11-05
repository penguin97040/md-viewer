//! Size probe: maths only (no rasteriser, no Mermaid).
fn main() {
    let a = mdv_content::render_math(&std::env::args().nth(1).unwrap_or_default(), true, &Default::default());
    println!("{}", a.map(|a| a.svg.len()).unwrap_or(0));
}
