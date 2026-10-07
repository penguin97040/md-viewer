// Package store holds the open documents, one per tab, and reloads them
// when their files change. It has no Wails dependencies, so the app and the
// dev server share it and it can be tested on its own.
package store

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"net/url"
	"os"
	"path/filepath"
	goruntime "runtime"
	"strings"
	"sync"
	"time"

	"github.com/penguin97040/md-viewer/internal/doc"
	"github.com/penguin97040/md-viewer/internal/watch"
)

// Info is everything the viewer needs to lay out a document.
type Info struct {
	ID       int             `json:"id"`  // this parse; changes on every reload
	Key      int             `json:"key"` // the open document; stays the same across reloads
	Web      string          `json:"web"` // URL prefix for web images, once the reader allows them
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
// the frontend never mixes chunks from two versions of a file; its key
// doesn't, so a tab can always close or reload it.
type entry struct {
	key     int
	id      int
	ticket  int // when the parse in doc started reading the file
	path    string
	doc     *doc.Doc
	watcher *watch.Watcher
}

// Store holds the open documents.
type Store struct {
	mu     sync.Mutex
	byKey  map[int]*entry
	byID   map[int]*entry
	next   int // last id or key handed out
	ticket int // last load started
	live   bool
	web    string // secret path that web images are fetched through

	// onChange is called (without the lock held) after a live reload.
	onChange func(info *Info)
}

// New returns an empty store. With live on, files are watched and onChange
// gets each new parse.
func New(live bool, onChange func(*Info)) *Store {
	t := make([]byte, 16)
	if _, err := rand.Read(t); err != nil {
		panic(err)
	}
	return &Store{
		byKey: make(map[int]*entry), byID: make(map[int]*entry), live: live, onChange: onChange,
		web: "/web/" + hex.EncodeToString(t),
	}
}

// WebPath returns the path that web images are fetched through. It holds a
// random token, so a document (which can't run script) can't learn it and
// load web images before the reader agrees.
func (s *Store) WebPath() string { return s.web }

// startLoad returns a ticket that orders loads by when they began reading.
func (s *Store) startLoad() int {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.ticket++
	return s.ticket
}

// Open parses a file as a new document.
func (s *Store) Open(path string) (*Info, error) {
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
	if !st.Mode().IsRegular() {
		return nil, errors.New("that is not an ordinary file")
	}
	t := s.startLoad()
	d, err := doc.Load(abs)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.next++
	e := &entry{key: s.next, id: s.next, ticket: t, path: abs, doc: d}
	s.byKey[e.key] = e
	s.byID[e.id] = e
	s.syncWatcher(e)
	return e.info(s.web), nil
}

// Close forgets a document and stops watching it.
func (s *Store) Close(key int) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if e, ok := s.byKey[key]; ok {
		s.drop(e)
	}
}

// CloseAll stops every watcher.
func (s *Store) CloseAll() {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, e := range s.byKey {
		s.drop(e)
	}
}

// drop removes an entry. Callers hold s.mu.
func (s *Store) drop(e *entry) {
	e.watcher.Close()
	e.watcher = nil
	delete(s.byKey, e.key)
	delete(s.byID, e.id)
}

// Reload parses a document's file again. The result has a new id, unless a
// live reload that read the file later got there first, in which case that
// newer parse is returned.
func (s *Store) Reload(key int) (*Info, error) {
	s.mu.Lock()
	e, ok := s.byKey[key]
	s.mu.Unlock()
	if !ok {
		return nil, errors.New("document is no longer open")
	}
	t := s.startLoad()
	d, err := doc.Load(e.path)
	if err != nil {
		return nil, err
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.byKey[key] != e {
		return nil, errors.New("document is no longer open")
	}
	info, _ := s.install(e, d, t)
	return info, nil
}

// install swaps in a new parse of an open entry under a new id, unless the
// entry holds a parse that began reading the file later. It returns the
// entry's info and whether d went in. Callers hold s.mu.
func (s *Store) install(e *entry, d *doc.Doc, ticket int) (*Info, bool) {
	if ticket < e.ticket {
		return e.info(s.web), false
	}
	delete(s.byID, e.id)
	s.next++
	e.id = s.next
	e.doc = d
	e.ticket = ticket
	s.byID[e.id] = e
	return e.info(s.web), true
}

// Get returns the parse with the given id.
func (s *Store) Get(id int) (*doc.Doc, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	e, ok := s.byID[id]
	if !ok {
		return nil, errors.New("stale document")
	}
	return e.doc, nil
}

// Dir returns the folder of a document, or "" if it isn't open.
func (s *Store) Dir(key int) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	if e, ok := s.byKey[key]; ok {
		return filepath.Dir(e.path)
	}
	return ""
}

// Resolve turns a relative link in a document into an absolute path. Any
// ?query or #fragment is dropped; the frontend handles fragments.
func (s *Store) Resolve(key int, href string) (string, error) {
	base := s.Dir(key)
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
	p = filepath.Clean(p)
	if !s.Reachable(p) {
		return "", errors.New("links to files on other computers are not followed")
	}
	return p, nil
}

// Reachable reports whether a document may read path p, for an image or
// link. On Windows, reading \\server\share\... connects to that server and
// signs in as the user, which hands the server a hash of the user's
// password. So a document may only reach a network share that an open
// document is itself on. Device paths (\\?\..., \\.\...) are always refused.
func (s *Store) Reachable(p string) bool {
	return goruntime.GOOS != "windows" || s.reachable(p)
}

func (s *Store) reachable(p string) bool {
	share := windowsShare(p)
	if share == "" {
		return true
	}
	if share == deviceShare {
		return false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, e := range s.byKey {
		if windowsShare(e.path) == share {
			return true
		}
	}
	return false
}

const deviceShare = `\\?`

// windowsShare returns \\server\share (lower-cased) for a Windows network
// path, deviceShare for a device path, or "" for a local one. Either slash
// is accepted, as Windows does.
func windowsShare(p string) string {
	p = strings.ReplaceAll(p, "/", `\`)
	if !strings.HasPrefix(p, `\\`) {
		return ""
	}
	parts := strings.SplitN(p[2:], `\`, 3)
	if parts[0] == "?" || parts[0] == "." || parts[0] == "" || len(parts) < 2 || parts[1] == "" {
		return deviceShare
	}
	return strings.ToLower(`\\` + parts[0] + `\` + parts[1])
}

// SetLive turns live reload on or off for every document.
func (s *Store) SetLive(on bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.live = on
	for _, e := range s.byKey {
		s.syncWatcher(e)
	}
}

// syncWatcher starts or stops an entry's watcher to match s.live. Callers
// hold s.mu.
func (s *Store) syncWatcher(e *entry) {
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
func (s *Store) fileChanged(e *entry) {
	t := s.startLoad()
	d, err := doc.Load(e.path)
	if err != nil {
		time.Sleep(300 * time.Millisecond)
		t = s.startLoad()
		if d, err = doc.Load(e.path); err != nil {
			return
		}
	}
	s.mu.Lock()
	var info *Info
	ok := s.byKey[e.key] == e // not closed meanwhile
	if ok {
		info, ok = s.install(e, d, t)
	}
	s.mu.Unlock()
	if ok && s.onChange != nil {
		s.onChange(info)
	}
}

func (e *entry) info(web string) *Info {
	d := e.doc
	return &Info{
		ID:       e.id,
		Key:      e.key,
		Web:      web,
		Path:     e.path,
		Name:     filepath.Base(e.path),
		Base:     LocalURL(filepath.Dir(e.path)) + "/",
		Size:     len(d.Source),
		ParseMs:  float64(d.ParseTime.Microseconds()) / 1000,
		Chunks:   d.Chunks,
		Headings: d.Headings,
		Anchors:  d.Anchors,
	}
}

var imageTypes = map[string]string{
	".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif",
	".webp": "image/webp", ".svg": "image/svg+xml", ".bmp": "image/bmp", ".ico": "image/x-icon",
	".avif": "image/avif",
}

// ImageType returns the content type of an image file, from its name.
// Only images are served from /local/, so a document can't pull other local
// files into the page.
func ImageType(p string) (string, bool) {
	ct, ok := imageTypes[strings.ToLower(filepath.Ext(p))]
	return ct, ok
}

// LocalURL turns a file system path into a /local/ URL, which the app's
// asset handler serves.
func LocalURL(p string) string {
	p = strings.TrimPrefix(filepath.ToSlash(p), "/")
	return (&url.URL{Path: "/local/" + p}).EscapedPath()
}

// LocalPath reverses LocalURL, given the decoded URL path.
func LocalPath(urlPath string) string {
	p := strings.TrimPrefix(urlPath, "/local/")
	if goruntime.GOOS != "windows" || strings.HasPrefix(p, "/") {
		p = "/" + p // unix absolute path, or a Windows UNC path (//server/share)
	}
	return filepath.FromSlash(p)
}
