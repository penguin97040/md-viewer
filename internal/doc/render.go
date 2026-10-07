package doc

import (
	"bytes"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/alecthomas/chroma/v2"
	"github.com/alecthomas/chroma/v2/lexers"
	"github.com/alecthomas/chroma/v2/styles"
	"github.com/microcosm-cc/bluemonday"
	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/ast"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/renderer"
	"github.com/yuin/goldmark/renderer/html"
	"github.com/yuin/goldmark/text"
	"github.com/yuin/goldmark/util"
	xhtml "golang.org/x/net/html"

	"github.com/gohugoio/hugo-goldmark-extensions/passthrough"
)

// maxHighlightBytes skips syntax highlighting for very large code blocks,
// which would be slow to tokenise and heavy to display.
const maxHighlightBytes = 256 * 1024

var markdown = goldmark.New(
	goldmark.WithExtensions(
		extension.GFM,
		extension.Footnote,
		extension.DefinitionList,
		passthrough.New(passthrough.Config{
			InlineDelimiters: []passthrough.Delimiters{{Open: "$", Close: "$"}, {Open: `\(`, Close: `\)`}},
			BlockDelimiters:  []passthrough.Delimiters{{Open: "$$", Close: "$$"}, {Open: `\[`, Close: `\]`}},
		}),
	),
	goldmark.WithParserOptions(
		parser.WithAttribute(),
		parser.WithASTTransformers(util.Prioritized(headingAttrs{}, 1000)),
	),
	goldmark.WithRendererOptions(
		html.WithUnsafe(), // raw HTML is kept, then sanitised
		renderer.WithNodeRenderers(util.Prioritized(&nodeRenderer{}, 1)),
	),
)

// nodeRenderer overrides code blocks (highlighting, mermaid) and maths.
type nodeRenderer struct{}

func (r *nodeRenderer) RegisterFuncs(reg renderer.NodeRendererFuncRegisterer) {
	reg.Register(ast.KindFencedCodeBlock, r.fenced)
	reg.Register(ast.KindCodeBlock, r.indented)
	reg.Register(passthrough.KindPassthroughInline, r.mathInline)
	reg.Register(passthrough.KindPassthroughBlock, r.mathBlock)
	reg.Register(ast.KindHTMLBlock, r.htmlBlock)
	reg.Register(ast.KindRawHTML, r.rawHTML)
}

// headingAttrs keeps only the id from heading attributes ({#id .class}),
// so a document can't give its headings the viewer's own classes.
type headingAttrs struct{}

func (headingAttrs) Transform(doc *ast.Document, _ text.Reader, _ parser.Context) {
	walkHeadings(doc, func(h *ast.Heading) {
		id, ok := h.AttributeString("id")
		h.RemoveAttributes()
		if ok {
			h.SetAttributeString("id", id)
		}
	})
}

// htmlBlock and rawHTML write a document's own HTML without class
// attributes. The viewer gives meaning to its classes (a hidden print copy,
// the code block a copy button copies), so a document borrowing them could
// show one thing and copy another. The sanitiser removes the rest.
func (r *nodeRenderer) htmlBlock(w util.BufWriter, src []byte, node ast.Node, entering bool) (ast.WalkStatus, error) {
	if !entering {
		return ast.WalkContinue, nil
	}
	n := node.(*ast.HTMLBlock)
	var b []byte
	for i := 0; i < n.Lines().Len(); i++ {
		line := n.Lines().At(i)
		b = append(b, line.Value(src)...)
	}
	if n.HasClosure() {
		b = append(b, n.ClosureLine.Value(src)...)
	}
	w.Write(stripClasses(b))
	return ast.WalkSkipChildren, nil
}

func (r *nodeRenderer) rawHTML(w util.BufWriter, src []byte, node ast.Node, entering bool) (ast.WalkStatus, error) {
	if !entering {
		return ast.WalkSkipChildren, nil
	}
	n := node.(*ast.RawHTML)
	var b []byte
	for i := 0; i < n.Segments.Len(); i++ {
		seg := n.Segments.At(i)
		b = append(b, seg.Value(src)...)
	}
	w.Write(stripClasses(b))
	return ast.WalkSkipChildren, nil
}

// stripClasses removes class attributes from tags in raw HTML, leaving
// everything else byte for byte. A tag left unfinished at the end is
// escaped, so it can't join up with what follows.
func stripClasses(b []byte) []byte {
	if !bytes.Contains(bytes.ToLower(b), []byte("class")) {
		return b
	}
	z := xhtml.NewTokenizer(bytes.NewReader(b))
	var out bytes.Buffer
	for {
		tt := z.Next()
		raw := z.Raw()
		switch tt {
		case xhtml.ErrorToken:
			out.WriteString(xhtml.EscapeString(string(raw)))
			return out.Bytes()
		case xhtml.StartTagToken, xhtml.SelfClosingTagToken:
			t := z.Token()
			kept := t.Attr[:0]
			for _, a := range t.Attr {
				if a.Key != "class" {
					kept = append(kept, a)
				}
			}
			if len(kept) == len(t.Attr) {
				out.Write(raw)
				continue
			}
			t.Attr = kept
			out.WriteString(t.String())
		default:
			out.Write(raw)
		}
	}
}

func (r *nodeRenderer) fenced(w util.BufWriter, src []byte, node ast.Node, entering bool) (ast.WalkStatus, error) {
	if !entering {
		return ast.WalkSkipChildren, nil
	}
	n := node.(*ast.FencedCodeBlock)
	lang := fenceLang(n, src)
	code := blockText(n, src)
	if lang == "mermaid" {
		w.WriteString(`<pre class="mermaid">`)
		w.Write(util.EscapeHTML(code))
		w.WriteString("</pre>\n")
		return ast.WalkSkipChildren, nil
	}
	writeCode(w, lang, code)
	return ast.WalkSkipChildren, nil
}

func (r *nodeRenderer) indented(w util.BufWriter, src []byte, node ast.Node, entering bool) (ast.WalkStatus, error) {
	if entering {
		writeCode(w, "", blockText(node, src))
	}
	return ast.WalkSkipChildren, nil
}

// writeCode writes a code block. The wrapper div holds the copy button, so
// the button stays put when the block scrolls sideways.
func writeCode(w util.BufWriter, lang string, code []byte) {
	w.WriteString(`<div class="code"><pre><code`)
	if lang != "" {
		w.WriteString(` class="hl language-`)
		w.Write(util.EscapeHTML([]byte(lang)))
		w.WriteString(`"`)
	}
	w.WriteString(">")
	if !highlight(w, lang, code) {
		w.Write(util.EscapeHTML(code))
	}
	w.WriteString("</code></pre></div>\n")
}

// highlightBudget bounds the time spent highlighting one code block. Some
// lexers' patterns backtrack badly on unusual text (minutes for a few
// hundred KB), so a block that runs over is shown plain instead.
const highlightBudget = time.Second

// highlight writes tokens as <span class="..."> using chroma's short class
// names. It returns false, having written nothing, when the language is
// unknown or highlighting takes too long.
func highlight(w util.BufWriter, lang string, code []byte) bool {
	if lang == "" || len(code) > maxHighlightBytes {
		return false
	}
	lexer := lexers.Get(lang)
	if lexer == nil {
		return false
	}
	it, err := chroma.Coalesce(lexer).Tokenise(nil, string(code))
	if err != nil {
		return false
	}
	deadline := time.Now().Add(highlightBudget)
	var b bytes.Buffer
	for tok := it(); tok != chroma.EOF; tok = it() {
		if time.Now().After(deadline) {
			return false
		}
		cls := tokenClass(tok.Type)
		if cls != "" {
			b.WriteString(`<span class="`)
			b.WriteString(cls)
			b.WriteString(`">`)
		}
		b.Write(util.EscapeHTML([]byte(tok.Value)))
		if cls != "" {
			b.WriteString("</span>")
		}
	}
	w.Write(b.Bytes())
	return true
}

func tokenClass(t chroma.TokenType) string {
	for _, c := range []chroma.TokenType{t, t.SubCategory(), t.Category()} {
		if s := chroma.StandardTypes[c]; s != "" {
			return s
		}
	}
	return ""
}

func (r *nodeRenderer) mathInline(w util.BufWriter, src []byte, node ast.Node, entering bool) (ast.WalkStatus, error) {
	if !entering {
		return ast.WalkContinue, nil
	}
	n := node.(*passthrough.PassthroughInline)
	raw := n.Segment.Value(src)
	body := trimDelims(raw, n.Delimiters)
	// Treat "$5 and $10" as plain text, like GitHub does: the content must not
	// start or end with a space, and the closing $ must not precede a digit.
	if n.Delimiters.Open == "$" {
		after := byte(0)
		if n.Segment.Stop < len(src) {
			after = src[n.Segment.Stop]
		}
		if len(body) == 0 || isSpace(body[0]) || isSpace(body[len(body)-1]) || (after >= '0' && after <= '9') {
			w.Write(util.EscapeHTML(raw))
			return ast.WalkSkipChildren, nil
		}
	}
	w.WriteString(`<span class="math">`)
	w.Write(util.EscapeHTML(body))
	w.WriteString("</span>")
	return ast.WalkSkipChildren, nil
}

func (r *nodeRenderer) mathBlock(w util.BufWriter, src []byte, node ast.Node, entering bool) (ast.WalkStatus, error) {
	if !entering {
		return ast.WalkSkipChildren, nil
	}
	n := node.(*passthrough.PassthroughBlock)
	w.WriteString(`<div class="math display">`)
	w.Write(util.EscapeHTML(trimDelims(blockText(n, src), n.Delimiters)))
	w.WriteString("</div>\n")
	return ast.WalkSkipChildren, nil
}

func trimDelims(b []byte, d *passthrough.Delimiters) []byte {
	if d == nil {
		return b
	}
	b = bytes.TrimSpace(b)
	b = bytes.TrimPrefix(b, []byte(d.Open))
	b = bytes.TrimSuffix(b, []byte(d.Close))
	return b
}

func isSpace(c byte) bool { return c == ' ' || c == '\t' || c == '\n' }

var policy = func() *bluemonday.Policy {
	p := bluemonday.UGCPolicy()
	p.RequireNoFollowOnLinks(false)
	p.AllowAttrs("class", "id", "role", "title", "align").Globally()
	p.AllowAttrs("width", "height").OnElements("img", "td", "th", "table")
	p.AllowElements("kbd", "mark", "ins", "del", "sup", "sub", "details", "summary", "picture", "source", "center")
	p.AllowAttrs("open").OnElements("details")
	p.AllowAttrs("srcset", "media", "type").OnElements("source")
	p.AllowAttrs("type").Matching(regexp.MustCompile(`^checkbox$`)).OnElements("input")
	p.AllowAttrs("checked", "disabled").OnElements("input")
	p.AllowDataURIImages()
	return p
}()

func sanitise(b []byte) string {
	return holdWebImages(policy.SanitizeBytes(b))
}

// holdWebImages moves image addresses on the web into data-web-src and
// data-web-srcset, so nothing is fetched until the reader agrees: loading an
// image tells its site that the file was opened, and from where. It runs on
// sanitised HTML, which is well formed.
func holdWebImages(b []byte) string {
	if !bytes.Contains(b, []byte("<img")) && !bytes.Contains(b, []byte("<source")) {
		return string(b)
	}
	z := xhtml.NewTokenizer(bytes.NewReader(b))
	var out strings.Builder
	for {
		tt := z.Next()
		raw := z.Raw()
		if tt == xhtml.ErrorToken {
			out.Write(raw)
			return out.String()
		}
		if tt != xhtml.StartTagToken && tt != xhtml.SelfClosingTagToken {
			out.Write(raw)
			continue
		}
		t := z.Token()
		if t.Data != "img" && t.Data != "source" {
			out.Write(raw)
			continue
		}
		held := false
		for i, a := range t.Attr {
			if (a.Key == "src" && isWebURL(a.Val)) || (a.Key == "srcset" && webSrcset(a.Val)) {
				t.Attr[i].Key = "data-web-" + a.Key
				held = true
			}
		}
		if held {
			out.WriteString(t.String())
		} else {
			out.Write(raw)
		}
	}
}

// isWebURL reports whether an image address points at another computer:
// http, https or protocol-relative (//host/...). It reads the address the
// way a browser does: tabs and newlines are ignored, leading spaces and
// control characters are dropped, and \ counts as /.
func isWebURL(u string) bool {
	u = strings.Map(func(r rune) rune {
		switch r {
		case '\t', '\n', '\r':
			return -1
		case '\\':
			return '/'
		}
		return r
	}, u)
	u = strings.TrimLeft(u, "\x00\x01\x02\x03\x04\x05\x06\x07\x08\x0b\x0c\x0e\x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\x1a\x1b\x1c\x1d\x1e\x1f ")
	if strings.HasPrefix(u, "//") {
		return true
	}
	scheme, _, ok := strings.Cut(u, ":")
	return ok && (strings.EqualFold(scheme, "http") || strings.EqualFold(scheme, "https"))
}

func webSrcset(v string) bool {
	for _, c := range strings.Split(v, ",") {
		if f := strings.Fields(c); len(f) > 0 && isWebURL(f[0]) {
			return true
		}
	}
	return false
}

var (
	cssOnce sync.Once
	css     string
)

// HighlightCSS returns syntax colours for both themes, keyed off the
// data-theme attribute on the root element.
func HighlightCSS() string {
	cssOnce.Do(func() {
		var b strings.Builder
		writeStyleCSS(&b, "[data-theme=dark]", styles.Get("github-dark"), false)
		writeStyleCSS(&b, "[data-theme=light]", styles.Get("github"), false)
		// Printing always uses light colours, whatever the theme. Every class
		// gets a rule so no dark-theme colour survives.
		b.WriteString("@media print{\n")
		writeStyleCSS(&b, "[data-theme]", styles.Get("github"), true)
		b.WriteString("}\n")
		css = b.String()
	})
	return css
}

// writeStyleCSS writes one rule per token class. With complete, classes the
// style leaves plain are reset to inherit, overriding other themes.
func writeStyleCSS(b *strings.Builder, scope string, st *chroma.Style, complete bool) {
	classes := make([]string, 0, len(chroma.StandardTypes))
	byClass := map[string]chroma.TokenType{}
	for tt, c := range chroma.StandardTypes {
		if c != "" && tt != chroma.Background && tt != chroma.PreWrapper && tt != chroma.Line &&
			tt != chroma.LineNumbers && tt != chroma.LineNumbersTable && tt != chroma.LineHighlight &&
			tt != chroma.LineTable && tt != chroma.LineTableTD && tt != chroma.CodeLine && tt != chroma.LineLink {
			classes = append(classes, c)
			byClass[c] = tt
		}
	}
	sort.Strings(classes)
	for _, c := range classes {
		e := st.Get(byClass[c])
		var decl []string
		if e.Colour.IsSet() {
			decl = append(decl, "color:"+e.Colour.String())
		} else if complete {
			decl = append(decl, "color:inherit")
		}
		if e.Bold == chroma.Yes {
			decl = append(decl, "font-weight:600")
		} else if complete {
			decl = append(decl, "font-weight:inherit")
		}
		if e.Italic == chroma.Yes {
			decl = append(decl, "font-style:italic")
		} else if complete {
			decl = append(decl, "font-style:inherit")
		}
		if len(decl) == 0 {
			continue
		}
		fmt.Fprintf(b, "%s .hl .%s{%s}\n", scope, c, strings.Join(decl, ";"))
	}
}
