package doc

import (
	"fmt"
	"strings"
	"testing"
)

func render(t *testing.T, d *Doc) string {
	t.Helper()
	var b strings.Builder
	for i := range d.Chunks {
		h, err := d.ChunkHTML(i)
		if err != nil {
			t.Fatal(err)
		}
		b.WriteString(h)
	}
	return b.String()
}

func TestSlugs(t *testing.T) {
	d := Parse([]byte("# Hello, World!\n\n## Hello World\n\n## Hello World\n\n## `code` & *more*\n\n## Custom {#mine}\n"))
	want := []string{"hello-world", "hello-world-1", "hello-world-2", "code--more", "mine"}
	if len(d.Headings) != len(want) {
		t.Fatalf("got %d headings", len(d.Headings))
	}
	for i, h := range d.Headings {
		if h.ID != want[i] {
			t.Errorf("heading %d: got %q, want %q", i, h.ID, want[i])
		}
	}
	html := render(t, d)
	if !strings.Contains(html, `id="hello-world-2"`) || !strings.Contains(html, `id="mine"`) {
		t.Errorf("ids missing from HTML: %s", html)
	}
}

func TestChunking(t *testing.T) {
	var b strings.Builder
	for i := 0; i < 2000; i++ {
		fmt.Fprintf(&b, "## Section %d\n\nSome paragraph text for section %d with **bold** words.\n\n", i, i)
	}
	d := Parse([]byte(b.String()))
	if len(d.Chunks) < 5 {
		t.Fatalf("expected several chunks, got %d", len(d.Chunks))
	}
	total := 0
	for _, c := range d.Chunks {
		total += c.Bytes
	}
	if total != len(d.Source) {
		t.Errorf("chunk bytes %d != source %d", total, len(d.Source))
	}
	if len(d.Headings) != 2000 {
		t.Errorf("got %d headings", len(d.Headings))
	}
	last := d.Headings[len(d.Headings)-1]
	h, _ := d.ChunkHTML(last.Chunk)
	if !strings.Contains(h, `id="`+last.ID+`"`) {
		t.Errorf("last heading not in its chunk")
	}
}

func TestDetailsNotSplit(t *testing.T) {
	var b strings.Builder
	b.WriteString("<details>\n<summary>More</summary>\n\n")
	for i := 0; i < 1000; i++ {
		b.WriteString("Line of text inside details.\n\n")
	}
	b.WriteString("</details>\n\nAfter.\n")
	d := Parse([]byte(b.String()))
	h, _ := d.ChunkHTML(0)
	if !strings.Contains(h, "</details>") {
		t.Errorf("details split across chunks (%d chunks)", len(d.Chunks))
	}
}

func TestRendering(t *testing.T) {
	src := "```mermaid\ngraph TD; A-->B\n```\n\n" +
		"```go\nfunc main() {}\n```\n\n" +
		"Inline $x^2$ costs $5 and $10.\n\n$$\na+b\n$$\n\n" +
		"- [x] done\n- [ ] todo\n\n" +
		"| a | b |\n|---|---|\n| 1 | 2 |\n\n" +
		"Note[^1].\n\n[^1]: Footnote.\n\n" +
		"<script>alert(1)</script>\n\n<details><summary>S</summary>hidden</details>\n"
	d := Parse([]byte(src))
	html := render(t, d)
	for _, want := range []string{
		`<pre class="mermaid">graph TD; A--&gt;B`,
		`<code class="hl language-go"><span class="kd">func</span>`,
		`<span class="math">x^2</span>`,
		`costs $5 and $10`,
		`<div class="math display">`,
		`type="checkbox"`,
		`<table>`,
		`id="fn:1"`,
		`<details><summary>S</summary>`,
	} {
		if !strings.Contains(html, want) {
			t.Errorf("missing %q in:\n%s", want, html)
		}
	}
	if strings.Contains(html, "<script") {
		t.Error("script not sanitised")
	}
	if _, ok := d.Anchors["fn:1"]; !ok {
		t.Error("footnote anchor missing")
	}
}

func TestSearch(t *testing.T) {
	d := Parse([]byte("# Apple\n\nAn apple a day.\n\n```\napple pie\n```\n\n| APPLE | x |\n|---|---|\n"))
	counts := d.Search("apple")
	if counts[0] != 4 {
		t.Errorf("got %d matches, want 4", counts[0])
	}
	if d.Search("  ")[0] != 0 {
		t.Error("blank query should match nothing")
	}
}

func TestHighlightCSS(t *testing.T) {
	css := HighlightCSS()
	if !strings.Contains(css, "[data-theme=dark] .hl .k{") || !strings.Contains(css, "[data-theme=light] .hl .k{") {
		t.Errorf("unexpected css: %.200s", css)
	}
}

// bigDoc builds roughly n bytes of mixed markdown.
func bigDoc(n int) []byte {
	var b strings.Builder
	for i := 0; b.Len() < n; i++ {
		fmt.Fprintf(&b, "## Heading %d\n\nA paragraph with **bold**, *italic*, `code` and a [link](https://example.com/%d). "+
			"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore.\n\n"+
			"- item one\n- item two\n  - nested\n\n"+
			"| col a | col b | col c |\n|---|---|---|\n| %d | two | three |\n| x | y | z |\n\n"+
			"```js\nfunction f%d(a, b) { return a + b; }\n```\n\n> A quote line.\n\n", i, i, i, i)
	}
	return []byte(b.String())
}

func BenchmarkParse25MB(b *testing.B) {
	src := bigDoc(25 << 20)
	b.SetBytes(int64(len(src)))
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		Parse(src)
	}
}

func BenchmarkChunkHTML(b *testing.B) {
	d := Parse(bigDoc(4 << 20))
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		d.html = map[int]string{}
		if _, err := d.ChunkHTML(i % len(d.Chunks)); err != nil {
			b.Fatal(err)
		}
	}
}

func BenchmarkSearch(b *testing.B) {
	d := Parse(bigDoc(25 << 20))
	d.Search("x") // build text index
	b.ResetTimer()
	for i := 0; i < b.N; i++ {
		d.Search("lorem ipsum")
	}
}

func TestSplitSections(t *testing.T) {
	var b strings.Builder
	for i := 0; i < 200; i++ {
		fmt.Fprintf(&b, "# H%d\n\n- list item\n\n- second item after blank\n\n```\n# not a heading\n\nstill code\n```\n\n<!--\n\ncomment\n\n-->\n\nPara %d.\n\n", i, i)
	}
	src := []byte(b.String())
	secs := splitSections(src, 1024)
	if len(secs) < 10 {
		t.Fatalf("expected many sections, got %d", len(secs))
	}
	prev := 0
	for _, s := range secs {
		if s[0] != prev {
			t.Fatalf("gap at %d", s[0])
		}
		prev = s[1]
		rest := string(src[s[0]:])
		if strings.HasPrefix(rest, "- ") || strings.HasPrefix(rest, "still") || strings.HasPrefix(rest, "comment") || strings.HasPrefix(rest, "-->") {
			t.Errorf("unsafe cut before %q", string(src[s[0]:min(s[0]+20, len(src))]))
		}
	}
	if prev != len(src) {
		t.Errorf("sections end at %d, want %d", prev, len(src))
	}
}

func TestSectionMode(t *testing.T) {
	var b strings.Builder
	b.WriteString("See [the ref][r] and [jump](#heading-1).\n\n")
	for i := 0; b.Len() < wholeParseLimit+100_000; i++ {
		fmt.Fprintf(&b, "## Heading\n\nText %d with some words to fill the section up nicely.\n\n", i)
	}
	b.WriteString("[r]: https://example.com/ref\n")
	d := Parse([]byte(b.String()))
	if d.isWhole() || len(d.Chunks) < 10 {
		t.Fatalf("expected section mode, got %d chunks", len(d.Chunks))
	}
	h0, _ := d.ChunkHTML(0)
	if !strings.Contains(h0, `href="https://example.com/ref"`) {
		t.Errorf("cross-section reference not resolved: %.300s", h0)
	}
	if d.Headings[1].ID != "heading-1" {
		t.Errorf("bad heading ids: %+v", d.Headings[:2])
	}
	last := d.Headings[len(d.Headings)-1]
	h, _ := d.ChunkHTML(last.Chunk)
	if !strings.Contains(h, `id="`+last.ID+`"`) {
		t.Errorf("id %s not rendered in chunk %d", last.ID, last.Chunk)
	}
	if n := d.Search("text 5 with")[0]; n != 1 {
		t.Errorf("search count %d", n)
	}
}
