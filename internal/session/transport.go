package session

import (
	"bytes"
	"context"
	"crypto/subtle"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/penguin97040/md-viewer/internal/settings"
	"github.com/penguin97040/md-viewer/internal/store"
)

type storeInfo = store.Info
type settingsValue = settings.Settings

const maxRequest = 64 << 10
const maxResponse = 64 << 20

type record struct {
	Address    string `json:"address"`
	Credential string `json:"credential"`
}
type request struct {
	Cancelled bool            `json:"cancelled"`
	PID       int             `json:"pid"`
	Op        string          `json:"op"`
	Key       int             `json:"key"`
	Parse     int             `json:"parse"`
	Index     int             `json:"index"`
	Text      string          `json:"text"`
	Paths     []string        `json:"paths"`
	Keys      []int           `json:"keys"`
	Token     string          `json:"token"`
	State     TabState        `json:"state"`
	Position  Position        `json:"position"`
	Settings  json.RawMessage `json:"settings"`
}
type response struct {
	Result json.RawMessage `json:"result"`
	Error  string          `json:"error,omitempty"`
}
type Client struct {
	bootstrap Bootstrap
	http      *http.Client
}

func NewClient(b Bootstrap) (*Client, error) {
	host, _, err := net.SplitHostPort(b.Address)
	if err != nil || host != "127.0.0.1" || len(b.Credential) != 64 {
		return nil, errors.New("invalid session discovery")
	}
	tr := &http.Transport{Proxy: nil, MaxIdleConnsPerHost: 8, MaxConnsPerHost: 8, ResponseHeaderTimeout: 35 * time.Second}
	return &Client{b, &http.Client{Transport: tr, Timeout: 40 * time.Second}}, nil
}
func (c *Client) call(ctx context.Context, r request, out any) error {
	b, e := json.Marshal(r)
	if e != nil {
		return e
	}
	if len(b) > maxRequest {
		return errors.New("request is too large")
	}
	req, e := http.NewRequestWithContext(ctx, http.MethodPost, "http://"+c.bootstrap.Address+"/rpc", bytes.NewReader(b))
	if e != nil {
		return e
	}
	req.Header.Set("Authorization", "Bearer "+c.bootstrap.Credential)
	req.Header.Set("Content-Type", "application/json")
	res, e := c.http.Do(req)
	if e != nil {
		return e
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return fmt.Errorf("session refused request (%d)", res.StatusCode)
	}
	var reply response
	if e = json.NewDecoder(io.LimitReader(res.Body, maxResponse)).Decode(&reply); e != nil {
		return e
	}
	if reply.Error != "" {
		return errors.New(reply.Error)
	}
	if out != nil {
		return json.Unmarshal(reply.Result, out)
	}
	return nil
}
func (c *Client) Call(r request, out any) error { return c.call(context.Background(), r, out) }
func (c *Client) Register() error               { return c.Call(request{Op: "register", PID: os.Getpid()}, nil) }
func (c *Client) Focus() error                  { return c.Call(request{Op: "focus"}, nil) }
func (c *Client) Unregister() error {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	defer c.http.CloseIdleConnections()
	return c.call(ctx, request{Op: "unregister"}, nil)
}
func (c *Client) WindowReady() error {
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	return c.call(ctx, request{Op: "window-ready"}, nil)
}
func (c *Client) Poll(ctx context.Context) ([]Event, error) {
	var events []Event
	e := c.call(ctx, request{Op: "poll"}, &events)
	return events, e
}
func (c *Client) Open(p string) (*OpenResult, error) {
	var r OpenResult
	e := c.Call(request{Op: "open", Text: p}, &r)
	c.decorate(r.Info)
	return &r, e
}
func (c *Client) Info(key int) (*storeInfo, error) {
	var info storeInfo
	e := c.Call(request{Op: "info", Key: key}, &info)
	c.decorate(&info)
	return &info, e
}
func (c *Client) Reload(key int) (*storeInfo, error) {
	var info storeInfo
	e := c.Call(request{Op: "reload", Key: key}, &info)
	c.decorate(&info)
	return &info, e
}
func (c *Client) Close(key int) error { return c.Call(request{Op: "close", Key: key}, nil) }
func (c *Client) Resolve(key int, href string) (string, error) {
	var p string
	e := c.Call(request{Op: "resolve", Key: key, Text: href}, &p)
	return p, e
}
func (c *Client) Dir(key int) string {
	var p string
	_ = c.Call(request{Op: "dir", Key: key}, &p)
	return p
}
func (c *Client) Chunk(id, i int) (string, error) {
	var html string
	e := c.Call(request{Op: "chunk", Parse: id, Index: i}, &html)
	return html, e
}
func (c *Client) Search(id int, q string) ([]int, error) {
	var counts []int
	e := c.Call(request{Op: "search", Parse: id, Text: q}, &counts)
	return counts, e
}
func (c *Client) Settings() (settingsValue, error) {
	var s settingsValue
	e := c.Call(request{Op: "settings"}, &s)
	return s, e
}
func (c *Client) SaveSettings(s settingsValue) error {
	b, e := json.Marshal(s)
	if e != nil {
		return e
	}
	return c.Call(request{Op: "save-settings", Settings: b}, nil)
}
func (c *Client) Order(keys []int) error { return c.Call(request{Op: "order", Keys: keys}, nil) }
func (c *Client) Begin(key int, state TabState) (string, error) {
	var t string
	e := c.Call(request{Op: "begin", Key: key, State: state}, &t)
	return t, e
}
func (c *Client) Accept(tok string, index int) error {
	return c.Call(request{Op: "accept", Token: tok, Index: index}, nil)
}
func (c *Client) Detach(tok string, pos Position) error {
	return c.Call(request{Op: "detach", Token: tok, Position: pos}, nil)
}
func (c *Client) Cancel(tok string) error { return c.Call(request{Op: "cancel", Token: tok}, nil) }
func (c *Client) Discard(tok string, key int) error {
	return c.Call(request{Op: "discard", Token: tok, Key: key}, nil)
}
func (c *Client) Ack(tok string, id int) (*Offer, error) {
	var offer *Offer
	e := c.Call(request{Op: "ack", Token: tok, Parse: id}, &offer)
	if offer != nil {
		c.decorate(offer.Info)
	}
	return offer, e
}
func (c *Client) Local(key int, p string) ([]byte, error) {
	var b []byte
	e := c.Call(request{Op: "local", Key: key, Text: p}, &b)
	return b, e
}
func (c *Client) WebPath() string { return "/web/" + c.bootstrap.Web }
func (c *Client) decorate(info *storeInfo) {
	if info != nil {
		info.Base = "/local/" + strconv.Itoa(info.Key) + strings.TrimPrefix(info.Base, "/local")
		info.Web = c.WebPath()
	}
}
func (c *Client) DecorateEvent(e Event) Event {
	b, _ := json.Marshal(e.Data)
	switch e.Name {
	case "doc:changed":
		var info storeInfo
		if json.Unmarshal(b, &info) == nil {
			c.decorate(&info)
			e.Data = &info
		}
	case "tab:offer":
		var offer Offer
		if json.Unmarshal(b, &offer) == nil {
			c.decorate(offer.Info)
			e.Data = offer
		}
	}
	return e
}
func (s *Service) handler(credential string) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// This service never accepts browser-origin requests, CORS or arbitrary Host.
		if r.Method != "POST" || r.URL.Path != "/rpc" || r.Host != s.address || r.Header.Get("Origin") != "" {
			http.Error(w, "forbidden", 403)
			return
		}
		auth := strings.TrimPrefix(r.Header.Get("Authorization"), "Bearer ")
		launcher := subtle.ConstantTimeCompare([]byte(auth), []byte(credential)) == 1
		id := ""
		if !launcher {
			id = s.authenticated(auth)
		}
		if !launcher && id == "" {
			http.Error(w, "forbidden", 403)
			return
		}
		var req request
		dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxRequest))
		dec.DisallowUnknownFields()
		if dec.Decode(&req) != nil {
			http.Error(w, "invalid request", 400)
			return
		}
		var tail any
		if dec.Decode(&tail) != io.EOF {
			http.Error(w, "invalid request", 400)
			return
		}
		var out any
		var err error
		if launcher {
			switch req.Op {
			case "ping":
				out = true
			case "launch":
				err = s.Launch(req.Paths)
			default:
				err = errors.New("launcher operation refused")
			}
		} else {
			out, err = s.dispatch(r.Context(), id, req)
		}
		reply := struct {
			Result any    `json:"result"`
			Error  string `json:"error,omitempty"`
		}{Result: out}
		if err != nil {
			reply.Error = err.Error()
		}
		w.Header().Set("Content-Type", "application/json")
		encoded, e := json.Marshal(reply)
		if e != nil || len(encoded) > maxResponse {
			http.Error(w, "response is too large", http.StatusRequestEntityTooLarge)
			return
		}
		_, _ = w.Write(encoded)
	})
}
func (s *Service) dispatch(ctx context.Context, id string, r request) (any, error) {
	switch r.Op {
	case "register":
		return nil, s.Register(id, r.PID)
	case "window-ready":
		return nil, s.WindowReady(id)
	case "focus":
		return nil, s.Focus(id)
	case "unregister":
		s.Unregister(id)
		return nil, nil
	case "poll":
		return s.Poll(ctx, id)
	case "open":
		return s.Open(id, r.Text)
	case "info":
		return s.Info(id, r.Key)
	case "close":
		return nil, s.Close(id, r.Key)
	case "reload":
		return s.Reload(id, r.Key)
	case "resolve", "dir":
		info, e := s.Info(id, r.Key)
		if e != nil {
			return nil, e
		}
		if r.Op == "dir" {
			return s.Store.Dir(r.Key), nil
		}
		path, e := s.Store.Resolve(r.Key, r.Text)
		if e != nil {
			return nil, e
		}
		if !store.ReachableFrom(info.Path, path) {
			return nil, errors.New("links to files on other computers are not followed")
		}
		return path, nil
	case "chunk", "search":
		if _, e := s.ReadID(id, r.Parse); e != nil {
			return nil, e
		}
		d, e := s.Store.Get(r.Parse)
		if e != nil {
			return nil, e
		}
		if r.Op == "chunk" {
			return d.ChunkHTML(r.Index)
		}
		if len(r.Text) > 4096 {
			return nil, errors.New("search is too long")
		}
		return d.Search(r.Text), nil
	case "local":
		if _, e := s.Info(id, r.Key); e != nil {
			return nil, e
		}
		info, e := s.Info(id, r.Key)
		if e != nil {
			return nil, e
		}
		if _, ok := store.ImageType(r.Text); !ok || !store.ReachableFrom(info.Path, r.Text) {
			return nil, errors.New("image refused")
		}
		f, e := os.Open(r.Text)
		if e != nil {
			return nil, e
		}
		defer f.Close()
		st, e := f.Stat()
		if e != nil || !st.Mode().IsRegular() || st.Size() > 25<<20 {
			return nil, errors.New("image is too large or not an ordinary file")
		}
		return io.ReadAll(io.LimitReader(f, 25<<20))
	case "settings":
		return s.Settings(), nil
	case "save-settings":
		var next settingsValue
		if e := json.Unmarshal(r.Settings, &next); e != nil {
			return nil, e
		}
		return nil, s.SaveSettings(next)
	case "order":
		return nil, s.Order(id, r.Keys)
	case "hover":
		return nil, s.Hover(id, r.Token, r.PID, r.Position)
	case "start":
		return nil, s.Start(id, r.Token, r.State)
	case "start-feedback":
		if err := s.Start(id, r.Token, r.State); err != nil {
			return nil, err
		}
		return s.DragWindows(id, r.Token)
	case "drag-windows":
		return s.DragWindows(id, r.Token)
	case "end":
		return nil, s.End(id, r.Token, r.Cancelled, r.PID, r.Position)
	case "begin":
		return s.Begin(id, r.Key, r.State)
	case "accept":
		return nil, s.Accept(id, r.Token, r.Index)
	case "detach":
		return nil, s.Detach(id, r.Token, r.Position)
	case "offer-info":
		return s.OfferInfo(id, r.Token)
	case "ack":
		return s.Ack(id, r.Token, r.Parse)
	case "cancel":
		return nil, s.Cancel(id, r.Token)
	case "discard":
		return nil, s.Discard(id, r.Token, r.Key)
	}
	return nil, errors.New("unknown operation")
}
func discoveryDir() (string, error) {
	p, e := os.UserConfigDir()
	if e != nil {
		return "", e
	}
	p = filepath.Join(p, "md-viewer", "session")
	if e = os.MkdirAll(p, 0700); e != nil {
		return "", e
	}
	if e = protect(p); e != nil {
		return "", e
	}
	return p, nil
}
func childCommand(exe, mode string, b *Bootstrap) (*exec.Cmd, error) {
	cmd := exec.Command(exe, mode)
	// STARTF_USESHOWWINDOW overrides a GUI application's first ShowWindow call.
	// Only the coordinator is hidden; viewer children must appear normally.
	prepareChild(cmd, mode == "--coordinator")
	if b != nil {
		data, e := json.Marshal(b)
		if e != nil {
			return nil, e
		}
		cmd.Stdin = bytes.NewReader(data)
	}
	return cmd, nil
}

func startChild(cmd *exec.Cmd, exited func()) error {
	if e := cmd.Start(); e != nil {
		return e
	}
	go func() {
		_ = cmd.Wait()
		if exited != nil {
			exited()
		}
	}()
	return nil
}

func spawnMode(mode string, b *Bootstrap, exited ...func()) error {
	exe, e := os.Executable()
	if e != nil {
		return e
	}
	cmd, e := childCommand(exe, mode, b)
	if e != nil {
		return e
	}
	var notify func()
	if len(exited) != 0 {
		notify = exited[0]
	}
	return startChild(cmd, notify)
}

// Launch starts (or discovers) the private coordinator. Competing coordinator
// children contend for the OS lock; only its owner writes the discovery record.
func Launch(paths []string) error {
	dir, e := discoveryDir()
	if e != nil {
		return e
	}
	name := filepath.Join(dir, "discovery.json")
	for tries := 0; tries < 100; tries++ {
		data, err := os.ReadFile(name)
		if err == nil && len(data) < 4096 {
			var r record
			if json.Unmarshal(data, &r) == nil {
				c, e := NewClient(Bootstrap{Address: r.Address, Credential: r.Credential})
				if e == nil {
					ctx, cancel := context.WithTimeout(context.Background(), time.Second)
					err = c.call(ctx, request{Op: "launch", Paths: paths}, nil)
					cancel()
					if err == nil {
						return nil
					}
				}
			}
		}
		if tries%20 == 0 {
			if e = spawnMode("--coordinator", nil); e != nil {
				return e
			}
		}
		time.Sleep(100 * time.Millisecond)
	}
	return errors.New("could not start the window coordinator")
}
func RunCoordinator() error {
	dir, e := discoveryDir()
	if e != nil {
		return e
	}
	lock, e := lockFile(filepath.Join(dir, "lock"))
	if e != nil {
		return nil
	}
	defer lock.Close()
	listener, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		return e
	}
	defer listener.Close()
	var s *Service
	s = New(settings.Load(), func(b Bootstrap) error {
		return spawnMode("--window", &b, func() { s.ProcessExited(b.Credential) })
	})
	s.address = listener.Addr().String()
	defer s.Store.CloseAll()
	r := record{s.address, token()}
	data, _ := json.Marshal(r)
	name := filepath.Join(dir, "discovery.json")
	tmp := name + ".tmp"
	if e = os.WriteFile(tmp, data, 0600); e != nil {
		return e
	}
	if e = protect(tmp); e != nil {
		return e
	}
	_ = os.Remove(name)
	if e = os.Rename(tmp, name); e != nil {
		return e
	}
	defer os.Remove(name)
	server := &http.Server{Handler: s.handler(r.Credential), ReadHeaderTimeout: 5 * time.Second, ReadTimeout: 5 * time.Second, WriteTimeout: 40 * time.Second, IdleTimeout: 20 * time.Second, MaxHeaderBytes: 4096}
	done := make(chan error, 1)
	go func() { done <- server.Serve(listener) }()
	tick := time.NewTicker(time.Second)
	defer tick.Stop()
	for {
		select {
		case e := <-done:
			return e
		case now := <-tick.C:
			if s.Sweep(now) {
				ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
				defer cancel()
				return server.Shutdown(ctx)
			}
		}
	}
}
func ReadBootstrap() (Bootstrap, error) {
	var b Bootstrap
	e := json.NewDecoder(io.LimitReader(os.Stdin, 4096)).Decode(&b)
	return b, e
}

func (c *Client) Start(tok string, state TabState) error {
	return c.Call(request{Op: "start", Token: tok, State: state}, nil)
}
func (c *Client) StartFeedback(tok string, state TabState) ([]int, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 500*time.Millisecond)
	defer cancel()
	var pids []int
	err := c.call(ctx, request{Op: "start-feedback", Token: tok, State: state}, &pids)
	return pids, err
}
func (c *Client) DragWindows(ctx context.Context, tok string) ([]int, error) {
	var pids []int
	err := c.call(ctx, request{Op: "drag-windows", Token: tok}, &pids)
	return pids, err
}
func (c *Client) End(tok string, cancelled bool, pid int, pos Position) error {
	return c.Call(request{Op: "end", Token: tok, Cancelled: cancelled, PID: pid, Position: pos}, nil)
}

// Handler serves the same private protocol in desktop and development modes.
func (s *Service) Handler(address, credential string) http.Handler {
	s.address = address
	return s.handler(credential)
}

// CreateWindow reserves a process slot; Register begins its frontend lifetime.
func (s *Service) CreateWindow(pos *Position) Bootstrap {
	s.mu.Lock()
	defer s.mu.Unlock()
	_, b := s.newWindow(pos)
	return b
}

func (c *Client) OfferInfo(tok string) (*Offer, error) {
	var offer Offer
	e := c.Call(request{Op: "offer-info", Token: tok}, &offer)
	c.decorate(offer.Info)
	return &offer, e
}

func (c *Client) Hover(ctx context.Context, tok string, pid int, pos Position) error {
	return c.call(ctx, request{Op: "hover", Token: tok, PID: pid, Position: pos}, nil)
}
