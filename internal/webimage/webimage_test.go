package webimage

import (
	"context"
	"crypto/tls"
	"crypto/x509"
	"errors"
	"net"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func TestCheck(t *testing.T) {
	for raw, ok := range map[string]bool{
		"https://example.com/a.png":          true,
		"http://192.168.1.10/a.png":          false,
		"file:///etc/passwd":                 false,
		"ftp://example.com/a.png":            false,
		"http://localhost/a.png":             false,
		"http://app.localhost/a.png":         false,
		"http://127.0.0.1:8080/a.png":        false,
		"http://[::1]/a.png":                 false,
		"http://169.254.169.254/latest/meta": false,
		"http://0.0.0.0/a.png":               false,
		"http://localhost./a.png":            false,
		"http://[::ffff:127.0.0.1]/a.png":    false,
		"http://[::ffff:192.168.1.1]/a.png":  false,
		"http://[fc00::1]/a.png":             false,
		"http://[fe80::1%25eth0]/a.png":      false,
		"http://224.0.0.1/a.png":             false,
		"http://[ff02::1]/a.png":             false,
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
	useTestClient(t, srv)
	base := "http://image.example"

	img, err := Fetch(base + "/a.png")
	if err != nil || img.Type != "image/png" || string(img.Data) != "\x89PNG" {
		t.Fatalf("got %v, %v", img, err)
	}
	for _, p := range []string{"/page", "/huge.png", "/to-file", "/to-local", "/missing.png"} {
		if _, err := Fetch(base + p); err == nil {
			t.Errorf("%s: fetched", p)
		}
	}

	// Serve refuses what Fetch refuses.
	rec := httptest.NewRecorder()
	Serve(rec, httptest.NewRequest("GET", "/web/x?u="+url.QueryEscape(base+"/page"), nil))
	if rec.Code != http.StatusNotFound || strings.Contains(rec.Body.String(), "script") {
		t.Errorf("served a page: %d %q", rec.Code, rec.Body.String())
	}
}

// The test transport sees a public destination but connects to our local
// fixture. Production has no exception for local addresses.
func useTestClient(t *testing.T, srv *httptest.Server) *http.Transport {
	t.Helper()
	original := client
	client = newClient(func(context.Context, string) ([]net.IPAddr, error) {
		return []net.IPAddr{{IP: net.ParseIP("93.184.216.34")}}, nil
	}, func(ctx context.Context, network, addr string) (net.Conn, error) {
		u, _ := url.Parse(srv.URL)
		return (&net.Dialer{}).DialContext(ctx, network, u.Host)
	})
	t.Cleanup(func() { client.CloseIdleConnections(); client = original })
	return client.Transport.(*http.Transport)
}

func TestResolvedDestinations(t *testing.T) {
	for _, ips := range [][]string{
		{"127.0.0.1"}, {"10.0.0.1"}, {"172.16.0.1"}, {"192.168.0.1"},
		{"169.254.169.254"}, {"0.0.0.0"}, {"224.0.0.1"}, {"::"}, {"::1"},
		{"fc00::1"}, {"fe80::1"}, {"ff02::1"}, {"::ffff:10.0.0.1"},
		{"93.184.216.34", "127.0.0.1"}, {},
	} {
		t.Run(strings.Join(ips, ","), func(t *testing.T) {
			dialled := false
			c := newClient(func(context.Context, string) ([]net.IPAddr, error) {
				var out []net.IPAddr
				for _, ip := range ips {
					out = append(out, net.IPAddr{IP: net.ParseIP(ip)})
				}
				return out, nil
			}, func(context.Context, string, string) (net.Conn, error) {
				dialled = true
				return nil, errors.New("unexpected dial")
			})
			defer c.CloseIdleConnections()
			if _, err := c.Get("http://image.example/a.png"); err == nil || dialled {
				t.Fatalf("unsafe DNS answer: err %v, dialled %v", err, dialled)
			}
		})
	}
}

func TestDialPinsResolvedIP(t *testing.T) {
	t.Setenv("HTTP_PROXY", "http://127.0.0.1:1")
	t.Setenv("HTTPS_PROXY", "http://127.0.0.1:1")
	lookups := 0
	c := newClient(func(context.Context, string) ([]net.IPAddr, error) {
		lookups++
		if lookups > 1 {
			return []net.IPAddr{{IP: net.ParseIP("127.0.0.1")}}, nil
		}
		return []net.IPAddr{{IP: net.ParseIP("93.184.216.34")}}, nil
	}, func(_ context.Context, _ string, addr string) (net.Conn, error) {
		if addr != "93.184.216.34:80" {
			t.Errorf("dialled %s", addr)
		}
		return nil, errors.New("controlled dial failure")
	})
	defer c.CloseIdleConnections()
	if c.Transport.(*http.Transport).Proxy != nil {
		t.Fatal("proxy enabled")
	}
	c.Get("http://image.example/a.png")
	if lookups != 1 {
		t.Fatalf("%d DNS lookups", lookups)
	}
}

func TestRedirectDNS(t *testing.T) {
	for _, dest := range []string{"private.example", "192.168.1.1"} {
		t.Run(dest, func(t *testing.T) {
			srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				http.Redirect(w, r, "http://"+dest+"/a.png", http.StatusFound)
			}))
			defer srv.Close()
			dials := 0
			c := newClient(func(_ context.Context, host string) ([]net.IPAddr, error) {
				ip := "93.184.216.34"
				if host == dest {
					ip = "10.0.0.1"
				}
				return []net.IPAddr{{IP: net.ParseIP(ip)}}, nil
			}, func(ctx context.Context, network, addr string) (net.Conn, error) {
				dials++
				u, _ := url.Parse(srv.URL)
				return (&net.Dialer{}).DialContext(ctx, network, u.Host)
			})
			defer c.CloseIdleConnections()
			if _, err := c.Get("http://image.example/start"); err == nil || dials != 1 {
				t.Fatalf("redirect: err %v, dials %d", err, dials)
			}
		})
	}
}

func TestTLSVerification(t *testing.T) {
	srv := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "image/png")
		w.Write([]byte("PNG"))
	}))
	defer srv.Close()
	transport := useTestClient(t, srv)
	roots := x509.NewCertPool()
	roots.AddCert(srv.Certificate())
	transport.TLSClientConfig = &tls.Config{RootCAs: roots}
	// The fixture certificate is valid for example.com, not its dialled IP.
	if _, err := Fetch("https://example.com/a.png"); err != nil {
		t.Fatal(err)
	}
	if _, err := Fetch("https://wrong.example/a.png"); err == nil {
		t.Fatal("accepted wrong TLS hostname")
	}
}
