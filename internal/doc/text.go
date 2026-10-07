package doc

import (
	"bytes"
	"regexp"
	"strings"

	"github.com/yuin/goldmark/ast"
	east "github.com/yuin/goldmark/extension/ast"

	"github.com/gohugoio/hugo-goldmark-extensions/passthrough"
)

// nodeStart returns the source offset where a block node's content begins,
// or -1 when it has none (e.g. a thematic break).
func nodeStart(n ast.Node) int {
	for n != nil {
		if n.Type() == ast.TypeBlock && n.Lines().Len() > 0 {
			return n.Lines().At(0).Start
		}
		if t, ok := n.(*ast.Text); ok {
			return t.Segment.Start
		}
		n = n.FirstChild()
	}
	return -1
}

// blockText returns the raw source lines of a block node.
func blockText(n ast.Node, src []byte) []byte {
	var b bytes.Buffer
	lines := n.Lines()
	for i := 0; i < lines.Len(); i++ {
		seg := lines.At(i)
		b.Write(seg.Value(src))
	}
	if h, ok := n.(*ast.HTMLBlock); ok && h.HasClosure() {
		cl := h.ClosureLine
		b.Write(cl.Value(src))
	}
	return b.Bytes()
}

var (
	openTagRe  = regexp.MustCompile(`(?i)<(details|div|table|blockquote|section|center)\b[^>]*>`)
	closeTagRe = regexp.MustCompile(`(?i)</(details|div|table|blockquote|section|center)\s*>`)
)

// htmlDepthDelta counts container tags opened minus closed in raw HTML, so
// the chunker avoids splitting e.g. a <details> block in two.
func htmlDepthDelta(b []byte) int {
	return len(openTagRe.FindAllIndex(b, -1)) - len(closeTagRe.FindAllIndex(b, -1))
}

// plainText returns the visible text of an inline container such as a heading.
func plainText(n ast.Node, src []byte) string {
	var b strings.Builder
	_ = ast.Walk(n, func(c ast.Node, entering bool) (ast.WalkStatus, error) {
		if !entering {
			return ast.WalkContinue, nil
		}
		switch t := c.(type) {
		case *ast.Text:
			b.Write(t.Segment.Value(src))
			if t.SoftLineBreak() || t.HardLineBreak() {
				b.WriteByte(' ')
			}
		case *ast.String:
			b.Write(t.Value)
		case *ast.RawHTML:
			return ast.WalkSkipChildren, nil
		case *passthrough.PassthroughInline:
			b.Write(t.Segment.Value(src))
		}
		return ast.WalkContinue, nil
	})
	return b.String()
}

// searchText appends the text a reader would see for a top-level node. It
// mirrors what the DOM shows closely enough for find to line up; diagrams,
// maths and raw HTML are skipped because their rendered text differs.
func searchText(b *strings.Builder, n ast.Node, src []byte) {
	_ = ast.Walk(n, func(c ast.Node, entering bool) (ast.WalkStatus, error) {
		if !entering {
			if c.Type() == ast.TypeBlock || c.Kind() == east.KindTableCell {
				b.WriteByte(' ')
			}
			return ast.WalkContinue, nil
		}
		switch t := c.(type) {
		case *ast.Text:
			b.Write(t.Segment.Value(src))
			if t.SoftLineBreak() || t.HardLineBreak() {
				b.WriteByte(' ')
			}
		case *ast.String:
			b.Write(t.Value)
		case *ast.AutoLink:
			b.Write(t.Label(src))
			return ast.WalkSkipChildren, nil
		case *ast.FencedCodeBlock:
			if isMermaid(t, src) {
				return ast.WalkSkipChildren, nil
			}
			b.Write(blockText(t, src))
			b.WriteByte(' ')
			return ast.WalkSkipChildren, nil
		case *ast.CodeBlock:
			b.Write(blockText(t, src))
			b.WriteByte(' ')
			return ast.WalkSkipChildren, nil
		case *ast.HTMLBlock, *ast.RawHTML, *passthrough.PassthroughInline, *passthrough.PassthroughBlock:
			return ast.WalkSkipChildren, nil
		case *east.FootnoteLink, *east.FootnoteBacklink:
			return ast.WalkSkipChildren, nil
		}
		return ast.WalkContinue, nil
	})
}

func fenceLang(n *ast.FencedCodeBlock, src []byte) string {
	if n.Info == nil {
		return ""
	}
	info := n.Info.Segment.Value(src)
	if i := bytes.IndexAny(info, " \t{"); i >= 0 {
		info = info[:i]
	}
	return strings.ToLower(string(info))
}

func isMermaid(n *ast.FencedCodeBlock, src []byte) bool {
	return fenceLang(n, src) == "mermaid"
}
