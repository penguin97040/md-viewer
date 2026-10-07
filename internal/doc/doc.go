// Package doc parses markdown into lazily rendered chunks.
//
// Small files (up to wholeParseLimit) are parsed once and the AST is kept;
// its top-level blocks are grouped into chunks of about targetChunkBytes.
//
// Large files are cut into sections at safe block boundaries (see split.go).
// Every section is parsed in parallel once at open to collect headings and
// link reference definitions, then the ASTs are dropped. A section is parsed
// again only when the viewer asks for its HTML. This keeps memory close to
// the file size and opening fast.
package doc

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/yuin/goldmark/ast"
	east "github.com/yuin/goldmark/extension/ast"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/text"
)

const (
	// targetChunkBytes is the approximate amount of markdown source per chunk.
	targetChunkBytes = 16 * 1024
	// maxHoldBytes caps how far a chunk may grow while waiting for an open
	// raw HTML block (e.g. <details>) to close.
	maxHoldBytes = 512 * 1024
	// wholeParseLimit is the largest file parsed as a single document.
	wholeParseLimit = 1 << 20
	// htmlCacheLimit bounds how many rendered chunks are kept for large files.
	htmlCacheLimit = 96
	// MaxFileBytes is the largest file Load reads, so a huge or endless file
	// can't use up memory.
	MaxFileBytes = 256 << 20
)

// Heading is a table of contents entry.
type Heading struct {
	Level int    `json:"level"`
	Text  string `json:"text"`
	ID    string `json:"id"`
	Chunk int    `json:"chunk"`
}

// MarshalJSON encodes a heading as [level, text, id, chunk] to keep the
// payload small for documents with very many headings.
func (h Heading) MarshalJSON() ([]byte, error) {
	return json.Marshal([]any{h.Level, h.Text, h.ID, h.Chunk})
}

// ChunkInfo describes a chunk before it is rendered, so the viewer can
// estimate its height.
type ChunkInfo struct {
	Bytes int `json:"bytes"`
	Lines int `json:"lines"`
}

// MarshalJSON encodes chunk info as [bytes, lines].
func (c ChunkInfo) MarshalJSON() ([]byte, error) {
	return json.Marshal([2]int{c.Bytes, c.Lines})
}

type chunk struct {
	start, end int
	nodes      []ast.Node // whole-parse mode only
	ids        []string   // section mode: heading ids in document order
}

// Doc is a parsed markdown document.
type Doc struct {
	Source    []byte
	Chunks    []ChunkInfo
	Headings  []Heading
	Anchors   map[string]int // non-heading element ids (footnotes) -> chunk
	ParseTime time.Duration

	chunks []chunk
	refs   []parser.Reference // section mode: all link reference definitions

	mu    sync.Mutex
	html  map[int]string
	order []int // cache insertion order, for eviction

	textOnce sync.Once
	text     []string // folded plain text per chunk, built on first search
}

// Load reads and parses a file.
func Load(path string) (*Doc, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	src, err := io.ReadAll(io.LimitReader(f, MaxFileBytes+1))
	if err != nil {
		return nil, err
	}
	if len(src) > MaxFileBytes {
		return nil, fmt.Errorf("the file is larger than %d MB", MaxFileBytes>>20)
	}
	return Parse(src), nil
}

// Parse parses markdown source.
func Parse(src []byte) *Doc {
	start := time.Now()
	d := &Doc{
		Source:  normaliseNewlines(src),
		Anchors: make(map[string]int),
		html:    make(map[int]string),
	}
	if len(d.Source) <= wholeParseLimit {
		d.parseWhole()
	} else {
		d.parseSections()
	}
	for _, c := range d.chunks {
		seg := d.Source[c.start:c.end]
		d.Chunks = append(d.Chunks, ChunkInfo{Bytes: len(seg), Lines: bytes.Count(seg, []byte{'\n'}) + 1})
	}
	d.ParseTime = time.Since(start)
	return d
}

func normaliseNewlines(src []byte) []byte {
	if bytes.IndexByte(src, '\r') < 0 {
		return src
	}
	src = bytes.ReplaceAll(src, []byte("\r\n"), []byte("\n"))
	return bytes.ReplaceAll(src, []byte("\r"), []byte("\n"))
}

// parseWhole parses the document in one go and groups top-level nodes.
func (d *Doc) parseWhole() {
	root := markdown.Parser().Parse(text.NewReader(d.Source))
	var cur []ast.Node
	chunkStart := 0
	depth := 0 // raw HTML container tags left open, e.g. <details>

	emit := func(end int) {
		d.chunks = append(d.chunks, chunk{start: chunkStart, end: end, nodes: cur})
		cur = nil
		chunkStart = end
	}
	for n := root.FirstChild(); n != nil; n = n.NextSibling() {
		cur = append(cur, n)
		if n.Kind() == ast.KindHTMLBlock {
			depth = max(0, depth+htmlDepthDelta(blockText(n, d.Source)))
		}
		next := n.NextSibling()
		if next == nil {
			break
		}
		ns := nodeStart(next)
		if ns <= chunkStart {
			continue // unknown position, keep going
		}
		size := ns - chunkStart
		if size >= targetChunkBytes && (depth == 0 || size >= maxHoldBytes) {
			emit(ns)
			depth = 0
		}
	}
	emit(len(d.Source))

	slugs := newSlugger()
	for ci, c := range d.chunks {
		for _, top := range c.nodes {
			_ = ast.Walk(top, func(n ast.Node, entering bool) (ast.WalkStatus, error) {
				if !entering {
					return ast.WalkContinue, nil
				}
				switch n := n.(type) {
				case *ast.Heading:
					d.addHeading(n, ci, slugs)
					return ast.WalkSkipChildren, nil
				case *east.Footnote:
					d.Anchors[fmt.Sprintf("fn:%d", n.Index)] = ci
				case *east.FootnoteLink:
					id := "fnref"
					if n.RefIndex > 0 {
						id += fmt.Sprint(n.RefIndex)
					}
					d.Anchors[fmt.Sprintf("%s:%d", id, n.Index)] = ci
				}
				return ast.WalkContinue, nil
			})
		}
	}
}

// addHeading records a heading, giving it an id unless it has one already.
func (d *Doc) addHeading(h *ast.Heading, ci int, slugs *slugger) string {
	txt := strings.TrimSpace(plainText(h, d.Source))
	id := ""
	if v, ok := h.AttributeString("id"); ok {
		if b, ok := v.([]byte); ok && len(b) > 0 {
			id = string(b)
			slugs.reserve(id)
		}
	}
	if id == "" {
		id = slugs.slug(txt)
		h.SetAttributeString("id", []byte(id))
	}
	d.Headings = append(d.Headings, Heading{Level: h.Level, Text: txt, ID: id, Chunk: ci})
	return id
}

type sectionHeading struct {
	level int
	text  string
	id    string // explicit {#id}, if any
}

// parseSections splits a large document and scans the sections in parallel.
func (d *Doc) parseSections() {
	for _, b := range splitSections(d.Source, targetChunkBytes) {
		d.chunks = append(d.chunks, chunk{start: b[0], end: b[1]})
	}
	heads := make([][]sectionHeading, len(d.chunks))
	refs := make([][]parser.Reference, len(d.chunks))
	d.forEachChunk(func(i int) {
		c := d.chunks[i]
		pc := parser.NewContext()
		root := markdown.Parser().Parse(text.NewReader(d.Source[c.start:c.end]), parser.WithContext(pc))
		src := d.Source[c.start:c.end]
		walkHeadings(root, func(h *ast.Heading) {
			sh := sectionHeading{level: h.Level, text: strings.TrimSpace(plainText(h, src))}
			if v, ok := h.AttributeString("id"); ok {
				if b, ok := v.([]byte); ok {
					sh.id = string(b)
				}
			}
			heads[i] = append(heads[i], sh)
		})
		refs[i] = pc.References()
	})

	slugs := newSlugger()
	for ci := range d.chunks {
		for _, sh := range heads[ci] {
			id := sh.id
			if id != "" {
				slugs.reserve(id)
			} else {
				id = slugs.slug(sh.text)
			}
			d.chunks[ci].ids = append(d.chunks[ci].ids, id)
			d.Headings = append(d.Headings, Heading{Level: sh.level, Text: sh.text, ID: id, Chunk: ci})
		}
		d.refs = append(d.refs, refs[ci]...)
	}
}

// walkHeadings visits headings without descending into inline content,
// which is where most of a walk's time would go.
func walkHeadings(n ast.Node, fn func(*ast.Heading)) {
	for c := n.FirstChild(); c != nil; c = c.NextSibling() {
		switch c.Kind() {
		case ast.KindHeading:
			fn(c.(*ast.Heading))
		case ast.KindBlockquote, ast.KindList, ast.KindListItem:
			walkHeadings(c, fn)
		}
	}
}

// forEachChunk runs fn for every chunk index across all CPUs.
func (d *Doc) forEachChunk(fn func(i int)) {
	jobs := make(chan int, len(d.chunks))
	for i := range d.chunks {
		jobs <- i
	}
	close(jobs)
	var wg sync.WaitGroup
	for range runtime.NumCPU() {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := range jobs {
				fn(i)
			}
		}()
	}
	wg.Wait()
}

// parseSection parses one section with the document's link references and
// heading ids applied.
func (d *Doc) parseSection(i int) (ast.Node, []byte) {
	c := d.chunks[i]
	src := d.Source[c.start:c.end]
	pc := parser.NewContext()
	for _, r := range d.refs {
		pc.AddReference(r)
	}
	root := markdown.Parser().Parse(text.NewReader(src), parser.WithContext(pc))
	k := 0
	walkHeadings(root, func(h *ast.Heading) {
		if k < len(c.ids) {
			h.SetAttributeString("id", []byte(c.ids[k]))
		}
		k++
	})
	return root, src
}

// ChunkHTML returns sanitised HTML for chunk i, rendering it on first use.
func (d *Doc) ChunkHTML(i int) (string, error) {
	if i < 0 || i >= len(d.chunks) {
		return "", fmt.Errorf("chunk %d out of range", i)
	}
	d.mu.Lock()
	if h, ok := d.html[i]; ok {
		d.mu.Unlock()
		return h, nil
	}
	d.mu.Unlock()

	var buf bytes.Buffer
	r := markdown.Renderer()
	if d.isWhole() {
		for _, n := range d.chunks[i].nodes {
			if err := r.Render(&buf, d.Source, n); err != nil {
				return "", err
			}
		}
	} else {
		root, src := d.parseSection(i)
		if err := r.Render(&buf, src, root); err != nil {
			return "", err
		}
	}
	h := sanitise(buf.Bytes())

	d.mu.Lock()
	if _, ok := d.html[i]; !ok {
		d.html[i] = h
		d.order = append(d.order, i)
		if !d.isWhole() && len(d.order) > htmlCacheLimit {
			delete(d.html, d.order[0])
			d.order = d.order[1:]
		}
	}
	d.mu.Unlock()
	return h, nil
}

func (d *Doc) isWhole() bool { return len(d.Source) <= wholeParseLimit }

// Search returns the number of case-insensitive matches of q in each chunk.
func (d *Doc) Search(q string) []int {
	q = foldText(q)
	counts := make([]int, len(d.chunks))
	if q == "" {
		return counts
	}
	d.textOnce.Do(d.buildText)
	for i, t := range d.text {
		counts[i] = strings.Count(t, q)
	}
	return counts
}

func (d *Doc) buildText() {
	d.text = make([]string, len(d.chunks))
	d.forEachChunk(func(i int) {
		var b strings.Builder
		if d.isWhole() {
			for _, n := range d.chunks[i].nodes {
				searchText(&b, n, d.Source)
			}
		} else {
			root, src := d.parseSection(i)
			searchText(&b, root, src)
		}
		d.text[i] = foldText(b.String())
	})
}

// foldText lower-cases and collapses whitespace so Go and DOM text compare
// the same way. The frontend applies the same folding.
func foldText(s string) string {
	return strings.Join(strings.Fields(strings.ToLower(s)), " ")
}
