package doc

import "bytes"

const (
	// forceSplitBytes splits a section at the next blank line even when the
	// usual safety rules say no, so one section never grows unbounded.
	forceSplitBytes = 1 << 20
	// panicSplitBytes splits at any line, e.g. inside a fence never closed.
	panicSplitBytes = 4 << 20
)

// splitSections cuts src into [start, end) ranges of at least target bytes.
// A cut is only made before a line that certainly begins a new top-level
// block: it follows a blank line, starts in column 0, is not a list item or
// block quote, and is not inside a fenced code block, maths block or raw
// HTML block.
func splitSections(src []byte, target int) [][2]int {
	var out [][2]int
	start := 0
	var fence []byte   // the fence marker while inside a code block
	var htmlEnd []byte // the closing marker while inside <pre>, <!-- etc.
	inMath := false
	depth := 0 // open raw HTML container tags, e.g. <details>
	prevBlank := true

	cut := func(at int) {
		out = append(out, [2]int{start, at})
		start = at
		depth = 0
	}

	for pos := 0; pos < len(src); {
		lineEnd := bytes.IndexByte(src[pos:], '\n')
		next := len(src)
		if lineEnd < 0 {
			lineEnd = len(src)
		} else {
			lineEnd += pos
			next = lineEnd + 1
		}
		line := src[pos:lineEnd]
		trimmed := bytes.TrimLeft(line, " \t")
		blank := len(bytes.TrimSpace(trimmed)) == 0
		size := pos - start

		switch {
		case fence != nil || htmlEnd != nil || inMath:
			if size >= panicSplitBytes {
				cut(pos)
				fence, htmlEnd, inMath = nil, nil, false
				break
			}
			if fence != nil {
				if closesFence(trimmed, fence) {
					fence = nil
				}
			} else if htmlEnd != nil {
				if bytes.Contains(bytes.ToLower(line), htmlEnd) {
					htmlEnd = nil
				}
			} else if bytes.HasSuffix(bytes.TrimSpace(line), []byte("$$")) {
				inMath = false
			}
		default:
			if prevBlank && !blank && size >= target &&
				((depth == 0 && safeStart(line)) || size >= forceSplitBytes) {
				cut(pos)
			}
			if f := openFence(trimmed); f != nil {
				fence = f
			} else if e := htmlBlockEnd(trimmed); e != nil && !bytes.Contains(bytes.ToLower(trimmed[4:]), e) {
				htmlEnd = e
			} else if t := bytes.TrimSpace(line); bytes.HasPrefix(t, []byte("$$")) && (len(t) == 2 || !bytes.HasSuffix(t[2:], []byte("$$"))) {
				inMath = true
			}
			if len(trimmed) > 0 && trimmed[0] == '<' {
				depth = max(0, depth+htmlDepthDelta(line))
			}
		}
		prevBlank = blank
		pos = next
	}
	if start < len(src) || len(out) == 0 {
		out = append(out, [2]int{start, len(src)})
	}
	return out
}

// safeStart reports whether a line in column 0 begins a fresh block rather
// than continuing a list or block quote.
func safeStart(line []byte) bool {
	if len(line) == 0 {
		return false
	}
	switch line[0] {
	case ' ', '\t', '>':
		return false
	case '-', '*', '+':
		return len(line) > 1 && line[1] != ' ' && line[1] != '\t'
	}
	i := 0
	for i < len(line) && i < 10 && line[i] >= '0' && line[i] <= '9' {
		i++
	}
	if i > 0 && i < len(line) && (line[i] == '.' || line[i] == ')') {
		return false
	}
	return true
}

func openFence(t []byte) []byte {
	if len(t) < 3 || (t[0] != '`' && t[0] != '~') {
		return nil
	}
	n := 0
	for n < len(t) && t[n] == t[0] {
		n++
	}
	if n < 3 || (t[0] == '`' && bytes.IndexByte(t[n:], '`') >= 0) {
		return nil
	}
	return t[:n]
}

func closesFence(t, fence []byte) bool {
	n := 0
	for n < len(t) && t[n] == fence[0] {
		n++
	}
	return n >= len(fence) && len(bytes.TrimSpace(t[n:])) == 0
}

var htmlBlocks = []struct{ open, end string }{
	{"<pre", "</pre>"}, {"<script", "</script>"}, {"<style", "</style>"},
	{"<textarea", "</textarea>"}, {"<!--", "-->"},
}

// htmlBlockEnd returns the end marker for raw HTML blocks that may contain
// blank lines (CommonMark types 1 and 2), or nil.
func htmlBlockEnd(t []byte) []byte {
	if len(t) < 4 || t[0] != '<' {
		return nil
	}
	lower := bytes.ToLower(t[:min(len(t), 10)])
	for _, b := range htmlBlocks {
		if bytes.HasPrefix(lower, []byte(b.open)) {
			return []byte(b.end)
		}
	}
	return nil
}
