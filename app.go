package main

import (
	"context"
	"net/url"
	"path/filepath"
	"strings"
	"sync"

	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/runtime"

	"github.com/penguin97040/md-viewer/internal/settings"
)

const appName = "MD Viewer"

// App is bound to the frontend. Each open tab is a document in the store.
type App struct {
	ctx   context.Context
	store *store

	mu         sync.Mutex
	ready      bool     // the frontend has asked for its start files
	startFiles []string // from the command line, a second launch or the OS
	settings   settings.Settings
}

func NewApp(startFiles []string, s settings.Settings) *App {
	a := &App{startFiles: startFiles, settings: s}
	a.store = newStore(s.LiveReload, func(old int, info *DocInfo) {
		runtime.EventsEmit(a.ctx, "doc:changed", old, info)
	})
	return a
}

func (a *App) startup(ctx context.Context) {
	a.ctx = ctx
}

func (a *App) shutdown(context.Context) {
	a.store.closeAll()
}

// openPaths hands files opened from outside (a second launch, the macOS
// open-file event) to the frontend, or queues them until it is ready.
func (a *App) openPaths(paths []string) {
	if len(paths) == 0 {
		return
	}
	a.mu.Lock()
	if !a.ready {
		a.startFiles = append(a.startFiles, paths...)
		a.mu.Unlock()
		return
	}
	a.mu.Unlock()
	runtime.EventsEmit(a.ctx, "open:paths", paths)
	runtime.WindowUnminimise(a.ctx)
	runtime.WindowShow(a.ctx)
}

func (a *App) openFromOS(path string) { a.openPaths([]string{path}) }

func (a *App) secondInstance(d options.SecondInstanceData) {
	var paths []string
	for _, p := range fileArgs(d.Args) {
		if !filepath.IsAbs(p) {
			p = filepath.Join(d.WorkingDirectory, p)
		}
		paths = append(paths, p)
	}
	if len(paths) == 0 {
		runtime.WindowUnminimise(a.ctx)
		runtime.WindowShow(a.ctx)
		return
	}
	a.openPaths(paths)
}

// Initial returns the files to open at start.
func (a *App) Initial() []string {
	a.mu.Lock()
	defer a.mu.Unlock()
	a.ready = true
	paths := a.startFiles
	a.startFiles = nil
	for i, p := range paths {
		if abs, err := filepath.Abs(p); err == nil {
			paths[i] = abs
		}
	}
	return paths
}

// PickFile asks for a markdown file and returns its path, or "" if
// cancelled. dirOf names an open document whose folder to start in.
func (a *App) PickFile(dirOf int) (string, error) {
	return runtime.OpenFileDialog(a.ctx, runtime.OpenDialogOptions{
		Title:            "Open markdown file",
		DefaultDirectory: a.store.dir(dirOf),
		Filters: []runtime.FileFilter{
			{DisplayName: "Markdown (*.md, *.markdown)", Pattern: "*.md;*.markdown;*.mdown;*.mkd;*.mkdn;*.mdx;*.txt"},
			{DisplayName: "All files", Pattern: "*.*"},
		},
	})
}

// Open parses a file as a new document.
func (a *App) Open(path string) (*DocInfo, error) { return a.store.open(path) }

// Close forgets a document when its tab closes.
func (a *App) Close(id int) { a.store.close(id) }

// Reload parses a document's file again. The result has a new id.
func (a *App) Reload(id int) (*DocInfo, error) { return a.store.reload(id) }

// ResolveLink turns a relative link in document id into an absolute path.
func (a *App) ResolveLink(id int, href string) (string, error) { return a.store.resolve(id, href) }

// Chunk returns the HTML of chunk i of document id.
func (a *App) Chunk(id, i int) (string, error) {
	d, err := a.store.get(id)
	if err != nil {
		return "", err
	}
	return d.ChunkHTML(i)
}

// Search returns match counts per chunk.
func (a *App) Search(id int, q string) ([]int, error) {
	d, err := a.store.get(id)
	if err != nil {
		return nil, err
	}
	return d.Search(q), nil
}

// SetTitle sets the window title from the active tab's file name.
func (a *App) SetTitle(name string) {
	t := appName
	if name != "" {
		t = name + " – " + appName
	}
	runtime.WindowSetTitle(a.ctx, t)
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
	liveChanged := s.LiveReload != a.settings.LiveReload
	a.settings = s
	a.mu.Unlock()
	if liveChanged {
		a.store.setLive(s.LiveReload)
	}
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

// Print opens the system print dialog, from which a PDF can be saved. On
// Windows and Linux this is window.print(); macOS uses a native print
// operation because its web view ignores window.print().
func (a *App) Print() { runtime.WindowPrint(a.ctx) }

// Version returns the build version.
func (a *App) Version() string { return version }
