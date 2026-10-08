// Package webimage fetches images from the web for a document whose reader
// has agreed to load them. The web view itself is not allowed to reach the
// internet (see the Content-Security-Policy in index.html), so these come
// through Go.
package webimage

import (
	"context"
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

var client = newClient(net.DefaultResolver.LookupIPAddr, (&net.Dialer{Timeout: 10 * time.Second}).DialContext)

// newClient resolves and validates destinations at the connection boundary.
// Dial only the validated IP, so a second DNS lookup cannot change it. The
// transport still verifies TLS against the original hostname. Proxies are
// disabled because they would resolve and connect outside these checks.
func newClient(lookup func(context.Context, string) ([]net.IPAddr, error), dial func(context.Context, string, string) (net.Conn, error)) *http.Client {
	transport := http.DefaultTransport.(*http.Transport).Clone()
	transport.Proxy = nil
	transport.DialContext = func(ctx context.Context, network, addr string) (net.Conn, error) {
		host, port, err := net.SplitHostPort(addr)
		if err != nil {
			return nil, err
		}
		ips, err := lookup(ctx, host)
		if err != nil {
			return nil, err
		}
		if len(ips) == 0 {
			return nil, errors.New("no web address found")
		}
		for _, ip := range ips {
			if ip.Zone != "" || !publicIP(ip.IP) {
				return nil, errors.New("not a public web address")
			}
		}
		for _, ip := range ips {
			var conn net.Conn
			conn, err = dial(ctx, network, net.JoinHostPort(ip.IP.String(), port))
			if err == nil {
				return conn, nil
			}
		}
		return nil, err
	}
	return &http.Client{
		Transport: transport,
		Timeout:   30 * time.Second,
		CheckRedirect: func(req *http.Request, via []*http.Request) error {
			if len(via) >= 5 {
				return errors.New("too many redirects")
			}
			return check(req.URL)
		},
	}
}

func publicIP(ip net.IP) bool {
	return ip.IsGlobalUnicast() && !ip.IsPrivate() && !ip.IsLoopback() && !ip.IsLinkLocalUnicast()
}

// check rejects unsafe URL forms and literal addresses. DNS answers are
// checked again by the transport when it connects, including on redirects.
func check(u *url.URL) error {
	if u.Scheme != "http" && u.Scheme != "https" {
		return errors.New("not a web address")
	}
	host := strings.TrimSuffix(u.Hostname(), ".")
	if host == "" || strings.EqualFold(host, "localhost") || strings.HasSuffix(strings.ToLower(host), ".localhost") {
		return errors.New("not a web address")
	}
	if strings.Contains(host, "%") {
		return errors.New("not a web address")
	}
	if ip := net.ParseIP(host); ip != nil && !publicIP(ip) {
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
