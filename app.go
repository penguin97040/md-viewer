package main

import (
	"context"
	"errors"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/wailsapp/wails/v2/pkg/runtime"

	"github.com/penguin97040/md-viewer/internal/doc"
	"github.com/penguin97040/md-viewer/internal/settings"
	"github.com/penguin97040/md-viewer/internal/watch"
)

const appName = "MD Viewer"

// App holds the open document and is bound to the frontend.
type App struct {
	ctx context.Context

	mu        sync.Mutex
	ready     bool   // the frontend has asked for its first document
	startFile string // file given on the command line or by the OS
	doc       *doc.Doc
	docID     int
	path      string
	watcher   *watch.Watcher
	settings  settings.Settings
}

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

func NewApp(startFile string, s settings.Settings) *App {
	return &App{startFile: startFile, settings: s}
}

func (a *App) startup(ctx context.Context) {
	a.ctx = ctx
}

func (a *App) shutdown(context.Context) {
	a.mu.Lock()
	a.watcher.Close()
	a.mu.Unlock()
}

// openFromOS handles the macOS open-file event. Before the frontend has
// asked for its first document, the file simply becomes that document.
func (a *App) openFromOS(path string) {
	a.mu.Lock()
	if !a.ready {
		a.startFile = path
		a.mu.Unlock()
		return
	}
	a.mu.Unlock()
	info, err := a.Open(path)
	if err != nil {
		runtime.EventsEmit(a.ctx, "doc:error", err.Error())
		return
	}
	runtime.EventsEmit(a.ctx, "doc:opened", info)
}

// Initial opens the file given on the command line, if any.
func (a *App) Initial() (*DocInfo, error) {
	a.mu.Lock()
	a.ready = true
	p := a.startFile
	a.mu.Unlock()
	if p == "" {
		return nil, nil
	}
	return a.Open(p)
}

// OpenDialog asks for a file and opens it. It returns nil if cancelled.
func (a *App) OpenDialog() (*DocInfo, error) {
	dir := ""
	a.mu.Lock()
	if a.path != "" {
		dir = filepath.Dir(a.path)
	}
	a.mu.Unlock()
	p, err := runtime.OpenFileDialog(a.ctx, runtime.OpenDialogOptions{
		Title:            "Open markdown file",
		DefaultDirectory: dir,
		Filters: []runtime.FileFilter{
			{DisplayName: "Markdown (*.md, *.markdown)", Pattern: "*.md;*.markdown;*.mdown;*.mkd;*.mkdn;*.mdx;*.txt"},
			{DisplayName: "All files", Pattern: "*.*"},
		},
	})
	if err != nil || p == "" {
		return nil, err
	}
	return a.Open(p)
}

// Open parses a file and makes it the current document.
func (a *App) Open(path string) (*DocInfo, error) {
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

	a.mu.Lock()
	defer a.mu.Unlock()
	if abs != a.path {
		a.watcher.Close()
		a.watcher = nil
	}
	a.doc, a.path = d, abs
	a.docID++
	a.syncWatcher()
	runtime.WindowSetTitle(a.ctx, filepath.Base(abs)+" – "+appName)
	return a.info(), nil
}

// OpenLink opens a relative link from the current document, e.g. to
// another markdown file. Any #fragment is left for the frontend.
func (a *App) OpenLink(href string) (*DocInfo, error) {
	a.mu.Lock()
	base := a.path
	a.mu.Unlock()
	if base == "" {
		return nil, errors.New("no document open")
	}
	if i := strings.IndexAny(href, "?#"); i >= 0 {
		href = href[:i]
	}
	p, err := url.PathUnescape(href)
	if err != nil {
		return nil, err
	}
	p = filepath.FromSlash(p)
	if !filepath.IsAbs(p) {
		p = filepath.Join(filepath.Dir(base), p)
	}
	return a.Open(p)
}

// Reload parses the current file again.
func (a *App) Reload() (*DocInfo, error) {
	a.mu.Lock()
	p := a.path
	a.mu.Unlock()
	if p == "" {
		return nil, nil
	}
	return a.Open(p)
}

// Chunk returns the HTML of chunk i of document id.
func (a *App) Chunk(id, i int) (string, error) {
	d, err := a.current(id)
	if err != nil {
		return "", err
	}
	return d.ChunkHTML(i)
}

// Search returns match counts per chunk.
func (a *App) Search(id int, q string) ([]int, error) {
	d, err := a.current(id)
	if err != nil {
		return nil, err
	}
	return d.Search(q), nil
}

// Settings returns the saved settings.
func (a *App) Settings() settings.Settings {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.settings
}

// SaveSettings stores settings and applies the parts Go owns.
func (a *App) SaveSettings(s settings.Settings) error {
	s = s.Clean()
	a.mu.Lock()
	themeChanged := s.Theme != a.settings.Theme
	a.settings = s
	a.syncWatcher()
	a.mu.Unlock()
	if themeChanged {
		if s.Theme == "light" {
			runtime.WindowSetLightTheme(a.ctx)
		} else {
			runtime.WindowSetDarkTheme(a.ctx)
		}
	}
	return settings.Save(s)
}

// OpenURL opens a web link in the system browser.
func (a *App) OpenURL(u string) {
	pu, err := url.Parse(u)
	if err != nil {
		return
	}
	switch strings.ToLower(pu.Scheme) {
	case "http", "https", "mailto":
		runtime.BrowserOpenURL(a.ctx, u)
	}
}

// Version returns the build version.
func (a *App) Version() string { return version }

func (a *App) current(id int) (*doc.Doc, error) {
	a.mu.Lock()
	defer a.mu.Unlock()
	if a.doc == nil || id != a.docID {
		return nil, errors.New("stale document")
	}
	return a.doc, nil
}

// info builds a DocInfo for the current document. Callers hold a.mu.
func (a *App) info() *DocInfo {
	d := a.doc
	return &DocInfo{
		ID:       a.docID,
		Path:     a.path,
		Name:     filepath.Base(a.path),
		Base:     localURL(filepath.Dir(a.path)) + "/",
		Size:     len(d.Source),
		ParseMs:  float64(d.ParseTime.Microseconds()) / 1000,
		Chunks:   d.Chunks,
		Headings: d.Headings,
		Anchors:  d.Anchors,
	}
}

// syncWatcher starts or stops live reload to match settings. Callers hold a.mu.
func (a *App) syncWatcher() {
	want := a.settings.LiveReload && a.path != ""
	if !want {
		a.watcher.Close()
		a.watcher = nil
		return
	}
	if a.watcher != nil {
		return
	}
	path := a.path
	w, err := watch.New(path, func() { a.fileChanged(path) })
	if err == nil {
		a.watcher = w
	}
}

// fileChanged reloads after an edit on disk. Editors that save by renaming
// can leave the file briefly missing, so a failed read is retried once.
func (a *App) fileChanged(path string) {
	d, err := doc.Load(path)
	if err != nil {
		time.Sleep(300 * time.Millisecond)
		if d, err = doc.Load(path); err != nil {
			return
		}
	}
	a.mu.Lock()
	if a.path != path {
		a.mu.Unlock()
		return
	}
	a.doc = d
	a.docID++
	info := a.info()
	a.mu.Unlock()
	runtime.EventsEmit(a.ctx, "doc:changed", info)
}
