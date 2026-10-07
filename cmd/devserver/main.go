// Command devserver serves the frontend to an ordinary browser for quick
// UI work and automated checks, without building the desktop app. A small
// shim stands in for the Wails bindings and calls a JSON API here instead.
//
//	go run ./cmd/devserver [-addr :8080] [file.md]
package main

import (
	"bytes"
	"compress/gzip"
	"encoding/json"
	"flag"
	"io"
	"log"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"

	"github.com/penguin97040/md-viewer/internal/doc"
	"github.com/penguin97040/md-viewer/internal/settings"
)

const shim = `
window.runtime = { EventsOn() {}, EventsOff() {}, OnFileDrop() {} };
const call = (m) => async (...args) => {
  const r = await fetch('/api/' + m, { method: 'POST', body: JSON.stringify(args) });
  const j = await r.json();
  if (j.error) throw new Error(j.error);
  return j.result;
};
window.go = { main: { App: new Proxy({}, { get: (_, m) => call(m) }) } };
`

type server struct {
	mu    sync.Mutex
	doc   *doc.Doc
	id    int
	path  string
	start string
	s     settings.Settings
}

func main() {
	addr := flag.String("addr", "127.0.0.1:8080", "listen address")
	flag.Parse()
	srv := &server{start: flag.Arg(0), s: settings.Defaults()}
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
		p := "/" + strings.TrimPrefix(r.URL.Path, "/local/")
		http.ServeFile(w, r, filepath.FromSlash(p))
	})
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
	log.Fatal(http.ListenAndServe(*addr, nil))
}

func (s *server) api(w http.ResponseWriter, r *http.Request) {
	var args []json.RawMessage
	json.NewDecoder(r.Body).Decode(&args)
	arg := func(i int, v any) {
		if i < len(args) {
			json.Unmarshal(args[i], v)
		}
	}
	var res any
	var err error
	s.mu.Lock()
	defer s.mu.Unlock()
	switch strings.TrimPrefix(r.URL.Path, "/api/") {
	case "Settings":
		res = s.s
	case "SaveSettings":
		arg(0, &s.s)
	case "Version":
		res = "dev"
	case "Initial":
		if s.start != "" {
			res, err = s.open(s.start)
		}
	case "Open":
		var p string
		arg(0, &p)
		res, err = s.open(p)
	case "OpenLink":
		var p string
		arg(0, &p)
		res, err = s.open(filepath.Join(filepath.Dir(s.path), filepath.FromSlash(p)))
	case "Reload":
		res, err = s.open(s.path)
	case "Chunk":
		var id, i int
		arg(0, &id)
		arg(1, &i)
		res, err = s.doc.ChunkHTML(i)
	case "Search":
		var q string
		arg(1, &q)
		res = s.doc.Search(q)
	}
	out := map[string]any{"result": res}
	if err != nil {
		out["error"] = err.Error()
	}
	json.NewEncoder(w).Encode(out)
}

func (s *server) open(p string) (any, error) {
	abs, _ := filepath.Abs(p)
	d, err := doc.Load(abs)
	if err != nil {
		return nil, err
	}
	s.doc, s.path = d, abs
	s.id++
	return map[string]any{
		"id": s.id, "path": abs, "name": filepath.Base(abs),
		"base": "/local" + filepath.ToSlash(filepath.Dir(abs)) + "/",
		"size": len(d.Source), "parseMs": float64(d.ParseTime.Microseconds()) / 1000,
		"chunks": d.Chunks, "headings": d.Headings, "anchors": d.Anchors,
	}, nil
}
