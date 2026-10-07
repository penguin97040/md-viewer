package doc

import (
	"strconv"
	"strings"
	"unicode"
)

// slugger makes GitHub-style heading ids: lower case, punctuation removed,
// spaces turned into hyphens, duplicates suffixed with -1, -2, ...
type slugger struct {
	seen map[string]bool
	next map[string]int // last suffix used per base, so repeats stay O(1)
}

func newSlugger() *slugger {
	return &slugger{seen: make(map[string]bool), next: make(map[string]int)}
}

func (s *slugger) reserve(id string) { s.seen[id] = true }

func (s *slugger) slug(text string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(text) {
		switch {
		case unicode.IsLetter(r) || unicode.IsNumber(r) || r == '_' || r == '-':
			b.WriteRune(r)
		case r == ' ':
			b.WriteByte('-')
		}
	}
	base := b.String()
	if base == "" {
		base = "section"
	}
	id := base
	if s.seen[id] {
		n := s.next[base]
		for s.seen[id] {
			n++
			id = base + "-" + strconv.Itoa(n)
		}
		s.next[base] = n
	}
	s.seen[id] = true
	return id
}
