// Package watch reports changes to a single file.
//
// It watches the file's directory rather than the file itself, because many
// editors save by writing a temporary file and renaming it over the original,
// which would end a watch on the file.
package watch

import (
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
)

const debounce = 200 * time.Millisecond

// Watcher calls onChange (debounced) whenever the watched file changes.
type Watcher struct {
	w      *fsnotify.Watcher
	target string
	stop   chan struct{}
	once   sync.Once
}

// New starts watching path.
func New(path string, onChange func()) (*Watcher, error) {
	abs, err := filepath.Abs(path)
	if err != nil {
		return nil, err
	}
	fw, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}
	if err := fw.Add(filepath.Dir(abs)); err != nil {
		fw.Close()
		return nil, err
	}
	w := &Watcher{w: fw, target: abs, stop: make(chan struct{})}
	go w.loop(onChange)
	return w, nil
}

func (w *Watcher) loop(onChange func()) {
	var timer *time.Timer
	for {
		select {
		case <-w.stop:
			if timer != nil {
				timer.Stop()
			}
			return
		case ev, ok := <-w.w.Events:
			if !ok {
				return
			}
			if !samePath(ev.Name, w.target) || ev.Op == fsnotify.Chmod {
				continue
			}
			if timer != nil {
				timer.Stop()
			}
			timer = time.AfterFunc(debounce, onChange)
		case _, ok := <-w.w.Errors:
			if !ok {
				return
			}
		}
	}
}

// Close stops the watcher. It is safe to call more than once.
func (w *Watcher) Close() {
	if w == nil {
		return
	}
	w.once.Do(func() {
		close(w.stop)
		w.w.Close()
	})
}

func samePath(a, b string) bool {
	a, b = filepath.Clean(a), filepath.Clean(b)
	if runtime.GOOS == "windows" || runtime.GOOS == "darwin" {
		return strings.EqualFold(a, b)
	}
	return a == b
}
