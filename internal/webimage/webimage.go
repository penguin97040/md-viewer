// Package webimage fetches images from the web for a document whose reader
// has agreed to load them. The web view itself is not allowed to reach the
// internet (see the Content-Security-Policy in index.html), so these come
// through Go.
package webimage

import (
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// MaxBytes is the largest image fetched.
const MaxBytes = 25 << 20

// allowLoopback lets tests fetch from a local server.
var allowLoopback = false

var client = &http.Client{
	Timeout: 30 * time.Second,
	CheckRedirect: func(req *http.Request, via []*http.Request) error {
		if len(via) >= 5 {
			return errors.New("too many redirects")
		}
		return check(req.URL)
	},
}

// check allows http and https addresses on other machines. Loopback and
// link-local addresses are refused, so a document can't use the reader's
// consent to poke at services on their own computer or a cloud metadata
// endpoint. (Names that resolve to such addresses are not caught; this is a
// guard against the obvious, not a firewall.)
func check(u *url.URL) error {
	if u.Scheme != "http" && u.Scheme != "https" {
		return errors.New("not a web address")
	}
	host := u.Hostname()
	if allowLoopback && host == "127.0.0.1" {
		return nil
	}
	if host == "" || strings.EqualFold(host, "localhost") || strings.HasSuffix(strings.ToLower(host), ".localhost") {
		return errors.New("not a web address")
	}
	if ip := net.ParseIP(host); ip != nil && (ip.IsLoopback() || ip.IsLinkLocalUnicast() || ip.IsUnspecified()) {
		return errors.New("not a web address")
	}
	return nil
}

// Image is a fetched image.
type Image struct {
	Type string
	Data []byte
}

// Fetch downloads an image. Anything that isn't an image is refused.
func Fetch(raw string) (*Image, error) {
	if strings.HasPrefix(raw, "//") {
		raw = "https:" + raw
	}
	u, err := url.Parse(raw)
	if err != nil {
		return nil, err
	}
	if err := check(u); err != nil {
		return nil, err
	}
	req, err := http.NewRequest(http.MethodGet, u.String(), nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Accept", "image/*")
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("%s", resp.Status)
	}
	ct := strings.ToLower(strings.TrimSpace(strings.Split(resp.Header.Get("Content-Type"), ";")[0]))
	if !strings.HasPrefix(ct, "image/") {
		return nil, errors.New("not an image")
	}
	b, err := io.ReadAll(io.LimitReader(resp.Body, MaxBytes+1))
	if err != nil {
		return nil, err
	}
	if len(b) > MaxBytes {
		return nil, errors.New("image too large")
	}
	return &Image{Type: ct, Data: b}, nil
}

// Serve writes a fetched image, or a 404 if it can't be had. The URL comes
// from the u query parameter.
func Serve(w http.ResponseWriter, r *http.Request) {
	img, err := Fetch(r.URL.Query().Get("u"))
	if err != nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", img.Type)
	w.Header().Set("Cache-Control", "private, max-age=3600")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	if img.Type == "image/svg+xml" {
		w.Header().Set("Content-Security-Policy", "script-src 'none'")
	}
	w.Write(img.Data)
}
