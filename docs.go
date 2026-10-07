package main

import (
	"errors"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/penguin97040/md-viewer/internal/doc"
	"github.com/penguin97040/md-viewer/internal/watch"
)

// DocInfo is everything the viewer needs to lay out a document.
type DocInfo struct {
	ID       int             `json:"id"`
	Path     string          `json:"path"`
	Name     string          `json:"name"`
	Base     string          `json:"base"` // URL for resolving relative links and images
	Size     int             `json:"size"`
	ParseMs  float64         `json:"parseMs"`
	Chunks   []doc.ChunkInfo `json:"chunks"`
	Headings []doc.Heading   `json:"headings"`
	Anchors  map[string]int  `json:"anchors"`
}

// entry is one open document (one tab). Its id changes on every reload so
// the frontend never mixes chunks from two versions of a file.
type entry struct {
	id      int
	path    string
	doc     *doc.Doc
	watcher *watch.Watcher
}

// store holds the open documents. It has no Wails dependencies so it can be
// tested on its own.
type store struct {
	mu   sync.Mutex
	docs map[int]*entry
	next int
	live bool // live reload on

	// onChange is called (without the lock held) after a live reload.
	onChange func(old int, info *DocInfo)
}

func newStore(live bool, onChange func(int, *DocInfo)) *store {
	return &store{docs: make(map[int]*entry), live: live, onChange: onChange}
}

// open parses a file into a new entry.
func (s *store) open(path string) (*DocInfo, error) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return nil, err
	}
	st, err := os.Stat(abs)
	if err != nil {
		return nil, err
	}
	if st.IsDir() {
		return nil, errors.New("that is a folder, not a file")
	}
	d, err := doc.Load(abs)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.next++
	e := &entry{id: s.next, path: abs, doc: d}
	s.docs[e.id] = e
	s.syncWatcher(e)
	return e.info(), nil
}

// close forgets a document and stops watching it.
func (s *store) close(id int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if e, ok := s.docs[id]; ok {
		e.watcher.Close()
		delete(s.docs, id)
	}
}

// closeAll stops every watcher.
func (s *store) closeAll() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for id, e := range s.docs {
		e.watcher.Close()
		delete(s.docs, id)
	}
}

// reload parses a document's file again under a new id.
func (s *store) reload(id int) (*DocInfo, error) {
	s.mu.Lock()
	e, ok := s.docs[id]
	s.mu.Unlock()
	if !ok {
		return nil, errors.New("document is no longer open")
	}
	d, err := doc.Load(e.path)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.docs[id] != e {
		return nil, errors.New("document is no longer open")
	}
	return s.replace(e, d), nil
}

// replace swaps in a new parse of an entry. Callers hold s.mu.
func (s *store) replace(e *entry, d *doc.Doc) *DocInfo {
	delete(s.docs, e.id)
	s.next++
	e.id = s.next
	e.doc = d
	s.docs[e.id] = e
	return e.info()
}

// get returns the document with the given id.
func (s *store) get(id int) (*doc.Doc, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	e, ok := s.docs[id]
	if !ok {
		return nil, errors.New("stale document")
	}
	return e.doc, nil
}

// dir returns the folder of a document, or "" if it isn't open.
func (s *store) dir(id int) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if e, ok := s.docs[id]; ok {
		return filepath.Dir(e.path)
	}
	return ""
}

// resolve turns a relative link in document id into an absolute path. Any
// ?query or #fragment is dropped; the frontend handles fragments.
func (s *store) resolve(id int, href string) (string, error) {
	base := s.dir(id)
	if base == "" {
		return "", errors.New("document is no longer open")
	}
	if i := strings.IndexAny(href, "?#"); i >= 0 {
		href = href[:i]
	}
	p, err := url.PathUnescape(href)
	if err != nil {
		return "", err
	}
	p = filepath.FromSlash(p)
	if !filepath.IsAbs(p) {
		p = filepath.Join(base, p)
	}
	return filepath.Clean(p), nil
}

// setLive turns live reload on or off for every document.
func (s *store) setLive(on bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.live = on
	for _, e := range s.docs {
		s.syncWatcher(e)
	}
}

// syncWatcher starts or stops an entry's watcher to match s.live. Callers
// hold s.mu.
func (s *store) syncWatcher(e *entry) {
	if !s.live {
		e.watcher.Close()
		e.watcher = nil
		return
	}
	if e.watcher != nil {
		return
	}
	w, err := watch.New(e.path, func() { s.fileChanged(e) })
	if err == nil {
		e.watcher = w
	}
}

// fileChanged reloads after an edit on disk. Editors that save by renaming
// can leave the file briefly missing, so a failed read is retried once.
func (s *store) fileChanged(e *entry) {
	d, err := doc.Load(e.path)
	if err != nil {
		time.Sleep(300 * time.Millisecond)
		if d, err = doc.Load(e.path); err != nil {
			return
		}
	}
	s.mu.Lock()
	if s.docs[e.id] != e {
		s.mu.Unlock()
		return // closed meanwhile
	}
	old := e.id
	info := s.replace(e, d)
	s.mu.Unlock()
	if s.onChange != nil {
		s.onChange(old, info)
	}
}

func (e *entry) info() *DocInfo {
	d := e.doc
	return &DocInfo{
		ID:       e.id,
		Path:     e.path,
		Name:     filepath.Base(e.path),
		Base:     localURL(filepath.Dir(e.path)) + "/",
		Size:     len(d.Source),
		ParseMs:  float64(d.ParseTime.Microseconds()) / 1000,
		Chunks:   d.Chunks,
		Headings: d.Headings,
		Anchors:  d.Anchors,
	}
}
