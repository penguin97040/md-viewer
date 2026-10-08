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
	"github.com/penguin97040/md-viewer/internal/store"
)

const appName = "MD Viewer"

// App is bound to the frontend. Each open tab is a document in the store.
type App struct {
	store *store.Store

	mu         sync.Mutex
	saveMu     sync.Mutex      // orders persistence and its runtime effects
	ctx        context.Context // nil until startup, which Wails runs in a goroutine
	ready      bool            // the frontend has asked for its start files
	startFiles []string        // from the command line, a second launch or the OS
	settings   settings.Settings
}

func NewApp(startFiles []string, s settings.Settings) *App {
	a := &App{startFiles: startFiles, settings: s}
	a.store = store.New(s.LiveReload, func(info *store.Info) {
		if ctx := a.context(); ctx != nil {
			runtime.EventsEmit(ctx, "doc:changed", info)
		}
	})
	return a
}

// context returns the Wails context, or nil before startup.
func (a *App) context() context.Context {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.ctx
}

func (a *App) startup(ctx context.Context) {
	a.mu.Lock()
	a.ctx = ctx
	var queued []string
	if a.ready {
		queued, a.startFiles = a.startFiles, nil
	}
	a.mu.Unlock()
	a.openPaths(queued)
}

func (a *App) shutdown(context.Context) {
	a.store.CloseAll()
}

// openPaths hands files opened from outside (a second launch, the macOS
// open-file event) to the frontend, or queues them until both it and the
// Wails context are ready.
func (a *App) openPaths(paths []string) {
	if len(paths) == 0 {
		return
	}
	a.mu.Lock()
	ctx := a.ctx
	if !a.ready || ctx == nil {
		a.startFiles = append(a.startFiles, paths...)
		a.mu.Unlock()
		return
	}
	a.mu.Unlock()
	runtime.EventsEmit(ctx, "open:paths", paths)
	runtime.WindowUnminimise(ctx)
	runtime.WindowShow(ctx)
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
	if len(paths) > 0 {
		a.openPaths(paths)
		return
	}
	// Just bring the window forward. Before startup it is about to appear
	// anyway, and the runtime can't be called yet.
	if ctx := a.context(); ctx != nil {
		runtime.WindowUnminimise(ctx)
		runtime.WindowShow(ctx)
	}
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
// cancelled. dirOf is the key of an open document whose folder to start in.
func (a *App) PickFile(dirOf int) (string, error) {
	return runtime.OpenFileDialog(a.context(), runtime.OpenDialogOptions{
		Title:            "Open markdown file",
		DefaultDirectory: a.store.Dir(dirOf),
		Filters: []runtime.FileFilter{
			{DisplayName: "Markdown (*.md, *.markdown)", Pattern: "*.md;*.markdown;*.mdown;*.mkd;*.mkdn;*.mdx;*.txt"},
			{DisplayName: "All files", Pattern: "*.*"},
		},
	})
}

// Open parses a file as a new document.
func (a *App) Open(path string) (*store.Info, error) { return a.store.Open(path) }

// Close forgets a document when its tab closes.
func (a *App) Close(key int) { a.store.Close(key) }

// Reload parses a document's file again. The result has a new id.
func (a *App) Reload(key int) (*store.Info, error) { return a.store.Reload(key) }

// ResolveLink turns a relative link in a document into an absolute path.
func (a *App) ResolveLink(key int, href string) (string, error) { return a.store.Resolve(key, href) }

// Chunk returns the HTML of chunk i of document id.
func (a *App) Chunk(id, i int) (string, error) {
	d, err := a.store.Get(id)
	if err != nil {
		return "", err
	}
	return d.ChunkHTML(i)
}

// Search returns match counts per chunk.
func (a *App) Search(id int, q string) ([]int, error) {
	d, err := a.store.Get(id)
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
	runtime.WindowSetTitle(a.context(), t)
}

// Settings returns the saved settings.
func (a *App) Settings() settings.Settings {
	a.mu.Lock()
	defer a.mu.Unlock()
	return a.settings
}

// SaveSettings stores settings and applies the parts Go owns.
func (a *App) SaveSettings(s settings.Settings) error {
	a.saveMu.Lock()
	defer a.saveMu.Unlock()
	s = s.Clean()
	if err := settings.Save(s); err != nil {
		return err
	}
	a.mu.Lock()
	themeChanged := s.Theme != a.settings.Theme
	liveChanged := s.LiveReload != a.settings.LiveReload
	a.settings = s
	a.mu.Unlock()
	if liveChanged {
		a.store.SetLive(s.LiveReload)
	}
	if themeChanged {
		if ctx := a.context(); ctx != nil {
			if s.Theme == "light" {
				runtime.WindowSetLightTheme(ctx)
			} else {
				runtime.WindowSetDarkTheme(ctx)
			}
		}
	}
	return nil
}

// OpenURL opens a web link in the system browser.
func (a *App) OpenURL(u string) {
	pu, err := url.Parse(u)
	if err != nil {
		return
	}
	switch strings.ToLower(pu.Scheme) {
	case "http", "https":
		if pu.Host == "" {
			return
		}
	case "mailto":
	default:
		return
	}
	runtime.BrowserOpenURL(a.context(), pu.String())
}

// Print opens the system print dialog, from which a PDF can be saved. On
// Windows and Linux this is window.print(); macOS uses a native print
// operation because its web view ignores window.print().
func (a *App) Print() { runtime.WindowPrint(a.context()) }

// Version returns the build version.
func (a *App) Version() string { return version }
