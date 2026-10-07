package main

import (
	"bytes"
	"compress/gzip"
	"io"
	"io/fs"
	"net/http"
	"net/url"
	"os"
	"path"
	"path/filepath"
	goruntime "runtime"
	"strings"

	"github.com/penguin97040/md-viewer/internal/doc"
)

// localURL turns a file system path into a URL served by assetHandler.
func localURL(p string) string {
	p = strings.TrimPrefix(filepath.ToSlash(p), "/")
	return (&url.URL{Path: "/local/" + p}).EscapedPath()
}

// localPath reverses localURL.
func localPath(urlPath string) string {
	p := strings.TrimPrefix(urlPath, "/local/")
	if goruntime.GOOS != "windows" || strings.HasPrefix(p, "/") {
		p = "/" + p // unix absolute path, or a Windows UNC path (//server/share)
	}
	return filepath.FromSlash(p)
}

var imageTypes = map[string]string{
	".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".gif": "image/gif",
	".webp": "image/webp", ".svg": "image/svg+xml", ".bmp": "image/bmp", ".ico": "image/x-icon",
	".avif": "image/avif",
}

// assetHandler serves what the embedded files cannot: images next to the
// open document, highlight colours, and gzipped vendor scripts.
type assetHandler struct {
	files fs.FS
}

func (h *assetHandler) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	p := r.URL.Path
	switch {
	case strings.HasPrefix(p, "/local/"):
		h.serveLocal(w, r, localPath(p))
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
// local files into the page.
func (h *assetHandler) serveLocal(w http.ResponseWriter, r *http.Request, p string) {
	ct, ok := imageTypes[strings.ToLower(filepath.Ext(p))]
	if !ok {
		http.NotFound(w, r)
		return
	}
	b, err := os.ReadFile(p)
	if err != nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", ct)
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
