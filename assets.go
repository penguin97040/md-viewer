package main

import (
	"bytes"
	"compress/gzip"
	"crypto/subtle"
	"github.com/penguin97040/md-viewer/internal/session"
	"io"
	"io/fs"
	"net/http"
	"path"
	"strconv"
	"strings"

	"github.com/penguin97040/md-viewer/internal/doc"
	"github.com/penguin97040/md-viewer/internal/store"
	"github.com/penguin97040/md-viewer/internal/webimage"
)

// assetHandler serves what the embedded files cannot: images next to the
// open document, highlight colours, and gzipped vendor scripts.
type assetHandler struct {
	files  fs.FS
	remote *session.Client
}

func (h *assetHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	p := r.URL.Path
	switch {
	case strings.HasPrefix(p, "/local/"):
		h.serveLocal(w, r, p)
	case subtle.ConstantTimeCompare([]byte(p), []byte(h.remote.WebPath())) == 1:
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
	parts := strings.SplitN(strings.TrimPrefix(p, "/local/"), "/", 2)
	if len(parts) != 2 {
		http.NotFound(w, r)
		return
	}
	key, err := strconv.Atoi(parts[0])
	if err != nil {
		http.NotFound(w, r)
		return
	}
	p = store.LocalPath("/local/" + parts[1])
	ct, ok := store.ImageType(p)
	if !ok {
		http.NotFound(w, r)
		return
	}
	b, err := h.remote.Local(key, p)
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
