package main

import (
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"strconv"
	"strings"
	"sync"

	"github.com/penguin97040/md-viewer/internal/doc"
	"github.com/penguin97040/md-viewer/internal/session"
	"github.com/penguin97040/md-viewer/internal/settings"
	"github.com/penguin97040/md-viewer/internal/store"
	"github.com/penguin97040/md-viewer/internal/webimage"
)

// The session preview uses the production Go protocol with one window client
// per browser page. Native release detection is deliberately absent here.
const sessionShim = `
const windowLabel = WINDOW_LABEL;
const handlers = new Map();
let running = false;
const call = (m) => async (...args) => {
 const r = await fetch('/api/' + m + '?window=' + encodeURIComponent(windowLabel), {method:'POST',headers:{'X-Devshim':'1'},body:JSON.stringify(args)});
 const j = await r.json();if(j.error)throw new Error(j.error);return j.result;
};
async function events() {
 while(running) {
  try { const list = await call('Poll')();for(const e of list || [])for(const fn of handlers.get(e.name)||[])fn(e.data); }
  catch(e){running=false;for(const fn of handlers.get('doc:error')||[])fn(e.message);}
 }
}
let previewMaximised = false;
window.runtime = {
  WindowMinimise() {},
  WindowToggleMaximise() { previewMaximised = !previewMaximised; window.dispatchEvent(new Event('resize')); },
  WindowIsMaximised: async () => previewMaximised,
 EventsOn(name,fn){if(!handlers.has(name))handlers.set(name,[]);handlers.get(name).push(fn);},
 EventsOff(name){handlers.delete(name);},OnFileDrop(){},
 ClipboardSetText:t=>navigator.clipboard.writeText(t).then(()=>true,()=>false),
};
window.go={main:{App:new Proxy({}, {get:(_,m)=>{
 if(m==='Initial')return async()=>{const r=await call(m)();running=true;events();return r;};
 if(m==='CloseWindow')return async()=>{running=false;await call(m)();window.previewClosed=true;};
 return call(m);
}})}};
`

type sessionPreview struct {
	mu      sync.Mutex
	service *session.Service
	clients map[string]*session.Client
	start   []string
	started map[string]bool
	serial  int
}

func runSession(addr string, paths []string) {
	p := &sessionPreview{clients: map[string]*session.Client{}, started: map[string]bool{}, start: paths}
	p.service = session.New(settings.Defaults(), func(b session.Bootstrap) error {
		p.mu.Lock()
		defer p.mu.Unlock()
		p.serial++
		label := fmt.Sprintf("detached%d", p.serial)
		c, e := session.NewClient(b)
		if e != nil {
			return e
		}
		p.clients[label] = c
		return nil
	}, func(settings.Settings) error { return nil })
	rpc, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		log.Fatal(e)
	}
	server := &http.Server{Handler: p.service.Handler(rpc.Addr().String(), strings.Repeat("0", 64))}
	go server.Serve(rpc)
	mux := http.NewServeMux()
	mux.HandleFunc("/api/", p.api)
	mux.HandleFunc("/devshim.js", func(w http.ResponseWriter, r *http.Request) {
		label, _ := json.Marshal(p.label(r))
		w.Header().Set("Content-Type", "text/javascript")
		io.WriteString(w, strings.Replace(sessionShim, "WINDOW_LABEL", string(label), 1))
	})
	mux.HandleFunc("/highlight.css", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/css")
		io.WriteString(w, doc.HighlightCSS())
	})
	mux.HandleFunc("/local/", func(w http.ResponseWriter, r *http.Request) {
		parts := strings.SplitN(strings.TrimPrefix(r.URL.Path, "/local/"), "/", 2)
		if len(parts) != 2 {
			http.NotFound(w, r)
			return
		}
		key, e := strconv.Atoi(parts[0])
		if e != nil {
			http.NotFound(w, r)
			return
		}
		path := store.LocalPath("/local/" + parts[1])
		ct, ok := store.ImageType(path)
		if !ok {
			http.NotFound(w, r)
			return
		}
		// Assets cannot carry the page query, so locate a client permitted to read
		// this key. Desktop processes instead have exactly one window credential.
		p.mu.Lock()
		clients := make([]*session.Client, 0, len(p.clients))
		for _, c := range p.clients {
			clients = append(clients, c)
		}
		p.mu.Unlock()
		for _, c := range clients {
			if data, e := c.Local(key, path); e == nil {
				w.Header().Set("Content-Type", ct)
				w.Header().Set("X-Content-Type-Options", "nosniff")
				if ct == "image/svg+xml" {
					w.Header().Set("Content-Security-Policy", "script-src 'none'")
				}
				w.Write(data)
				return
			}
		}
		http.NotFound(w, r)
	})
	front := http.FileServer(http.Dir("frontend"))
	mux.HandleFunc("/", func(w http.ResponseWriter, r *http.Request) {
		if strings.HasPrefix(r.URL.Path, "/web/") {
			p.mu.Lock()
			allowed := false
			for _, c := range p.clients {
				if c.WebPath() == r.URL.Path {
					allowed = true
					break
				}
			}
			p.mu.Unlock()
			if allowed {
				webimage.Serve(w, r)
				return
			}
			http.NotFound(w, r)
			return
		}
		if r.URL.Path == "/" {
			label := p.label(r)
			p.mu.Lock()
			_, exists := p.clients[label]
			p.mu.Unlock()
			if !exists {
				b := p.service.CreateWindow(nil)
				c, e := session.NewClient(b)
				if e != nil {
					http.Error(w, e.Error(), 500)
					return
				}
				p.mu.Lock()
				p.clients[label] = c
				p.mu.Unlock()
			}
			b, e := os.ReadFile("frontend/index.html")
			if e != nil {
				http.Error(w, e.Error(), 500)
				return
			}
			html := strings.Replace(string(b), "<link", "<script src=\"/devshim.js?window="+label+"\"></script><link", 1)
			w.Header().Set("Content-Type", "text/html")
			io.WriteString(w, html)
			return
		}
		if serveCompressed(w, r, os.DirFS("frontend")) {
			return
		}
		front.ServeHTTP(w, r)
	})
	log.Printf("session preview on http://%s", addr)
	log.Fatal(http.ListenAndServe(addr, localOnly(addr, mux)))
}
func (p *sessionPreview) label(r *http.Request) string {
	label := r.URL.Query().Get("window")
	if label == "" {
		return "one"
	}
	if len(label) > 40 {
		return "invalid"
	}
	for _, c := range label {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9') {
			return "invalid"
		}
	}
	return label
}
func (p *sessionPreview) api(w http.ResponseWriter, r *http.Request) {
	if r.Method != "POST" || r.Header.Get("X-Devshim") != "1" {
		http.Error(w, "forbidden", 403)
		return
	}
	var args []json.RawMessage
	if e := json.NewDecoder(http.MaxBytesReader(w, r.Body, 64<<10)).Decode(&args); e != nil {
		http.Error(w, "invalid request", 400)
		return
	}
	var argErr error
	arg := func(i int, v any) {
		if i < len(args) {
			if e := json.Unmarshal(args[i], v); e != nil {
				argErr = e
			}
		}
	}
	label := p.label(r)
	p.mu.Lock()
	c := p.clients[label]
	p.mu.Unlock()
	if c == nil {
		json.NewEncoder(w).Encode(map[string]any{"error": "preview window closed"})
		return
	}
	var res any
	var err error
	var key int
	method := strings.TrimPrefix(r.URL.Path, "/api/")
	switch method {
	case "Close", "Reload", "ResolveLink", "Chunk", "Search", "BeginTransfer":
		arg(0, &key)
	}
	switch method {
	case "Initial":
		err = c.Register()
		p.mu.Lock()
		first := !p.started[label]
		p.started[label] = true
		p.mu.Unlock()
		if first && label == "one" {
			res = p.start
		} else {
			res = []string{}
		}
	case "Poll":
		events, e := c.Poll(r.Context())
		err = e
		for i, event := range events {
			events[i] = c.DecorateEvent(event)
		}
		res = events
	case "Settings":
		res, err = c.Settings()
	case "SaveSettings":
		var s settings.Settings
		arg(0, &s)
		err = c.SaveSettings(s)
	case "Open":
		var path string
		argErr = nil
		arg(0, &path)
		var result *session.OpenResult
		result, err = c.Open(path)
		if result != nil {
			res = result.Info
		}
	case "Close":
		err = c.Close(key)
	case "Reload":
		res, err = c.Reload(key)
	case "ResolveLink":
		var href string
		arg(1, &href)
		res, err = c.Resolve(key, href)
	case "Chunk":
		var index int
		arg(1, &index)
		res, err = c.Chunk(key, index)
	case "Search":
		var q string
		arg(1, &q)
		res, err = c.Search(key, q)
	case "FocusWindow":
		err = c.Focus()
	case "WindowReady":
		err = c.WindowReady()
	case "TabOrder":
		var keys []int
		argErr = nil
		arg(0, &keys)
		err = c.Order(keys)
	case "BeginTransfer":
		var state session.TabState
		arg(1, &state)
		res, err = c.Begin(key, state)
	case "StartTabDrag":
		var token string
		var state session.TabState
		argErr = nil
		arg(0, &token)
		arg(1, &state)
		err = c.Start(token, state)
	case "EndTabDrag":
		var token string
		argErr = nil
		arg(0, &token)
		err = c.End(token, true, 0, session.Position{})
	case "AcceptTransfer", "CancelTransfer", "DiscardTransfer", "AcknowledgeTransfer", "DetachTransfer", "TransferInfo":
		var token string
		argErr = nil
		arg(0, &token)
		switch method {
		case "AcceptTransfer":
			var index int
			arg(1, &index)
			err = c.Accept(token, index)
		case "CancelTransfer":
			err = c.Cancel(token)
		case "DiscardTransfer":
			arg(1, &key)
			err = c.Discard(token, key)
		case "AcknowledgeTransfer":
			var id int
			arg(1, &id)
			res, err = c.Ack(token, id)
		case "TransferInfo":
			res, err = c.OfferInfo(token)
		case "DetachTransfer":
			err = c.Detach(token, session.Position{})
		}
	case "Windows":
		p.mu.Lock()
		var labels []string
		for label := range p.clients {
			labels = append(labels, label)
		}
		p.mu.Unlock()
		res = labels
	case "CloseWindow":
		err = c.Unregister()
		p.mu.Lock()
		delete(p.clients, label)
		p.mu.Unlock()
	case "Version":
		res = "preview"
	case "SetTitle", "Print", "OpenURL", "PickFile":
		res = ""
	default:
		err = fmt.Errorf("unsupported preview binding %s", method)
	}
	if argErr != nil {
		err = argErr
	}
	out := map[string]any{"result": res}
	if err != nil {
		out["error"] = err.Error()
	}
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(out)
}
