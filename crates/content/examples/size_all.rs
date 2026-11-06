//! Size probe: maths, Mermaid and the rasteriser together, as the app would link them.
fn main() {
    let t = std::env::args().nth(1).unwrap_or_default();
    let m = mdv_content::render_math(&t, true, &Default::default());
    let d = mdv_content::render_mermaid(&t, &Default::default());
    let px = m.ok().and_then(|a| mdv_content::rasterise(&a, 1.0).ok()).map_or(0, |i| i.pixels.len());
    let px2 = d.ok().and_then(|a| mdv_content::rasterise(&a, 1.0).ok()).map_or(0, |i| i.pixels.len());
    println!("{}", px + px2);
}
