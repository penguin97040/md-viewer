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
	"net/netip"
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

// IsGlobalUnicast also includes special-purpose ranges that are not public
// destinations. Translation/tunnel prefixes can route an embedded IPv4
// address to an internal service even when the IPv6 address looks public.
var nonPublicRanges = []netip.Prefix{
	netip.MustParsePrefix("0.0.0.0/8"),
	netip.MustParsePrefix("100.64.0.0/10"),
	netip.MustParsePrefix("192.0.0.0/24"),
	netip.MustParsePrefix("192.0.2.0/24"),
	netip.MustParsePrefix("198.18.0.0/15"),
	netip.MustParsePrefix("198.51.100.0/24"),
	netip.MustParsePrefix("203.0.113.0/24"),
	netip.MustParsePrefix("240.0.0.0/4"),
	netip.MustParsePrefix("64:ff9b::/96"),
	netip.MustParsePrefix("64:ff9b:1::/48"),
	netip.MustParsePrefix("100::/64"),
	netip.MustParsePrefix("2001::/32"),
	netip.MustParsePrefix("2001:10::/28"),
	netip.MustParsePrefix("2001:20::/28"),
	netip.MustParsePrefix("2001:db8::/32"),
	netip.MustParsePrefix("2002::/16"),
	netip.MustParsePrefix("3fff::/20"),
	netip.MustParsePrefix("5f00::/16"),
}

func publicIP(ip net.IP) bool {
	if !ip.IsGlobalUnicast() || ip.IsPrivate() || ip.IsLoopback() || ip.IsLinkLocalUnicast() {
		return false
	}
	addr, ok := netip.AddrFromSlice(ip)
	if !ok {
		return false
	}
	addr = addr.Unmap()
	for _, prefix := range nonPublicRanges {
		if prefix.Contains(addr) {
			return false
		}
	}
	return true
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
