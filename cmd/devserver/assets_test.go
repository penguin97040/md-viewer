package main

import (
	"bytes"
	"compress/gzip"
	"io/fs"
	"net/http"
	"net/http/httptest"
	"testing"
	"testing/fstest"
)

type assetFS struct {
	fs.FS
	opened []string
}

func (f *assetFS) Open(name string) (fs.File, error) {
	f.opened = append(f.opened, name)
	return f.FS.Open(name)
}

func TestCompressedAssets(t *testing.T) {
	var data bytes.Buffer
	z := gzip.NewWriter(&data)
	z.Write([]byte("window.assetLoaded = true;"))
	z.Close()
	root := &assetFS{FS: fstest.MapFS{
		"vendor/test.js.gz": {Data: data.Bytes()},
		"invalid.js.gz":     {Data: []byte("invalid gzip")},
	}}
	mux := http.NewServeMux()
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if !serveCompressed(w, r, root) {
			http.NotFound(w, r)
		}
	})
	for _, path := range []string{
		"/%2e%2e/private", "/vendor/%2e%2e/%2e%2e/private",
		"/%2f..%2fprivate", "/..%5cprivate", "/vendor%5c..%5cprivate",
	} {
		root.opened = nil
		w := httptest.NewRecorder()
		mux.ServeHTTP(w, httptest.NewRequest("GET", path, nil))
		if w.Code != http.StatusNotFound || len(root.opened) != 0 {
			t.Errorf("traversal %q: status %d, opened %v", path, w.Code, root.opened)
		}
	}
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("GET", "/vendor/test.js", nil))
	if w.Code != http.StatusOK || w.Body.String() != "window.assetLoaded = true;" {
		t.Fatalf("valid asset: %d %q", w.Code, w.Body.String())
	}
	w = httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest("GET", "/invalid.js", nil))
	if w.Code != http.StatusNotFound {
		t.Fatalf("invalid gzip: %d", w.Code)
	}
}
