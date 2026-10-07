package webimage

import (
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func TestCheck(t *testing.T) {
	for raw, ok := range map[string]bool{
		"https://example.com/a.png":          true,
		"http://192.168.1.10/a.png":          true,
		"file:///etc/passwd":                 false,
		"ftp://example.com/a.png":            false,
		"http://localhost/a.png":             false,
		"http://app.localhost/a.png":         false,
		"http://127.0.0.1:8080/a.png":        false,
		"http://[::1]/a.png":                 false,
		"http://169.254.169.254/latest/meta": false,
		"http://0.0.0.0/a.png":               false,
		"https:///a.png":                     false,
	} {
		u, err := url.Parse(raw)
		if err != nil {
			t.Fatal(err)
		}
		if got := check(u) == nil; got != ok {
			t.Errorf("check(%q) allowed = %v, want %v", raw, got, ok)
		}
	}
}

func TestFetch(t *testing.T) {
	allowLoopback = true
	defer func() { allowLoopback = false }()
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/a.png":
			w.Header().Set("Content-Type", "image/png")
			w.Write([]byte("\x89PNG"))
		case "/page":
			w.Header().Set("Content-Type", "text/html")
			w.Write([]byte("<script>alert(1)</script>"))
		case "/huge.png":
			w.Header().Set("Content-Type", "image/png")
			w.Write(make([]byte, MaxBytes+1))
		case "/to-file":
			http.Redirect(w, r, "file:///etc/passwd", http.StatusFound)
		case "/to-local":
			http.Redirect(w, r, "http://localhost:1/a.png", http.StatusFound)
		default:
			http.NotFound(w, r)
		}
	}))
	defer srv.Close()

	img, err := Fetch(srv.URL + "/a.png")
	if err != nil || img.Type != "image/png" || string(img.Data) != "\x89PNG" {
		t.Fatalf("got %v, %v", img, err)
	}
	for _, p := range []string{"/page", "/huge.png", "/to-file", "/to-local", "/missing.png"} {
		if _, err := Fetch(srv.URL + p); err == nil {
			t.Errorf("%s: fetched", p)
		}
	}

	// Serve refuses what Fetch refuses.
	rec := httptest.NewRecorder()
	Serve(rec, httptest.NewRequest("GET", "/web/x?u="+url.QueryEscape(srv.URL+"/page"), nil))
	if rec.Code != http.StatusNotFound || strings.Contains(rec.Body.String(), "script") {
		t.Errorf("served a page: %d %q", rec.Code, rec.Body.String())
	}
}
