//! Measurement aid: renders sample maths and diagrams and writes raw straight-alpha RGBA
//! (`W H\n` then bytes) for `scale` x `theme`, and reports how the reader's pipeline treats them.
//! `cargo run --release -p mdv-content --example dumpraster -- <outdir> <scale>`
use mdv_content::*;
use std::io::Write;

fn dump(dir: &str, name: &str, a: &RenderedAsset, scale: f32) {
    let img = rasterise(a, scale).unwrap();
    let mut f = std::fs::File::create(format!("{dir}/{name}.rgba")).unwrap();
    write!(f, "{} {}\n", img.width, img.height).unwrap();
    f.write_all(&img.pixels).unwrap();
    println!("{name} scale={scale} asset={:.3}x{:.3} raster={}x{} painted_phys={:.3}x{:.3}",
        a.width_px, a.height_px, img.width, img.height, a.width_px * scale, a.height_px * scale);
}

/// Un-premultiply (in `rasterise`) then premultiply again (in the reader): worst and mean error
/// over every (premultiplied value, alpha) pair.
fn roundtrip() {
    let (mut maxe, mut sum, mut n) = (0i32, 0u64, 0u64);
    for a in 1u32..255 {
        for p in 0..=a {
            let st = ((p * 255 + a / 2) / a).min(255);
            let rt = ((st * a + 127) / 255) as i32;
            let e = (rt - p as i32).abs();
            maxe = maxe.max(e);
            sum += e as u64;
            n += 1;
        }
    }
    println!("premul roundtrip: max err {maxe}, mean {:.4} (of 255)", sum as f64 / n as f64);
}

fn main() {
    roundtrip();
    let a: Vec<String> = std::env::args().collect();
    let (dir, scale) = (&a[1], a[2].parse::<f32>().unwrap());
    std::fs::create_dir_all(dir).unwrap();
    let maths = [("frac", r"t = \frac{d}{5} + \frac{h}{600}", true), ("int", r"\int_0^1 x^2 \, dx = \frac{1}{3}", true), ("inline", r"t = 2.8 + 1.5 \approx 4.3", false)];
    let mmd = "flowchart LR\n  A[Start] --> B{Decide}\n  B -->|yes| C[Walk]\n  B -->|no| D[Rest]\n";
    for (tn, th) in [("dark", ContentTheme::Dark), ("light", ContentTheme::Light)] {
        for (id, tex, disp) in maths {
            let asset = render_math(tex, disp, &MathOptions { theme: th, font_size: 17.0 * 1.1, text_color: None }).unwrap();
            dump(dir, &format!("math-{id}-{tn}"), &asset, scale);
        }
        let asset = render_mermaid(mmd, &DiagramOptions { theme: th, font_size: 17.0, text_color: None }).unwrap();
        dump(dir, &format!("mermaid-{tn}"), &asset, scale);
    }
}
