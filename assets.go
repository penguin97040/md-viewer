package main

import (
	"bytes"
	"compress/gzip"
	"crypto/subtle"
	"io"
	"io/fs"
	"net/http"
	"os"
	"path"
	"strings"

	"github.com/penguin97040/md-viewer/internal/doc"
	"github.com/penguin97040/md-viewer/internal/store"
	"github.com/penguin97040/md-viewer/internal/webimage"
)

// assetHandler serves what the embedded files cannot: images next to the
// open document, highlight colours, and gzipped vendor scripts.
type assetHandler struct {
	files fs.FS
	store *store.Store
}

func (h *assetHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	p := r.URL.Path
	switch {
	case strings.HasPrefix(p, "/local/"):
		h.serveLocal(w, r, store.LocalPath(p))
	case subtle.ConstantTimeCompare([]byte(p), []byte(h.store.WebPath())) == 1:
		webimage.Serve(w, r)
	case p == "/highlight.css":
		w.Header().Set("Content-Type", "text/css; charset=utf-8")
		io.WriteString(w, doc.HighlightCSS())
	default:
		b, err := h.gunzip(strings.TrimPrefix(path.Clean(p), "/"))
		if err != nil {
			http.NotFound(w, r)
			return
		}
		ct := "application/octet-stream"
		switch path.Ext(p) {
		case ".js":
			ct = "text/javascript; charset=utf-8"
		case ".css":
			ct = "text/css; charset=utf-8"
		}
		w.Header().Set("Content-Type", ct)
		w.Write(b)
	}
}

// serveLocal serves image files only, so a document cannot pull arbitrary
// local files into the page, and not from other computers (see
// store.Reachable).
func (h *assetHandler) serveLocal(w http.ResponseWriter, r *http.Request, p string) {
	ct, ok := store.ImageType(p)
	if !ok || !h.store.Reachable(p) {
		http.NotFound(w, r)
		return
	}
	b, err := os.ReadFile(p)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", ct)
	w.Header().Set("X-Content-Type-Options", "nosniff")
	if ct == "image/svg+xml" {
		w.Header().Set("Content-Security-Policy", "script-src 'none'")
	}
	w.Write(b)
}

// gunzip returns the decompressed contents of name+".gz" from the embedded
// files. Large vendor scripts are stored compressed to keep the executable
// small; each is loaded at most once per page.
func (h *assetHandler) gunzip(name string) ([]byte, error) {
	raw, err := fs.ReadFile(h.files, name+".gz")
	if err != nil {
		return nil, err
	}
	zr, err := gzip.NewReader(bytes.NewReader(raw))
	if err != nil {
		return nil, err
	}
	return io.ReadAll(zr)
}
