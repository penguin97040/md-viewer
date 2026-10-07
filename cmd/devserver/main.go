// Command devserver serves the frontend to an ordinary browser for quick
// UI work and automated checks, without building the desktop app. A small
// shim stands in for the Wails bindings and calls a JSON API here instead.
//
//	go run ./cmd/devserver [-addr :8080] [file.md ...]
package main

import (
	"bytes"
	"compress/gzip"
	"encoding/json"
	"flag"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"github.com/penguin97040/md-viewer/internal/doc"
	"github.com/penguin97040/md-viewer/internal/settings"
	"github.com/penguin97040/md-viewer/internal/store"
	"github.com/penguin97040/md-viewer/internal/webimage"
)

const shim = `
window.runtime = {
  EventsOn() {}, EventsOff() {}, OnFileDrop() {},
  ClipboardSetText: (t) => navigator.clipboard.writeText(t).then(() => true, () => false),
};
const call = (m) => async (...args) => {
  const r = await fetch('/api/' + m, { method: 'POST', headers: { 'X-Devshim': '1' }, body: JSON.stringify(args) });
  const j = await r.json();
  if (j.error) throw new Error(j.error);
  return j.result;
};
window.go = { main: { App: new Proxy({}, { get: (_, m) => call(m) }) } };
`

type server struct {
	mu    sync.Mutex // guards s
	docs  *store.Store
	start []string
	s     settings.Settings
}

func main() {
	addr := flag.String("addr", "127.0.0.1:8080", "listen address")
	flag.Parse()
	srv := &server{docs: store.New(false, nil), start: flag.Args(), s: settings.Defaults()}
	front := http.FileServer(http.Dir("frontend"))

	http.HandleFunc("/api/", srv.api)
	http.HandleFunc("/devshim.js", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/javascript")
		io.WriteString(w, shim)
	})
	http.HandleFunc("/highlight.css", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/css")
		io.WriteString(w, doc.HighlightCSS())
	})
	http.HandleFunc("/local/", func(w http.ResponseWriter, r *http.Request) {
		p := store.LocalPath(r.URL.Path)
		ct, ok := store.ImageType(p)
		if !ok || !srv.docs.Reachable(p) {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", ct)
		w.Header().Set("X-Content-Type-Options", "nosniff")
		http.ServeFile(w, r, p)
	})
	http.HandleFunc(srv.docs.WebPath(), webimage.Serve)
	http.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path == "/" {
			b, _ := os.ReadFile("frontend/index.html")
			b = bytes.Replace(b, []byte("<link"), []byte(`<script src="/devshim.js"></script><link`), 1)
			w.Header().Set("Content-Type", "text/html")
			w.Write(b)
			return
		}
		if gz, err := os.ReadFile(filepath.Join("frontend", r.URL.Path+".gz")); err == nil {
			zr, _ := gzip.NewReader(bytes.NewReader(gz))
			w.Header().Set("Content-Type", "text/javascript")
			io.Copy(w, zr)
			return
		}
		front.ServeHTTP(w, r)
	})
	log.Printf("serving on http://%s", *addr)
	log.Fatal(http.ListenAndServe(*addr, localOnly(*addr, http.DefaultServeMux)))
}

// localOnly refuses requests whose Host header isn't this server, so a web
// page open in the same browser can't reach the API through DNS rebinding.
func localOnly(addr string, next http.Handler) http.Handler {
	_, port, _ := net.SplitHostPort(addr)
	ok := map[string]bool{addr: true, "localhost:" + port: true, "127.0.0.1:" + port: true, "[::1]:" + port: true}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if !ok[r.Host] {
			http.Error(w, "forbidden", http.StatusForbidden)
			return
		}
		next.ServeHTTP(w, r)
	})
}

func (s *server) api(w http.ResponseWriter, r *http.Request) {
	// A custom header can't be sent cross-site without a CORS preflight,
	// which this server never approves.
	if r.Method != http.MethodPost || r.Header.Get("X-Devshim") != "1" {
		http.Error(w, "forbidden", http.StatusForbidden)
		return
	}
	var args []json.RawMessage
	json.NewDecoder(r.Body).Decode(&args)
	arg := func(i int, v any) {
		if i < len(args) {
			json.Unmarshal(args[i], v)
		}
	}
	var res any
	var err error
	var n int // the first argument as a number: a key or an id
	arg(0, &n)
	switch strings.TrimPrefix(r.URL.Path, "/api/") {
	case "Settings":
		s.mu.Lock()
		res = s.s
		s.mu.Unlock()
	case "SaveSettings":
		s.mu.Lock()
		arg(0, &s.s)
		s.mu.Unlock()
	case "Version":
		res = "dev"
	case "Initial":
		var paths []string
		for _, p := range s.start {
			abs, _ := filepath.Abs(p)
			paths = append(paths, abs)
		}
		res = paths
	case "PickFile", "SetTitle", "Print", "OpenURL":
		res = ""
	case "Open":
		var p string
		arg(0, &p)
		res, err = s.docs.Open(p)
	case "Close":
		s.docs.Close(n)
	case "Reload":
		res, err = s.docs.Reload(n)
	case "ResolveLink":
		var href string
		arg(1, &href)
		res, err = s.docs.Resolve(n, href)
	case "Chunk":
		var i int
		arg(1, &i)
		var d *doc.Doc
		if d, err = s.docs.Get(n); err == nil {
			res, err = d.ChunkHTML(i)
		}
	case "Search":
		var q string
		arg(1, &q)
		var d *doc.Doc
		if d, err = s.docs.Get(n); err == nil {
			res = d.Search(q)
		}
	}
	out := map[string]any{"result": res}
	if err != nil {
		out["error"] = err.Error()
	}
	json.NewEncoder(w).Encode(out)
}
