// Package session coordinates document ownership between independent windows.
// Only Go processes talk to this service; the web view uses Wails bindings.
package session

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/penguin97040/md-viewer/internal/settings"
	"github.com/penguin97040/md-viewer/internal/store"
)

const transferTTL = 2 * time.Minute
const windowTTL = 45 * time.Second

func token() string {
	var b [32]byte
	if _, err := rand.Read(b[:]); err != nil {
		panic(err)
	}
	return hex.EncodeToString(b[:])
}

type Position struct {
	X int `json:"x"`
	Y int `json:"y"`
}
type Anchor struct {
	Chunk int     `json:"chunk"`
	Frac  float64 `json:"frac"`
}
type TabState struct {
	Anchor       Anchor  `json:"anchor"`
	TocScroll    float64 `json:"tocScroll"`
	WebImages    bool    `json:"webImages"`
	WebDismissed bool    `json:"webDismissed"`
}
type Event struct {
	Name string `json:"name"`
	Data any    `json:"data"`
}
type Offer struct {
	Token string      `json:"token"`
	Info  *store.Info `json:"info"`
	State TabState    `json:"state"`
	Index int         `json:"index"`
}
type DragPoint struct {
	Token    string   `json:"token"`
	Position Position `json:"position"`
}

type Move struct {
	Token       string `json:"token"`
	Key         int    `json:"key"`
	Destination bool   `json:"destination"`
	Empty       bool   `json:"empty"`
}
type OpenResult struct {
	Info      *store.Info `json:"info,omitempty"`
	Elsewhere bool        `json:"elsewhere"`
}
type Bootstrap struct {
	Web        string    `json:"web"`
	Address    string    `json:"address"`
	Credential string    `json:"credential"`
	Position   *Position `json:"position,omitempty"`
}
type window struct {
	pendingPaths map[string]string
	failed       bool
	pid          int
	credential   string
	ready        bool
	visible      bool
	last         time.Time
	focused      uint64
	order        []int
	queue        []Event
	notify       chan struct{}
	position     *Position
}
type transfer struct {
	hoverWindow         string
	nativeDrop          bool
	offered             chan struct{}
	token               string
	key                 int
	source, destination string
	state               TabState
	index               int
	expires             time.Time
}
type document struct {
	owner    string
	info     *store.Info
	identity os.FileInfo
}

// Service serialises registry changes, transfers and settings. Slow parsing is
// outside the registry mutex; per-path promises coalesce simultaneous opens.
type Service struct {
	mu        sync.Mutex
	saveMu    sync.Mutex
	Store     *store.Store
	settings  settings.Settings
	windows   map[string]*window
	docs      map[int]*document
	paths     map[string]int
	opening   map[string]chan struct{}
	transfers map[string]*transfer
	focus     uint64
	lastBusy  time.Time
	stopping  bool
	spawn     func(Bootstrap) error
	address   string
	save      func(settings.Settings) error
}

func New(s settings.Settings, spawn func(Bootstrap) error, persist ...func(settings.Settings) error) *Service {
	x := &Service{settings: s, windows: map[string]*window{}, docs: map[int]*document{}, paths: map[string]int{}, opening: map[string]chan struct{}{}, transfers: map[string]*transfer{}, spawn: spawn, lastBusy: time.Now(), save: settings.Save}
	if len(persist) > 0 {
		x.save = persist[0]
	}
	x.Store = store.New(s.LiveReload, x.changed)
	return x
}
func canonical(path string) (string, error) {
	p, e := filepath.Abs(path)
	if e != nil {
		return "", e
	}
	if real, e := filepath.EvalSymlinks(p); e == nil {
		p = real
	}
	return p, nil
}
func pathKey(p string) string {
	if runtime.GOOS == "windows" || runtime.GOOS == "darwin" {
		return strings.ToLower(p)
	}
	return p
}

// pathDocument revalidates aliases after rename-based saves. The tab's own
// path stays stable, while a replaced hard link becomes a separate file.
// Callers hold s.mu; filesystem metadata is read before taking the lock.
func (s *Service) pathDocument(p string, identity os.FileInfo) int {
	pk := pathKey(p)
	key := s.paths[pk]
	if key != 0 {
		d := s.docs[key]
		if d == nil || (pk != pathKey(d.info.Path) && (identity == nil || d.identity == nil || !os.SameFile(identity, d.identity))) {
			delete(s.paths, pk)
			return 0
		}
	}
	return key
}

func (s *Service) emit(id, name string, data any) {
	w := s.windows[id]
	if w == nil {
		return
	}
	// A bounded queue must never silently discard ownership events. An
	// unresponsive window is disconnected, rolling back its pending moves.
	if len(w.queue) >= 256 {
		w.failed = true
		select {
		case w.notify <- struct{}{}:
		default:
		}
		return
	}
	w.queue = append(w.queue, Event{name, data})
	select {
	case w.notify <- struct{}{}:
	default:
	}
}
func (s *Service) changed(info *store.Info) {
	identity, _ := os.Stat(info.Path)
	s.mu.Lock()
	defer s.mu.Unlock()
	d := s.docs[info.Key]
	if d == nil {
		return
	}
	if info.ID <= d.info.ID {
		return
	}
	d.info = info
	d.identity = identity
	s.emit(d.owner, "doc:changed", info)
	for _, t := range s.transfers {
		if t.key == info.Key && t.destination != "" {
			s.emit(t.destination, "doc:changed", info)
		}
	}
}
func (s *Service) newWindow(pos *Position) (string, Bootstrap) {
	id := token()
	w := &window{pendingPaths: map[string]string{}, credential: token(), last: time.Now(), notify: make(chan struct{}, 1), position: pos}
	s.focus++
	w.focused = s.focus
	s.windows[id] = w
	return id, Bootstrap{Address: s.address, Credential: w.credential, Position: pos, Web: token()}
}
func (s *Service) authenticated(credential string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	for id, w := range s.windows {
		if subtle.ConstantTimeCompare([]byte(w.credential), []byte(credential)) == 1 {
			return id
		}
	}
	return ""
}
func (s *Service) Register(id string, pid int) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	w := s.windows[id]
	if w == nil {
		return errors.New("window expired")
	}
	w.ready = true
	w.pid = pid
	w.last = time.Now()
	return nil
}

// WindowReady is sent by the window process after native visibility is checked.
// Registration alone only proves that the hidden web view can run bindings.
func (s *Service) WindowReady(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	w := s.windows[id]
	if w == nil || !w.ready {
		return errors.New("window is not registered")
	}
	w.visible = true
	return nil
}

// ProcessExited is called by the coordinator's child waiter, including children
// that failed before registration. It never trusts a frontend-supplied pid.
func (s *Service) ProcessExited(credential string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for id, w := range s.windows {
		if w.credential == credential {
			s.remove(id)
			return
		}
	}
}
func (s *Service) Focus(id string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	w := s.windows[id]
	if w == nil {
		return errors.New("window expired")
	}
	s.focus++
	w.focused = s.focus
	return nil
}
func (s *Service) recent() string {
	var id string
	var n uint64
	for k, w := range s.windows {
		if w.focused >= n {
			id = k
			n = w.focused
		}
	}
	return id
}

// Launch routes each existing file to its owner, and new files to the most
// recently focused window. A newly spawned window waits for Register.
func (s *Service) Launch(paths []string) error {
	identities := make([]os.FileInfo, len(paths))
	for i, p := range paths {
		abs, e := canonical(p)
		if e != nil {
			return e
		}
		paths[i] = abs
		identities[i], _ = os.Stat(abs)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.stopping {
		return errors.New("session is stopping")
	}
	s.lastBusy = time.Now()
	target := s.recent()
	if target == "" {
		var b Bootstrap
		target, b = s.newWindow(nil)
		if err := s.spawn(b); err != nil {
			s.remove(target)
			return err
		}
	}
	if len(paths) == 0 {
		s.emit(target, "window:focus", nil)
	}
	groups := map[string][]string{}
	var destinations []string
	for i, p := range paths {
		id := target
		if key := s.pathDocument(p, identities[i]); key != 0 {
			if d := s.docs[key]; d != nil {
				id = d.owner
			}
		}
		if len(groups[id]) == 0 {
			destinations = append(destinations, id)
		}
		groups[id] = append(groups[id], p)
		if s.paths[pathKey(p)] == 0 {
			s.windows[id].pendingPaths[pathKey(p)] = p
		}
	}
	for _, id := range destinations {
		s.emit(id, "open:paths", groups[id])
	}
	return nil
}
func (s *Service) Poll(ctx context.Context, id string) ([]Event, error) {
	s.mu.Lock()
	w := s.windows[id]
	if w == nil || w.failed {
		if w != nil {
			s.remove(id)
		}
		s.mu.Unlock()
		return nil, errors.New("window expired or event queue overflowed")
	}
	w.last = time.Now()
	notify := w.notify
	s.mu.Unlock()
	select {
	case <-ctx.Done():
		return nil, ctx.Err()
	case <-notify:
	case <-time.After(10 * time.Second):
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	w = s.windows[id]
	if w == nil {
		return nil, errors.New("window expired")
	}
	w.last = time.Now()
	q := w.queue
	w.queue = nil
	return q, nil
}
func (s *Service) owns(id string, key int, read bool) bool {
	d := s.docs[key]
	if d == nil {
		return false
	}
	if d.owner == id {
		return true
	}
	if read {
		for _, t := range s.transfers {
			if t.key == key && t.destination == id {
				return true
			}
		}
	}
	return false
}
func (s *Service) Open(id, path string) (*OpenResult, error) {
	caller := id
	p, err := canonical(path)
	if err != nil {
		return nil, err
	}
	pk := pathKey(p)
	for {
		identity, _ := os.Stat(p)
		s.mu.Lock()
		if s.windows[id] == nil {
			s.mu.Unlock()
			return nil, errors.New("window expired")
		}
		delete(s.windows[id].pendingPaths, pk)
		if key := s.pathDocument(p, identity); key != 0 {
			d := s.docs[key]
			if d.owner != id {
				s.emit(d.owner, "open:paths", []string{d.info.Path})
				s.emit(d.owner, "window:focus", nil)
				s.mu.Unlock()
				return &OpenResult{Elsewhere: true}, nil
			}
			info := d.info
			s.mu.Unlock()
			return &OpenResult{Info: info}, nil
		}
		if wait := s.opening[pk]; wait != nil {
			s.mu.Unlock()
			<-wait
			continue
		}
		wait := make(chan struct{})
		s.opening[pk] = wait
		s.mu.Unlock()
		info, e := s.Store.Open(p)
		identity, _ = os.Stat(p)
		s.mu.Lock()
		delete(s.opening, pk)
		close(wait)
		if e != nil {
			s.mu.Unlock()
			return nil, e
		}
		if s.windows[id] == nil {
			// A tab move can close its source while a launch is still parsing. Hand
			// that completed parse to a surviving window rather than reading it twice.
			target := s.recent()
			if target == "" {
				s.Store.Close(info.Key)
				s.mu.Unlock()
				return nil, errors.New("window closed during open")
			}
			id = target
			s.emit(target, "open:paths", []string{p})
		}
		// Symlinks are canonicalised above; hard links are compared by file id.
		for key, d := range s.docs {
			if identity != nil && d.identity != nil && os.SameFile(identity, d.identity) {
				s.Store.Close(info.Key)
				s.paths[pk] = key
				if d.owner != id {
					s.emit(d.owner, "open:paths", []string{d.info.Path})
					s.emit(d.owner, "window:focus", nil)
					s.mu.Unlock()
					return &OpenResult{Elsewhere: true}, nil
				}
				res := d.info
				s.mu.Unlock()
				return &OpenResult{Info: res}, nil
			}
		}
		if current, e := s.Store.Current(info.Key); e == nil {
			info = current
		}
		s.docs[info.Key] = &document{id, info, identity}
		s.paths[pk] = info.Key
		s.windows[id].order = append(s.windows[id].order, info.Key)
		s.mu.Unlock()
		if caller != id {
			return &OpenResult{Elsewhere: true}, nil
		}
		return &OpenResult{Info: info}, nil
	}
}
func (s *Service) Info(id string, key int) (*store.Info, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.owns(id, key, true) {
		return nil, errors.New("document belongs to another window")
	}
	return s.docs[key].info, nil
}
func (s *Service) ReadID(id string, parse int) (int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	for key, d := range s.docs {
		if d.info.ID == parse && s.owns(id, key, true) {
			return key, nil
		}
	}
	return 0, errors.New("stale or foreign document")
}
func (s *Service) Reload(id string, key int) (*store.Info, error) {
	s.mu.Lock()
	ok := s.owns(id, key, false)
	s.mu.Unlock()
	if !ok {
		return nil, errors.New("document belongs to another window")
	}
	info, err := s.Store.Reload(key)
	if err != nil {
		return nil, err
	}
	s.changed(info)
	return s.Info(id, key)
}
func removeKey(a []int, key int) []int {
	for i, k := range a {
		if k == key {
			return append(a[:i], a[i+1:]...)
		}
	}
	return a
}
func (s *Service) Close(id string, key int) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.owns(id, key, false) {
		return errors.New("document belongs to another window")
	}
	for _, t := range s.transfers {
		if t.key == key {
			s.abort(t)
		}
	}
	s.drop(key)
	return nil
}
func (s *Service) drop(key int) {
	if d := s.docs[key]; d != nil {
		if w := s.windows[d.owner]; w != nil {
			w.order = removeKey(w.order, key)
		}
		delete(s.docs, key)
		for p, k := range s.paths {
			if k == key {
				delete(s.paths, p)
			}
		}
		s.Store.Close(key)
	}
}
func (s *Service) Order(id string, keys []int) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	w := s.windows[id]
	if w == nil {
		return errors.New("window expired")
	}
	if len(keys) != len(w.order) {
		return errors.New("tab order changed")
	}
	seen := map[int]bool{}
	for _, key := range keys {
		if seen[key] || !s.owns(id, key, false) {
			return errors.New("invalid tab order")
		}
		seen[key] = true
	}
	w.order = append([]int(nil), keys...)
	return nil
}
func (s *Service) Begin(id string, key int, state TabState) (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if !s.owns(id, key, false) {
		return "", errors.New("document belongs to another window")
	}
	for _, t := range s.transfers {
		if t.key == key {
			return "", errors.New("tab is already moving")
		}
	}
	if state.Anchor.Chunk < 0 || state.Anchor.Frac < 0 || state.Anchor.Frac > 1 || state.TocScroll < 0 {
		return "", errors.New("invalid reading position")
	}
	tok := token()
	s.transfers[tok] = &transfer{token: tok, key: key, source: id, state: state, expires: time.Now().Add(transferTTL), offered: make(chan struct{})}
	return tok, nil
}
func (s *Service) Accept(id, tok string, index int) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.accept(id, tok, index)
}
func (s *Service) accept(id, tok string, index int) error {
	t := s.transfers[tok]
	if t == nil || time.Now().After(t.expires) || t.destination != "" || s.windows[id] == nil || t.source == id {
		return errors.New("transfer is no longer available")
	}
	if !s.owns(t.source, t.key, false) {
		return errors.New("source no longer owns tab")
	}
	if index < 0 {
		index = 0
	}
	if index > len(s.windows[id].order) {
		index = len(s.windows[id].order)
	}
	t.destination = id
	if t.hoverWindow != "" {
		s.emit(t.hoverWindow, "tab:leave", nil)
		t.hoverWindow = ""
	}
	close(t.offered)
	t.index = index
	s.emit(id, "tab:offer", Offer{tok, s.docs[t.key].info, t.state, index})
	return nil
}

// Start updates reading state at the actual drag start, rather than at the
// earlier pointer press that armed the synchronous browser data transfer.
func (s *Service) Start(id, tok string, state TabState) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.transfers[tok]
	if t == nil || t.source != id || t.destination != "" || time.Now().After(t.expires) {
		return errors.New("transfer is no longer available")
	}
	if state.Anchor.Chunk < 0 || state.Anchor.Frac < 0 || state.Anchor.Frac > 1 || state.TocScroll < 0 {
		return errors.New("invalid reading position")
	}
	t.state = state
	return nil
}

// DragWindows exposes only registered viewer processes, and only to the owner
// of a live outside drag. Native feedback uses them to leave viewers uncovered.
func (s *Service) DragWindows(id, tok string) ([]int, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.transfers[tok]
	if t == nil || t.source != id || t.destination != "" || t.nativeDrop || time.Now().After(t.expires) || !s.owns(id, t.key, false) {
		return nil, errors.New("transfer is no longer available")
	}
	seen := make(map[int]bool)
	for _, w := range s.windows {
		if w.ready && w.pid > 0 {
			seen[w.pid] = true
		}
	}
	pids := make([]int, 0, len(seen))
	for pid := range seen {
		pids = append(pids, pid)
	}
	sort.Ints(pids)
	return pids, nil
}

// End never undoes a drop already accepted by another window. A native
// cancellation or release anywhere inside a viewer simply rolls back.
func (s *Service) End(id, tok string, cancelled bool, pid int, pos Position) error {
	s.mu.Lock()
	t := s.transfers[tok]
	if t == nil {
		s.mu.Unlock()
		return nil
	}
	if t.source != id {
		s.mu.Unlock()
		return errors.New("foreign transfer")
	}
	if t.destination != "" {
		s.mu.Unlock()
		return nil
	}
	for target, w := range s.windows {
		if pid != 0 && w.pid == pid {
			if !cancelled {
				// Some web views omit DOM drop events for native sessions. Their adapter
				// routes this authenticated source token to the hit window; its frontend
				// still accepts only a release within the strip.
				if !t.nativeDrop {
					t.nativeDrop = true
					s.emit(target, "tab:drop", DragPoint{tok, pos})
				}
				offered := t.offered
				s.mu.Unlock()
				select {
				case <-offered:
				case <-time.After(500 * time.Millisecond):
				}
				s.mu.Lock()
				t = s.transfers[tok]
				if t == nil || t.destination != "" {
					s.mu.Unlock()
					return nil
				}
			}
			cancelled = true
			break
		}
	}
	if cancelled {
		s.abort(t)
		s.mu.Unlock()
		return nil
	}
	s.mu.Unlock()
	return s.Detach(id, tok, pos)
}
func (s *Service) Detach(id, tok string, pos Position) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.transfers[tok]
	if t == nil || t.source != id || t.destination != "" || time.Now().After(t.expires) {
		return errors.New("transfer is no longer available")
	}
	dest, b := s.newWindow(&pos)
	if err := s.spawn(b); err != nil {
		s.remove(dest)
		s.abort(t)
		return err
	}
	return s.accept(dest, tok, 0)
}

// OfferInfo reads a pending move without acknowledging content. It lets a
// destination recover a stale prefetch after a reload invalidated its parse id.
func (s *Service) OfferInfo(id, tok string) (*Offer, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.transfers[tok]
	if t == nil || t.destination != id || time.Now().After(t.expires) {
		return nil, errors.New("transfer is no longer available")
	}
	d := s.docs[t.key]
	if d == nil {
		return nil, errors.New("document closed")
	}
	return &Offer{tok, d.info, t.state, t.index}, nil
}

// Ack commits once, and only after the destination loaded the current parse.
// A reload overtaking that load returns a fresh offer rather than committing.
func (s *Service) Ack(id, tok string, parse int) (*Offer, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.transfers[tok]
	if t == nil || t.destination != id || time.Now().After(t.expires) {
		return nil, errors.New("transfer is no longer available")
	}
	d := s.docs[t.key]
	if d == nil || d.owner != t.source {
		return nil, errors.New("source no longer owns tab")
	}
	if d.info.ID != parse {
		return &Offer{tok, d.info, t.state, t.index}, nil
	}
	source := s.windows[t.source]
	dest := s.windows[id]
	if source == nil || dest == nil {
		return nil, errors.New("window expired")
	}
	if !dest.ready || !dest.visible {
		return nil, errors.New("destination window is not visible")
	}
	source.order = removeKey(source.order, t.key)
	index := min(t.index, len(dest.order))
	dest.order = append(dest.order, 0)
	copy(dest.order[index+1:], dest.order[index:])
	dest.order[index] = t.key
	d.owner = id
	delete(s.transfers, tok)
	if t.hoverWindow != "" {
		s.emit(t.hoverWindow, "tab:leave", nil)
	}
	s.emit(t.source, "tab:committed", Move{tok, t.key, false, len(source.order) == 0})
	s.emit(id, "tab:committed", Move{tok, t.key, true, false})
	return nil, nil
}
func (s *Service) Cancel(id, tok string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.transfers[tok]
	if t == nil {
		return nil
	}
	if t.source != id && t.destination != id {
		return errors.New("foreign transfer")
	}
	s.abort(t)
	return nil
}

// Discard closes an incoming tab even if its acknowledgement committed just
// before the user closed it. Cancellation and ownership checks share the lock,
// so an uncommitted move always leaves the source document open.
func (s *Service) Discard(id, tok string, key int) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if t := s.transfers[tok]; t != nil {
		if t.destination != id || t.key != key {
			return errors.New("foreign transfer")
		}
		s.abort(t)
		return nil
	}
	if s.owns(id, key, false) {
		s.drop(key)
	}
	return nil
}

func (s *Service) abort(t *transfer) {
	delete(s.transfers, t.token)
	if t.destination == "" {
		close(t.offered)
	}
	if t.hoverWindow != "" {
		s.emit(t.hoverWindow, "tab:leave", nil)
	}
	s.emit(t.source, "tab:aborted", Move{Token: t.token, Key: t.key})
	if t.destination != "" {
		s.emit(t.destination, "tab:aborted", Move{Token: t.token, Key: t.key, Destination: true})
		if w := s.windows[t.destination]; w != nil && w.position != nil && len(w.order) == 0 && len(w.pendingPaths) == 0 {
			s.emit(t.destination, "window:close", nil)
		}
	}
}
func (s *Service) remove(id string) {
	w := s.windows[id]
	if w == nil {
		return
	}
	delete(s.windows, id)
	close(w.notify)
	for _, t := range s.transfers {
		if t.source == id || t.destination == id {
			s.abort(t)
		}
	}
	for key, d := range s.docs {
		if d.owner == id {
			s.drop(key)
		}
	}
	if target := s.recent(); target != "" {
		var pending []string
		for pk, path := range w.pendingPaths {
			s.windows[target].pendingPaths[pk] = path
			pending = append(pending, path)
		}
		if len(pending) > 0 {
			s.emit(target, "open:paths", pending)
		}
	}
	s.lastBusy = time.Now()
}
func (s *Service) Unregister(id string) { s.mu.Lock(); defer s.mu.Unlock(); s.remove(id) }
func (s *Service) Sweep(now time.Time) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, t := range s.transfers {
		if !now.Before(t.expires) {
			s.abort(t)
		}
	}
	for id, w := range s.windows {
		if w.failed || now.Sub(w.last) > windowTTL {
			s.remove(id)
		}
	}
	if len(s.windows) == 0 && len(s.opening) == 0 && now.Sub(s.lastBusy) > 2*time.Second {
		s.stopping = true
		return true
	}
	return false
}
func (s *Service) Settings() settings.Settings { s.mu.Lock(); defer s.mu.Unlock(); return s.settings }
func (s *Service) SaveSettings(next settings.Settings) error {
	s.saveMu.Lock()
	defer s.saveMu.Unlock()
	next = next.Clean()
	if err := s.save(next); err != nil {
		return err
	}
	s.Store.SetLive(next.LiveReload)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.settings = next
	for id := range s.windows {
		s.emit(id, "settings:changed", next)
	}
	return nil
}

// Local images are authorised against this window's documents only, including
// its pending incoming tab. A different window cannot grant network access.
func (s *Service) Reachable(id, path string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	for key, d := range s.docs {
		if !s.owns(id, key, true) {
			continue
		}
		if store.ReachableFrom(d.info.Path, path) {
			return true
		}
	}
	return false
}
func (s *Service) String() string {
	s.mu.Lock()
	defer s.mu.Unlock()
	return fmt.Sprintf("%d windows, %d documents", len(s.windows), len(s.docs))
}

// MarshalEvent keeps typed payloads inside the service and a compact opaque
// JSON payload in window processes.
func MarshalEvent(e Event) json.RawMessage { b, _ := json.Marshal(e.Data); return b }

func (s *Service) Hover(id, tok string, pid int, pos Position) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	t := s.transfers[tok]
	if t == nil || t.source != id || time.Now().After(t.expires) {
		return errors.New("transfer is no longer available")
	}
	if t.nativeDrop || t.destination != "" {
		return nil
	}
	target := ""
	for key, w := range s.windows {
		if pid != 0 && w.pid == pid {
			target = key
			break
		}
	}
	if t.hoverWindow != "" && t.hoverWindow != target {
		s.emit(t.hoverWindow, "tab:leave", nil)
	}
	t.hoverWindow = target
	if target != "" {
		s.emit(target, "tab:hover", DragPoint{tok, pos})
	}
	return nil
}
