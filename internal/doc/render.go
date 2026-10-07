package doc

import (
	"bytes"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"

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
	"github.com/yuin/goldmark/util"

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
	goldmark.WithParserOptions(parser.WithAttribute()),
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

func writeCode(w util.BufWriter, lang string, code []byte) {
	w.WriteString("<pre><code")
	if lang != "" {
		w.WriteString(` class="hl language-`)
		w.Write(util.EscapeHTML([]byte(lang)))
		w.WriteString(`"`)
	}
	w.WriteString(">")
	if !highlight(w, lang, code) {
		w.Write(util.EscapeHTML(code))
	}
	w.WriteString("</code></pre>\n")
}

// highlight writes tokens as <span class="..."> using chroma's short class
// names. It returns false when the language is unknown.
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
	for tok := it(); tok != chroma.EOF; tok = it() {
		cls := tokenClass(tok.Type)
		if cls != "" {
			w.WriteString(`<span class="`)
			w.WriteString(cls)
			w.WriteString(`">`)
		}
		w.Write(util.EscapeHTML([]byte(tok.Value)))
		if cls != "" {
			w.WriteString("</span>")
		}
	}
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
	return string(policy.SanitizeBytes(b))
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
		writeStyleCSS(&b, "dark", styles.Get("github-dark"))
		writeStyleCSS(&b, "light", styles.Get("github"))
		css = b.String()
	})
	return css
}

func writeStyleCSS(b *strings.Builder, theme string, st *chroma.Style) {
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
		}
		if e.Bold == chroma.Yes {
			decl = append(decl, "font-weight:600")
		}
		if e.Italic == chroma.Yes {
			decl = append(decl, "font-style:italic")
		}
		if len(decl) == 0 {
			continue
		}
		fmt.Fprintf(b, "[data-theme=%s] .hl .%s{%s}\n", theme, c, strings.Join(decl, ";"))
	}
}
