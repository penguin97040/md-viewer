//! Headless evidence tool. Renders every `*.mmd` in a directory and every row of a maths TSV
//! (`id<TAB>D|I<TAB>tex`) for dark and light, writing `<id>-<theme>-rust.{svg,png}` and printing
//! timings.
//!
//! cargo run --release -p mdv-content --example gallery -- <samples-dir> <maths.tsv> <out-dir>

use std::time::Instant;

use mdv_content::{rasterise, render_math, render_mermaid, ContentTheme, DiagramOptions, MathOptions, RenderedAsset};

fn write_png(path: &str, a: &RenderedAsset, scale: f32) -> Result<(u32, u32), String> {
    let img = rasterise(a, scale).map_err(|e| e.to_string())?;
    let f = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().and_then(|mut w| w.write_image_data(&img.pixels)).map_err(|e| e.to_string())?;
    Ok((img.width, img.height))
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (samples, maths, out) = (&args[1], &args[2], &args[3]);
    std::fs::create_dir_all(out).unwrap();
    let mut files: Vec<_> = std::fs::read_dir(samples).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "mmd")).collect();
    files.sort();
    let mut first = true;
    for (tname, theme) in [("dark", ContentTheme::Dark), ("light", ContentTheme::Light)] {
        let dopts = DiagramOptions { theme, ..Default::default() };
        for f in &files {
            let id = f.file_stem().unwrap().to_string_lossy().to_string();
            let src = std::fs::read_to_string(f).unwrap();
            let t0 = Instant::now();
            let r = render_mermaid(&src, &dopts);
            let cold = t0.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok(a) => {
                    let times: Vec<f64> = (0..7).map(|_| { let t = Instant::now(); let _ = render_mermaid(&src, &dopts); t.elapsed().as_secs_f64() * 1000.0 }).collect();
                    let t = Instant::now();
                    let px = write_png(&format!("{out}/{id}-{tname}-rust.png"), &a, 2.0);
                    let rms = t.elapsed().as_secs_f64() * 1000.0;
                    std::fs::write(format!("{out}/{id}-{tname}-rust.svg"), a.svg.as_bytes()).unwrap();
                    println!("diagram\t{tname}\t{id}\tOK\t{:.0}x{:.0}\tfirst_ms={cold:.2}\tmedian_ms={:.2}\traster2x_ms={rms:.1}\t{:?}\tsvg_bytes={}", a.width_px, a.height_px, median(times), px, a.svg.len());
                    if first { first = false; }
                }
                Err(e) => println!("diagram\t{tname}\t{id}\tERR\t{e}"),
            }
        }
        let mopts = MathOptions { theme, ..Default::default() };
        for line in std::fs::read_to_string(maths).unwrap().lines() {
            let mut p = line.splitn(3, '\t');
            let (id, d, tex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            let display = d == "D";
            let t0 = Instant::now();
            let r = render_math(tex, display, &mopts);
            let cold = t0.elapsed().as_secs_f64() * 1000.0;
            match r {
                Ok(a) => {
                    let times: Vec<f64> = (0..7).map(|_| { let t = Instant::now(); let _ = render_math(tex, display, &mopts); t.elapsed().as_secs_f64() * 1000.0 }).collect();
                    let _ = write_png(&format!("{out}/math-{id}-{tname}-rust.png"), &a, 2.0);
                    std::fs::write(format!("{out}/math-{id}-{tname}-rust.svg"), a.svg.as_bytes()).unwrap();
                    println!("math\t{tname}\t{id}\tOK\t{:.1}x{:.1} base={:.1}\tfirst_ms={cold:.2}\tmedian_ms={:.3}\tsvg_bytes={}", a.width_px, a.height_px, a.baseline_px, median(times), a.svg.len());
                }
                Err(e) => println!("math\t{tname}\t{id}\tERR\t{e}"),
            }
        }
    }
}
