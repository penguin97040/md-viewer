use mdv_content::{rasterise, render_math, render_mermaid, svgcheck, ContentError, ContentTheme, DiagramOptions, MathOptions};

#[test]
fn maths_renders_and_rasterises_at_logical_size() {
    let a = render_math("\\frac{a}{b}", true, &MathOptions::default()).unwrap();
    assert!(a.baseline_px > 0.0 && a.baseline_px < a.height_px);
    assert!(!a.svg.contains("<text"), "glyphs must be outlines");
    let img = rasterise(&a, 2.0).unwrap();
    assert_eq!(img.width, (a.width_px * 2.0).ceil() as u32);
    assert!(img.pixels.chunks_exact(4).any(|p| p[3] > 0));
}

#[test]
fn maths_errors_do_not_poison_later_renders() {
    assert!(matches!(render_math("\\frac{", false, &MathOptions::default()), Err(ContentError::Parse(_))));
    assert!(matches!(render_math("\\rule{60em}{1em}", false, &MathOptions::default()), Err(ContentError::TooLarge(_))));
    assert!(render_math("x^2", false, &MathOptions::default()).is_ok());
}

#[test]
fn mermaid_init_directives_are_ignored() {
    let plain = render_mermaid("flowchart LR\n A-->B", &DiagramOptions::default()).unwrap();
    let init = render_mermaid("%%{init: {\"fontFamily\":\"Comic\",\"themeCSS\":\"@import url(http://x/y.css);\"}}%%\nflowchart LR\n A-->B", &DiagramOptions::default()).unwrap();
    assert_eq!(plain.svg, init.svg);
}

#[test]
fn mermaid_themes_differ_and_rasterise() {
    let d = render_mermaid("flowchart LR\n A-->B", &DiagramOptions { theme: ContentTheme::Dark, ..Default::default() }).unwrap();
    let l = render_mermaid("flowchart LR\n A-->B", &DiagramOptions { theme: ContentTheme::Light, ..Default::default() }).unwrap();
    assert_ne!(d.svg, l.svg);
    assert!(rasterise(&d, 1.0).is_ok());
}

#[test]
fn svgcheck_rejects_script_links_and_css_urls() {
    for bad in [
        "<svg xmlns='http://www.w3.org/2000/svg'><script>1</script></svg>",
        "<svg xmlns='http://www.w3.org/2000/svg'><rect onclick='x()'/></svg>",
        "<svg xmlns='http://www.w3.org/2000/svg'><use href='http://evil/x.svg#a'/></svg>",
        "<svg xmlns='http://www.w3.org/2000/svg'><style>@import url(http://evil/x.css);</style></svg>",
        "<svg xmlns='http://www.w3.org/2000/svg'><rect style='fill:url(http://evil/a)'/></svg>",
        "<svg xmlns='http://www.w3.org/2000/svg'><foreignObject/></svg>",
    ] {
        assert!(svgcheck::check(bad).is_err(), "{bad}");
    }
    assert!(svgcheck::check("<svg xmlns='http://www.w3.org/2000/svg'><rect fill='url(#g)'/></svg>").is_ok());
}

#[test]
fn hostile_sizes_return_errors() {
    assert!(matches!(render_math(&"{".repeat(30_000), false, &MathOptions::default()), Err(ContentError::TooLarge(_))));
    assert!(render_math(&format!("{}x{}", "\\frac{1}{".repeat(100), "}".repeat(100)), false, &MathOptions::default()).is_err());
    let many = format!("flowchart LR\n{}", (0..400).map(|i| format!(" N{i}-->N{}\n", i + 1)).collect::<String>());
    assert!(matches!(render_mermaid(&many, &DiagramOptions::default()), Err(ContentError::TooLarge(_))));
}
