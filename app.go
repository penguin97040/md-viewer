package main

import (
	"context"
	"encoding/json"
	"errors"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/penguin97040/md-viewer/internal/native"
	"github.com/penguin97040/md-viewer/internal/session"
	"github.com/penguin97040/md-viewer/internal/settings"
	"github.com/penguin97040/md-viewer/internal/store"
	"github.com/wailsapp/wails/v2/pkg/runtime"
)

const appName = "MD Viewer"

// App keeps native UI in the window process and document state in the session.
type App struct {
	dragCancel   context.CancelFunc
	dragReady    bool
	started      chan struct{}
	remote       *session.Client
	position     *session.Position
	mu           sync.Mutex
	ctx          context.Context
	cancel       context.CancelFunc
	ready        bool
	closing      bool
	initialMu    sync.Mutex
	shutdownOnce sync.Once
}

func NewApp(remote *session.Client, position *session.Position) *App {
	return &App{remote: remote, position: position, started: make(chan struct{})}
}
func (a *App) context() context.Context { a.mu.Lock(); defer a.mu.Unlock(); return a.ctx }
func (a *App) startup(ctx context.Context) {
	a.mu.Lock()
	if a.closing {
		close(a.started)
		a.mu.Unlock()
		return
	}
	a.ctx = ctx
	a.mu.Unlock()
	native.Init()
	if a.position != nil {
		placeWindow(ctx, *a.position)
	}
	close(a.started)
}
func (a *App) shutdown(context.Context) {
	a.shutdownOnce.Do(func() {
		a.mu.Lock()
		a.closing = true
		if a.dragCancel != nil {
			a.dragCancel()
		}
		if a.cancel != nil {
			a.cancel()
		}
		a.mu.Unlock()
		native.End()
		if a.remote != nil {
			_ = a.remote.Unregister()
		}
	})
}

func (a *App) beforeClose(ctx context.Context) bool {
	a.shutdown(ctx)
	return false
}

func (a *App) Initial() ([]string, error) {
	a.initialMu.Lock()
	defer a.initialMu.Unlock()
	a.mu.Lock()
	if a.closing {
		a.mu.Unlock()
		return nil, errors.New("window is closing")
	}
	if a.ready {
		a.mu.Unlock()
		return nil, nil
	}
	a.mu.Unlock()
	native.Init()
	if err := a.remote.Register(); err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(context.Background())
	a.mu.Lock()
	if a.closing {
		a.mu.Unlock()
		cancel()
		_ = a.remote.Unregister()
		return nil, errors.New("window is closing")
	}
	a.ready = true
	a.cancel = cancel
	a.mu.Unlock()
	go a.events(ctx)
	return nil, nil
}
func (a *App) events(ctx context.Context) {
	select {
	case <-a.started:
	case <-ctx.Done():
		return
	}
	for {
		events, err := a.remote.Poll(ctx)
		if ctx.Err() != nil {
			return
		}
		if err != nil {
			if c := a.context(); c != nil {
				runtime.EventsEmit(c, "doc:error", "Window coordinator disconnected: "+err.Error())
			}
			return
		}
		for _, event := range events {
			event = a.remote.DecorateEvent(event)
			c := a.context()
			if c == nil {
				continue
			}
			switch event.Name {
			case "window:focus", "open:paths":
				runtime.WindowUnminimise(c)
				runtime.WindowShow(c)
			case "settings:changed":
				s, err := a.remote.Settings()
				if err == nil {
					a.theme(s)
				}
			}
			if event.Name == "tab:hover" || event.Name == "tab:drop" {
				var point session.DragPoint
				b, _ := json.Marshal(event.Data)
				if json.Unmarshal(b, &point) != nil {
					continue
				}
				x, y, valid := native.ClientPoint(point.Position.X, point.Position.Y)
				event.Data = map[string]any{"token": point.Token, "x": x, "y": y, "valid": valid}
			}
			runtime.EventsEmit(c, event.Name, event.Data)
		}
	}
}
func (a *App) openFromOS(path string) { _ = session.Launch([]string{path}) }
func (a *App) PickFile(dirOf int) (string, error) {
	return runtime.OpenFileDialog(a.context(), runtime.OpenDialogOptions{Title: "Open markdown file", DefaultDirectory: a.remote.Dir(dirOf), Filters: []runtime.FileFilter{{DisplayName: "Markdown (*.md, *.markdown)", Pattern: "*.md;*.markdown;*.mdown;*.mkd;*.mkdn;*.mdx;*.txt"}, {DisplayName: "All files", Pattern: "*.*"}}})
}
func (a *App) Open(path string) (*store.Info, error) {
	r, e := a.remote.Open(path)
	if e != nil {
		return nil, e
	}
	return r.Info, nil
}
func (a *App) Close(key int) error                              { return a.remote.Close(key) }
func (a *App) Reload(key int) (*store.Info, error)              { return a.remote.Reload(key) }
func (a *App) ResolveLink(key int, href string) (string, error) { return a.remote.Resolve(key, href) }
func (a *App) Chunk(id, i int) (string, error)                  { return a.remote.Chunk(id, i) }
func (a *App) Search(id int, q string) ([]int, error)           { return a.remote.Search(id, q) }
func (a *App) SetTitle(name string) {
	t := appName
	if name != "" {
		t = name + " – " + appName
	}
	if c := a.context(); c != nil {
		runtime.WindowSetTitle(c, t)
	}
}
func (a *App) Settings() (settings.Settings, error)   { return a.remote.Settings() }
func (a *App) SaveSettings(s settings.Settings) error { return a.remote.SaveSettings(s) }
func (a *App) theme(s settings.Settings) {
	if c := a.context(); c != nil {
		if s.Theme == "light" {
			runtime.WindowSetLightTheme(c)
		} else {
			runtime.WindowSetDarkTheme(c)
		}
	}
}
func (a *App) FocusWindow() error        { return a.remote.Focus() }
func (a *App) TabOrder(keys []int) error { return a.remote.Order(keys) }
func (a *App) BeginTransfer(key int, state session.TabState) (string, error) {
	return a.remote.Begin(key, state)
}
func (a *App) AcceptTransfer(tok string, index int) error { return a.remote.Accept(tok, index) }
func (a *App) AcknowledgeTransfer(tok string, parse int) (*session.Offer, error) {
	a.mu.Lock()
	closing := a.closing
	a.mu.Unlock()
	if closing || !native.Visible() {
		return nil, errors.New("destination window is not visible")
	}
	return a.remote.Ack(tok, parse)
}

// WindowReady proves native visibility independently of a running hidden web view.
func (a *App) WindowReady() error {
	a.mu.Lock()
	closing := a.closing
	c := a.ctx
	a.mu.Unlock()
	if closing || c == nil {
		return errors.New("window is not available")
	}
	ctx, cancel := context.WithTimeout(c, 2*time.Second)
	defer cancel()
	select {
	case <-a.started:
	case <-ctx.Done():
		return errors.New("window startup did not finish")
	}
	runtime.WindowUnminimise(c)
	runtime.WindowShow(c)
	if err := waitVisible(ctx, native.Visible); err != nil {
		return err
	}
	return a.remote.WindowReady()
}

func waitVisible(ctx context.Context, visible func() bool) error {
	tick := time.NewTicker(20 * time.Millisecond)
	defer tick.Stop()
	for {
		if visible() {
			return nil
		}
		select {
		case <-ctx.Done():
			return errors.New("destination window could not be shown")
		case <-tick.C:
		}
	}
}

func (a *App) domReady(ctx context.Context) {
	a.mu.Lock()
	closing := a.closing
	a.mu.Unlock()
	if !closing && a.position != nil {
		runtime.WindowShow(ctx)
	}
}
func (a *App) CancelTransfer(tok string) error           { return a.remote.Cancel(tok) }
func (a *App) DiscardTransfer(tok string, key int) error { return a.remote.Discard(tok, key) }
func (a *App) CloseWindow() {
	if c := a.context(); c != nil {
		a.shutdown(c)
		runtime.Quit(c)
	}
}
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
	if c := a.context(); c != nil {
		runtime.BrowserOpenURL(c, pu.String())
	}
}
func (a *App) Print() {
	if c := a.context(); c != nil {
		runtime.WindowPrint(c)
	}
}
func (a *App) Version() string { return version }

func (a *App) StartTabDrag(tok string, state session.TabState) error {
	a.mu.Lock()
	closing := a.closing
	a.mu.Unlock()
	if closing {
		return errors.New("window is closing")
	}
	var pids []int
	var err error
	if native.DragFeedback {
		pids, err = a.remote.StartFeedback(tok, state)
	} else {
		err = a.remote.Start(tok, state)
	}
	if err != nil {
		return err
	}
	dragReady := native.StartForTab(tok, pids)
	a.mu.Lock()
	if a.closing {
		a.mu.Unlock()
		native.End()
		return errors.New("window is closing")
	}
	if a.dragCancel != nil {
		a.dragCancel()
	}
	ctx, cancel := context.WithCancel(context.Background())
	a.dragCancel = cancel
	a.dragReady = dragReady
	a.mu.Unlock()
	go func() {
		ticker := time.NewTicker(40 * time.Millisecond)
		defer ticker.Stop()
		ticks := 0
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				ticks++
				if native.DragFeedback && ticks%5 == 0 {
					refresh, stop := context.WithTimeout(ctx, 250*time.Millisecond)
					pids, err := a.remote.DragWindows(refresh, tok)
					stop()
					if ctx.Err() != nil {
						return
					}
					if err != nil {
						// Losing feedback never prevents the existing detach path.
						native.UpdateDragWindows(tok, nil)
						return
					}
					native.UpdateDragWindows(tok, pids)
				}
				point := native.Cursor()
				if ctx.Err() != nil {
					return
				}
				if point.Valid {
					if a.remote.Hover(ctx, tok, point.PID, session.Position{X: point.X, Y: point.Y}) != nil {
						return
					}
				}
			}
		}
	}()
	return nil
}
func (a *App) EndTabDrag(tok string, cancelled bool) error {
	a.mu.Lock()
	dragReady := a.dragReady
	a.dragReady = false
	if a.dragCancel != nil {
		a.dragCancel()
		a.dragCancel = nil
	}
	a.mu.Unlock()
	result := native.End()
	return a.remote.End(tok, cancelled || !dragReady || !result.Valid || result.Cancelled, result.PID, session.Position{X: result.X, Y: result.Y})
}

func (a *App) TransferInfo(tok string) (*session.Offer, error) { return a.remote.OfferInfo(tok) }
