package session

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"github.com/penguin97040/md-viewer/internal/settings"
)

func fixture(t *testing.T) (*Service, string, string, string) {
	t.Helper()
	s := New(settings.Defaults(), func(Bootstrap) error { return nil })
	t.Cleanup(s.Store.CloseAll)
	a, _ := s.newWindow(nil)
	b, _ := s.newWindow(nil)
	if err := s.Register(a, 100); err != nil {
		t.Fatal(err)
	}
	if err := s.Register(b, 200); err != nil {
		t.Fatal(err)
	}
	if err := s.WindowReady(a); err != nil {
		t.Fatal(err)
	}
	if err := s.WindowReady(b); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(t.TempDir(), "file.md")
	if err := os.WriteFile(path, []byte("# Heading\n\nA paragraph with a match.\n"), 0600); err != nil {
		t.Fatal(err)
	}
	return s, a, b, path
}
func opened(t *testing.T, s *Service, id, p string) int {
	t.Helper()
	r, e := s.Open(id, p)
	if e != nil || r.Info == nil {
		t.Fatalf("open: %v %#v", e, r)
	}
	return r.Info.Key
}
func begin(t *testing.T, s *Service, id string, key int) string {
	t.Helper()
	tok, e := s.Begin(id, key, TabState{Anchor: Anchor{0, .35}, TocScroll: 72, WebImages: true})
	if e != nil {
		t.Fatal(e)
	}
	return tok
}
func drain(s *Service, id string) []Event {
	s.mu.Lock()
	defer s.mu.Unlock()
	if w := s.windows[id]; w != nil {
		q := w.queue
		w.queue = nil
		return q
	}
	return nil
}
func TestSimultaneousOpenDeduplicates(t *testing.T) {
	s, a, b, path := fixture(t)
	var wg sync.WaitGroup
	var mu sync.Mutex
	keys := map[int]bool{}
	counts := map[string]int{}
	for i := 0; i < 32; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			id := a
			if i%2 == 0 {
				id = b
			}
			r, e := s.Open(id, path)
			if e != nil {
				t.Error(e)
				return
			}
			mu.Lock()
			defer mu.Unlock()
			if r.Info != nil {
				keys[r.Info.Key] = true
				counts[id]++
			}
		}(i)
	}
	wg.Wait()
	if len(keys) != 1 || len(s.docs) != 1 || len(s.paths) != 1 || len(s.opening) != 0 || len(counts) != 1 {
		t.Fatalf("duplicate entries: %v, %v, %s", keys, counts, s)
	}
	for key := range keys {
		if _, e := s.Store.Current(key + 1); e == nil {
			t.Fatal("redundant parse/store entry")
		}
	}
}
func TestAliasesAndCase(t *testing.T) {
	s, a, b, path := fixture(t)
	key := opened(t, s, a, path)
	link := filepath.Join(filepath.Dir(path), "alias.md")
	if e := os.Symlink(path, link); e == nil {
		r, e := s.Open(b, link)
		if e != nil || !r.Elsewhere {
			t.Fatalf("symlink: %v %#v", e, r)
		}
	}
	hard := filepath.Join(filepath.Dir(path), "hard.md")
	if e := os.Link(path, hard); e == nil {
		r, e := s.Open(b, hard)
		if e != nil || !r.Elsewhere {
			t.Fatalf("hardlink: %v %#v", e, r)
		}
	}
	if len(s.docs) != 1 {
		t.Fatal("alias created duplicate")
	}
	if e := s.Close(a, key); e != nil {
		t.Fatal(e)
	}
	if len(s.paths) != 0 {
		t.Fatal("alias registry survived close")
	}
}
func TestReplacedHardLinkAlias(t *testing.T) {
	for _, replacePrimary := range []bool{false, true} {
		t.Run(fmt.Sprintf("primary=%v", replacePrimary), func(t *testing.T) {
			s, a, b, path := fixture(t)
			s.Store.SetLive(false)
			key := opened(t, s, a, path)
			alias := filepath.Join(filepath.Dir(path), "hard.md")
			if err := os.Link(path, alias); err != nil {
				t.Skipf("hard links unavailable: %v", err)
			}
			if r, err := s.Open(b, alias); err != nil || !r.Elsewhere {
				t.Fatalf("initial alias: %v %#v", err, r)
			}
			replacement := filepath.Join(filepath.Dir(path), "replacement.md")
			if err := os.WriteFile(replacement, []byte("# Replacement\n"), 0600); err != nil {
				t.Fatal(err)
			}
			target := alias
			if replacePrimary {
				target = path
			}
			if err := os.Rename(replacement, target); err != nil {
				t.Fatal(err)
			}
			if replacePrimary {
				if _, err := s.Reload(a, key); err != nil {
					t.Fatal(err)
				}
			}
			drain(s, a)
			drain(s, b)
			if err := s.Focus(b); err != nil {
				t.Fatal(err)
			}
			if err := s.Launch([]string{alias}); err != nil {
				t.Fatal(err)
			}
			if len(drain(s, a)) != 0 || len(drain(s, b)) != 1 {
				t.Fatal("replaced alias was routed to its former owner")
			}
			r, err := s.Open(b, alias)
			if err != nil || r.Info == nil || r.Info.Key == key || len(s.docs) != 2 {
				t.Fatalf("distinct file still treated as alias: %v %#v", err, r)
			}
			want := "Replacement"
			if replacePrimary {
				want = "Heading"
			}
			if r.Info.Headings[0].Text != want {
				t.Fatalf("opened wrong content: %q", r.Info.Headings[0].Text)
			}
		})
	}
}

func TestDiscardIncomingBeforeAndAfterCommit(t *testing.T) {
	for _, committed := range []bool{false, true} {
		t.Run(fmt.Sprintf("committed=%v", committed), func(t *testing.T) {
			s, a, b, path := fixture(t)
			key := opened(t, s, a, path)
			tok := begin(t, s, a, key)
			if err := s.Accept(b, tok, 0); err != nil {
				t.Fatal(err)
			}
			if err := s.Discard(a, tok, key); err == nil {
				t.Fatal("source discarded a destination transfer")
			}
			if committed {
				info, _ := s.Info(a, key)
				if offer, err := s.Ack(b, tok, info.ID); err != nil || offer != nil {
					t.Fatalf("ack: %v %#v", err, offer)
				}
			}
			for range 2 {
				if err := s.Discard(b, tok, key); err != nil {
					t.Fatal(err)
				}
			}
			if len(s.transfers) != 0 || len(s.windows[b].order) != 0 {
				t.Fatal("discard left a pending transfer or destination tab")
			}
			_, err := s.Store.Current(key)
			if committed {
				if err == nil || len(s.docs) != 0 {
					t.Fatal("committed incoming close left the document open")
				}
			} else if err != nil || !s.owns(a, key, false) {
				t.Fatal("uncommitted incoming close removed the source document")
			}
		})
	}
}

func TestTransferAcknowledgementAndOwnership(t *testing.T) {
	s, a, b, path := fixture(t)
	key := opened(t, s, a, path)
	tok := begin(t, s, a, key)
	before, _ := s.Info(a, key)
	if _, e := s.Info(b, key); e == nil {
		t.Fatal("foreign metadata readable")
	}
	if e := s.Accept(b, tok, 0); e != nil {
		t.Fatal(e)
	}
	if _, e := s.Info(b, key); e != nil {
		t.Fatal("incoming document unreadable", e)
	}
	if e := s.Close(b, key); e == nil {
		t.Fatal("incoming window closed source document")
	}
	offer, e := s.Ack(b, tok, before.ID)
	if e != nil || offer != nil {
		t.Fatalf("ack %v %#v", e, offer)
	}
	if _, e := s.Info(a, key); e == nil {
		t.Fatal("former owner retained access")
	}
	after, e := s.Info(b, key)
	if e != nil || after.Key != key || after.ID != before.ID {
		t.Fatal("transfer reparsed document", e)
	}
	if _, e := s.Ack(b, tok, before.ID); e == nil {
		t.Fatal("duplicate commit accepted")
	}
	if e := s.Cancel(a, tok); e != nil {
		t.Fatal(e)
	}
	q := drain(s, a)
	if len(q) != 1 || q[0].Name != "tab:committed" || !q[0].Data.(Move).Empty {
		t.Fatalf("last-tab event %#v", q)
	}
	s.Unregister(a)
	if _, e := s.Info(b, key); e != nil {
		t.Fatal("closing original window broke destination", e)
	}
}
func TestReloadOvertakesTransfer(t *testing.T) {
	s, a, b, path := fixture(t)
	key := opened(t, s, a, path)
	old, _ := s.Info(a, key)
	tok := begin(t, s, a, key)
	_ = s.Accept(b, tok, 0)
	if e := os.WriteFile(path, []byte("# New heading\n\nChanged match."), 0600); e != nil {
		t.Fatal(e)
	}
	fresh, e := s.Reload(a, key)
	if e != nil {
		t.Fatal(e)
	}
	offer, e := s.Ack(b, tok, old.ID)
	if e != nil || offer == nil || offer.Info.ID != fresh.ID || offer.State.Anchor.Frac != .35 || !offer.State.WebImages {
		t.Fatalf("fresh offer %#v %v", offer, e)
	}
	if s.docs[key].owner != a {
		t.Fatal("ownership committed before current parse loaded")
	}
	if newer, e := s.Ack(b, tok, fresh.ID); e != nil || newer != nil {
		t.Fatal("fresh acknowledgement failed", e)
	}
	if len(s.docs) != 1 {
		t.Fatal("transfer duplicated store entry")
	}
}
func TestRollbackTimeoutAndWindowCrash(t *testing.T) {
	for _, kind := range []string{"cancel", "timeout", "destination-close", "destination-crash"} {
		t.Run(kind, func(t *testing.T) {
			s, a, b, p := fixture(t)
			key := opened(t, s, a, p)
			tok := begin(t, s, a, key)
			_ = s.Accept(b, tok, 0)
			switch kind {
			case "cancel":
				_ = s.Cancel(b, tok)
			case "timeout":
				s.transfers[tok].expires = time.Now().Add(-time.Second)
				s.Sweep(time.Now())
			case "destination-close":
				s.Unregister(b)
			case "destination-crash":
				s.windows[b].last = time.Now().Add(-windowTTL - time.Second)
				s.Sweep(time.Now())
			}
			if _, e := s.Info(a, key); e != nil {
				t.Fatal("source not restored", e)
			}
			if _, e := s.Info(b, key); e == nil {
				t.Fatal("destination retained incoming read access")
			}
			if len(s.docs) != 1 || len(s.transfers) != 0 {
				t.Fatal("rollback leaked state")
			}
			q := drain(s, a)
			if len(q) != 1 || q[0].Name != "tab:aborted" {
				t.Fatalf("missing source rollback %#v", q)
			}
		})
	}
}
func TestSourceCloseCancelsIncoming(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	tok := begin(t, s, a, key)
	_ = s.Accept(b, tok, 0)
	s.Unregister(a)
	if len(s.docs) != 0 || len(s.transfers) != 0 {
		t.Fatal("closed source leaked document")
	}
	if _, e := s.Ack(b, tok, key); e == nil {
		t.Fatal("closed document committed")
	}
	q := drain(s, b)
	if len(q) != 2 || q[1].Name != "tab:aborted" {
		t.Fatal("destination not notified", q)
	}
}
func TestExpiredForeignAndReplayedTokens(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	tok := begin(t, s, a, key)
	if e := s.Accept(a, tok, 0); e == nil {
		t.Fatal("same-window transfer accepted")
	}
	if e := s.Accept(b, "../../file.md", 0); e == nil {
		t.Fatal("path used as token")
	}
	if e := s.Cancel(b, tok); e == nil {
		t.Fatal("foreign cancellation accepted")
	}
	s.transfers[tok].expires = time.Now().Add(-time.Second)
	if e := s.Accept(b, tok, 0); e == nil {
		t.Fatal("expired token accepted")
	}
	if _, e := s.Begin(a, key, TabState{}); e == nil {
		t.Fatal("second pending transfer accepted")
	}
	_ = s.Cancel(a, tok)
	newtok := begin(t, s, a, key)
	if newtok == tok {
		t.Fatal("token reused")
	}
	if e := s.Accept(b, tok, 0); e == nil {
		t.Fatal("cancelled token replayed")
	}
}
func TestDropInsideViewerCancelsAndAcceptedDropSurvivesEnd(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	tok := begin(t, s, a, key)
	if e := s.End(a, tok, false, 200, Position{100, 200}); e != nil {
		t.Fatal(e)
	}
	if len(s.transfers) != 0 {
		t.Fatal("drop over viewer detached")
	}
	tok = begin(t, s, a, key)
	_ = s.Accept(b, tok, 0)
	if e := s.End(a, tok, false, 0, Position{100, 200}); e != nil {
		t.Fatal(e)
	}
	if s.transfers[tok].destination != b {
		t.Fatal("dragend undid accepted drop")
	}
}
func TestDetachPlacementAndSpawnFailure(t *testing.T) {
	s, a, _, p := fixture(t)
	key := opened(t, s, a, p)
	tok := begin(t, s, a, key)
	var boot Bootstrap
	s.spawn = func(b Bootstrap) error { boot = b; return nil }
	if e := s.Detach(a, tok, Position{-800, 120}); e != nil {
		t.Fatal(e)
	}
	if boot.Position.X != -800 || boot.Credential == "" || boot.Web == "" {
		t.Fatalf("invalid bootstrap %#v", boot)
	}
	if s.transfers[tok].destination == "" {
		t.Fatal("destination not reserved")
	}
	_ = s.Cancel(a, tok)
	tok = begin(t, s, a, key)
	s.spawn = func(Bootstrap) error { return errors.New("cannot launch") }
	if e := s.Detach(a, tok, Position{}); e == nil {
		t.Fatal("spawn failure hidden")
	}
	if len(s.transfers) != 0 || s.docs[key].owner != a {
		t.Fatal("spawn failure lost source")
	}
}
func TestTabOrderAndInsert(t *testing.T) {
	s, a, b, p := fixture(t)
	one := opened(t, s, a, p)
	q := filepath.Join(filepath.Dir(p), "second.md")
	_ = os.WriteFile(q, []byte("# Second"), 0600)
	two := opened(t, s, a, q)
	if e := s.Order(a, []int{two, one}); e != nil {
		t.Fatal(e)
	}
	for _, bad := range [][]int{{one}, {one, one}, {one, 9999}} {
		if e := s.Order(a, bad); e == nil {
			t.Fatal("invalid order accepted", bad)
		}
	}
	tok := begin(t, s, a, two)
	_ = s.Accept(b, tok, 99)
	_, e := s.Ack(b, tok, two)
	if e != nil {
		t.Fatal(e)
	}
	if len(s.windows[a].order) != 1 || s.windows[a].order[0] != one || s.windows[b].order[0] != two {
		t.Fatal("incorrect committed order")
	}
}
func TestSettingsSerialisationAndFailure(t *testing.T) {
	s, _, _, _ := fixture(t)
	s.save = func(next settings.Settings) error { return nil }
	var wg sync.WaitGroup
	for i := 0; i < 24; i++ {
		wg.Add(1)
		go func(i int) {
			defer wg.Done()
			next := settings.Defaults()
			next.FontSize = 12 + i%17
			if e := s.SaveSettings(next); e != nil {
				t.Error(e)
			}
		}(i)
	}
	wg.Wait()
	final := s.Settings()
	for id := range s.windows {
		q := drain(s, id)
		if len(q) != 24 {
			t.Fatal("missing settings events", len(q))
		}
		if q[len(q)-1].Data.(settings.Settings) != final {
			t.Fatal("windows disagree with persistence order")
		}
	}
	s.save = func(settings.Settings) error { return errors.New("disk failure") }
	if s.SaveSettings(settings.Defaults()) == nil || s.Settings() != final {
		t.Fatal("failed save changed settings")
	}
}
func TestLiveReloadOnlyOwningWindow(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	drain(s, a)
	drain(s, b)
	_ = os.WriteFile(p, []byte("# Live change"), 0600)
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) {
		s.mu.Lock()
		fresh := s.docs[key].info.ID != key
		s.mu.Unlock()
		if fresh {
			q := drain(s, a)
			if len(q) == 0 || q[0].Name != "doc:changed" {
				t.Fatal("no live event")
			}
			if len(drain(s, b)) != 0 {
				t.Fatal("foreign window saw live document")
			}
			return
		}
		time.Sleep(10 * time.Millisecond)
	}
	t.Fatal("watcher did not reload")
}
func TestRoutingAndFinalShutdown(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	_ = key
	_ = s.Focus(b)
	if e := s.Launch([]string{p, filepath.Join(filepath.Dir(p), "new.md")}); e != nil {
		t.Fatal(e)
	}
	if q := drain(s, a); len(q) != 1 || q[0].Name != "open:paths" {
		t.Fatal("existing file did not route to owner", q)
	}
	if q := drain(s, b); len(q) != 1 {
		t.Fatal("new file did not route to focused window", q)
	}
	s.Unregister(a)
	if s.Sweep(time.Now().Add(3 * time.Second)) {
		t.Fatal("coordinator stopped while a window lived")
	}
	s.Unregister(b)
	if !s.Sweep(time.Now().Add(3 * time.Second)) {
		t.Fatal("empty coordinator did not stop")
	}
	if s.Launch(nil) == nil {
		t.Fatal("stopping coordinator accepted launcher")
	}
}
func TestOSLockAndPrivateRecordPermissions(t *testing.T) {
	p := filepath.Join(t.TempDir(), "lock")
	first, e := lockFile(p)
	if e != nil {
		t.Fatal(e)
	}
	if second, e := lockFile(p); e == nil {
		second.Close()
		t.Fatal("two coordinators acquired lock")
	}
	first.Close()
	third, e := lockFile(p)
	if e != nil {
		t.Fatal("lock not released", e)
	}
	third.Close()
}
func TestTransportAuthenticationBoundsAndOwnership(t *testing.T) {
	s, a, b, p := fixture(t)
	listener, e := net.Listen("tcp4", "127.0.0.1:0")
	if e != nil {
		t.Fatal(e)
	}
	credential := token()
	server := &http.Server{Handler: s.Handler(listener.Addr().String(), credential)}
	go server.Serve(listener)
	t.Cleanup(func() { server.Close() })
	client, _ := NewClient(Bootstrap{Address: s.address, Credential: s.windows[a].credential, Web: token()})
	other, _ := NewClient(Bootstrap{Address: s.address, Credential: s.windows[b].credential, Web: token()})
	r, e := client.Open(p)
	if e != nil {
		t.Fatal(e)
	}
	if r.Info.Base == "" || r.Info.Web != client.WebPath() {
		t.Fatal("window asset URLs missing")
	}
	if _, e := other.Chunk(r.Info.ID, 0); e == nil {
		t.Fatal("foreign chunks readable")
	}
	if _, e := client.Search(r.Info.ID, string(make([]byte, 4097))); e == nil {
		t.Fatal("unbounded search accepted")
	}
	bad, _ := NewClient(Bootstrap{Address: s.address, Credential: token()})
	if bad.Register() == nil {
		t.Fatal("foreign credential accepted")
	}
	if client.Call(request{Op: "launch"}, nil) == nil {
		t.Fatal("window used launcher operation")
	}
	launcher, _ := NewClient(Bootstrap{Address: s.address, Credential: credential})
	if launcher.Register() == nil {
		t.Fatal("launcher read window data")
	}
	req, _ := http.NewRequest("POST", "http://"+s.address+"/rpc", nil)
	req.Host = "foreign.test"
	req.Header.Set("Authorization", "Bearer "+credential)
	res, e := http.DefaultClient.Do(req)
	if e != nil {
		t.Fatal(e)
	}
	res.Body.Close()
	if res.StatusCode != 403 {
		t.Fatal("host check missing")
	}
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if _, e := client.Poll(ctx); e == nil {
		t.Fatal("poll ignored cancellation")
	}
	data, _ := json.Marshal(request{Op: "open", Text: string(make([]byte, maxRequest))})
	_ = data
	if client.Call(request{Op: "open", Text: string(make([]byte, maxRequest))}, nil) == nil {
		t.Fatal("unbounded request accepted")
	}
}

func TestDragEndBeforeDestinationBinding(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	tok := begin(t, s, a, key)
	done := make(chan error, 1)
	go func() { done <- s.End(a, tok, false, 200, Position{}) }()
	time.Sleep(20 * time.Millisecond)
	if e := s.Accept(b, tok, 0); e != nil {
		t.Fatal("late destination binding rejected", e)
	}
	if e := <-done; e != nil {
		t.Fatal(e)
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.transfers[tok] == nil || s.transfers[tok].destination != b {
		t.Fatal("drag end cancelled accepted transfer")
	}
}

func TestPendingLaunchSurvivesLastTabMove(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	_ = s.Focus(a)
	pending := filepath.Join(filepath.Dir(p), "pending.md")
	_ = os.WriteFile(pending, []byte("# Pending"), 0600)
	if e := s.Launch([]string{pending}); e != nil {
		t.Fatal(e)
	}
	tok := begin(t, s, a, key)
	_ = s.Accept(b, tok, 0)
	_, e := s.Ack(b, tok, key)
	if e != nil {
		t.Fatal(e)
	}
	drain(s, b)
	s.Unregister(a)
	q := drain(s, b)
	if len(q) != 1 || q[0].Name != "open:paths" || q[0].Data.([]string)[0] != pending {
		t.Fatal("pending launch was lost", q)
	}
	opened(t, s, b, pending)
	if len(s.windows[b].pendingPaths) != 0 || len(s.docs) != 2 {
		t.Fatal("pending launch did not finish")
	}
}

func TestLocalImageAccessFollowsCommittedOwnership(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	image := filepath.Join(filepath.Dir(p), "image.png")
	_ = os.WriteFile(image, []byte("image fixture"), 0600)
	req := request{Op: "local", Key: key, Text: image}
	if _, e := s.dispatch(context.Background(), b, req); e == nil {
		t.Fatal("foreign local image readable")
	}
	tok := begin(t, s, a, key)
	_ = s.Accept(b, tok, 0)
	if _, e := s.dispatch(context.Background(), b, req); e != nil {
		t.Fatal("incoming image inaccessible", e)
	}
	_, e := s.Ack(b, tok, key)
	if e != nil {
		t.Fatal(e)
	}
	if _, e := s.dispatch(context.Background(), a, req); e == nil {
		t.Fatal("former owner read transferred image")
	}
	if _, e := s.dispatch(context.Background(), b, req); e != nil {
		t.Fatal(e)
	}
	req.Text = p
	if _, e := s.dispatch(context.Background(), b, req); e == nil {
		t.Fatal("non-image local file returned")
	}
	large := filepath.Join(filepath.Dir(p), "large.png")
	f, e := os.Create(large)
	if e != nil {
		t.Fatal(e)
	}
	_ = f.Truncate((25 << 20) + 1)
	f.Close()
	req.Text = large
	if _, e := s.dispatch(context.Background(), b, req); e == nil {
		t.Fatal("unbounded local image returned")
	}
}
func TestEventOverflowDisconnectsWithoutMutatingTransaction(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	tok := begin(t, s, a, key)
	_ = s.Accept(b, tok, 0)
	s.mu.Lock()
	for i := 0; i < 257; i++ {
		s.emit(a, "window:focus", nil)
	}
	s.mu.Unlock()
	if !s.windows[a].failed {
		t.Fatal("unbounded queue")
	}
	// The overflow is handled after the ownership transaction, not recursively
	// from inside emit. The accepted destination remains a valid owner.
	if _, e := s.Ack(b, tok, key); e != nil {
		t.Fatal(e)
	}
	s.Sweep(time.Now())
	if _, e := s.Info(b, key); e != nil {
		t.Fatal("overflow destroyed committed destination", e)
	}
}

func TestNativeHoverRoutingRequiresSourceAndStopsAfterAccept(t *testing.T) {
	s, a, b, p := fixture(t)
	key := opened(t, s, a, p)
	tok := begin(t, s, a, key)
	point := Position{X: 120, Y: 30}
	if err := s.Hover(b, tok, 200, point); err == nil {
		t.Fatal("another window routed the source's drag")
	}
	if len(drain(s, b)) != 0 {
		t.Fatal("foreign drag reached destination")
	}
	if err := s.Hover(a, tok, 200, point); err != nil {
		t.Fatal(err)
	}
	q := drain(s, b)
	if len(q) != 1 || q[0].Name != "tab:hover" || q[0].Data.(DragPoint) != (DragPoint{Token: tok, Position: point}) {
		t.Fatal("native hover did not reach the hit window", q)
	}
	if err := s.Hover(a, tok, 0, point); err != nil {
		t.Fatal(err)
	}
	q = drain(s, b)
	if len(q) != 1 || q[0].Name != "tab:leave" {
		t.Fatal("leaving a viewer retained its insertion marker", q)
	}
	if err := s.Hover(a, tok, 200, point); err != nil {
		t.Fatal(err)
	}
	drain(s, b)
	if err := s.Accept(b, tok, 0); err != nil {
		t.Fatal(err)
	}
	q = drain(s, b)
	if len(q) != 2 || q[0].Name != "tab:leave" || q[1].Name != "tab:offer" {
		t.Fatal("accepted drop did not clear its marker and offer content", q)
	}
	if err := s.Hover(a, tok, 200, point); err != nil {
		t.Fatal(err)
	}
	if len(drain(s, b)) != 0 {
		t.Fatal("native hover continued after destination accepted the move")
	}
	if _, err := s.Info(a, key); err != nil {
		t.Fatal("hover or acceptance committed ownership before acknowledgement", err)
	}
}
