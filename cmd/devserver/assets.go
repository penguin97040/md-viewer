package main

import (
	"compress/gzip"
	"io"
	"io/fs"
	"net/http"
	"strings"
)

// serveCompressed keeps decoded URL paths inside the frontend. ServeMux
// preserves escaped dot segments, so filesystem joins alone are unsafe.
func serveCompressed(w http.ResponseWriter, r *http.Request, root fs.FS) bool {
	name := strings.TrimPrefix(r.URL.Path, "/") + ".gz"
	if !fs.ValidPath(name) || strings.Contains(name, `\`) {
		return false
	}
	f, err := root.Open(name)
	if err != nil {
		return false
	}
	defer f.Close()
	z, err := gzip.NewReader(f)
	if err != nil {
		http.NotFound(w, r)
		return true
	}
	defer z.Close()
	w.Header().Set("Content-Type", "text/javascript")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	io.Copy(w, z)
	return true
}
