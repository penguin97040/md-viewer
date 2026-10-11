package session

import (
	"context"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestDragWindowsRequiresCurrentOwnedTransfer(t *testing.T) {
	s, a, b, path := fixture(t)
	tok := begin(t, s, a, opened(t, s, a, path))
	if _, err := s.DragWindows(b, tok); err == nil {
		t.Fatal("foreign window enumerated peers")
	}
	if _, err := s.DragWindows(a, "unknown"); err == nil {
		t.Fatal("unknown token enumerated peers")
	}
	assertPIDs := func(want ...int) {
		t.Helper()
		got, err := s.DragWindows(a, tok)
		if err != nil || !slices.Equal(got, want) {
			t.Fatalf("peers = %v, %v; want %v", got, err, want)
		}
	}
	assertPIDs(100, 200)
	c, _ := s.newWindow(nil)
	assertPIDs(100, 200) // A pending process is not a viewer yet.
	if err := s.Register(c, 300); err != nil {
		t.Fatal(err)
	}
	assertPIDs(100, 200, 300)
	s.Unregister(c)
	assertPIDs(100, 200)
	s.mu.Lock()
	s.transfers[tok].expires = time.Now().Add(-time.Second)
	s.mu.Unlock()
	if _, err := s.DragWindows(a, tok); err == nil {
		t.Fatal("expired token enumerated peers")
	}
	s.mu.Lock()
	s.transfers[tok].expires = time.Now().Add(time.Minute)
	s.mu.Unlock()
	if err := s.Accept(b, tok, 0); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DragWindows(a, tok); err == nil {
		t.Fatal("accepted transfer retained outside feedback")
	}
}

func TestDragFeedbackRPCDoesNotExposePeersToLauncherOrForeignWindow(t *testing.T) {
	s, a, b, path := fixture(t)
	tok := begin(t, s, a, opened(t, s, a, path))
	launcher := strings.Repeat("f", 64)
	server := httptest.NewServer(nil)
	defer server.Close()
	address := strings.TrimPrefix(server.URL, "http://")
	server.Config.Handler = s.Handler(address, launcher)
	client := func(credential string) *Client {
		t.Helper()
		c, err := NewClient(Bootstrap{Address: address, Credential: credential})
		if err != nil {
			t.Fatal(err)
		}
		t.Cleanup(c.http.CloseIdleConnections)
		return c
	}
	owner := client(s.windows[a].credential)
	pids, err := owner.StartFeedback(tok, TabState{})
	if err != nil || !slices.Equal(pids, []int{100, 200}) {
		t.Fatal("start feedback", pids, err)
	}
	for _, credential := range []string{s.windows[b].credential, launcher, strings.Repeat("0", 64)} {
		if _, err := client(credential).DragWindows(context.Background(), tok); err == nil {
			t.Fatal("unauthorised peer disclosure")
		}
	}
	if err := owner.Cancel(tok); err != nil {
		t.Fatal(err)
	}
	if _, err := owner.DragWindows(context.Background(), tok); err == nil {
		t.Fatal("cancelled feedback retained peers")
	}
}
