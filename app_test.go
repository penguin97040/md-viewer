package main

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/penguin97040/md-viewer/internal/session"
)

func TestShutdownIsBoundedAndIdempotentWithUnresponsiveCoordinator(t *testing.T) {
	var requests atomic.Int32
	release := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests.Add(1)
		select {
		case <-r.Context().Done():
		case <-release:
		}
	}))
	defer server.Close()
	defer close(release)
	client, err := session.NewClient(session.Bootstrap{Address: strings.TrimPrefix(server.URL, "http://"), Credential: strings.Repeat("a", 64)})
	if err != nil {
		t.Fatal(err)
	}
	a := NewApp(client, nil)
	ctx, cancel := context.WithCancel(context.Background())
	a.cancel = cancel
	start := time.Now()
	var wg sync.WaitGroup
	for range 4 {
		wg.Go(func() { a.shutdown(context.Background()) })
	}
	wg.Wait()
	if time.Since(start) > 3*time.Second {
		t.Fatal("shutdown waited beyond its timeout")
	}
	if requests.Load() != 1 {
		t.Fatal("duplicate unregister requests", requests.Load())
	}
	if ctx.Err() == nil || !a.closing {
		t.Fatal("polling or closing state survived shutdown")
	}
	if _, err := a.Initial(); err == nil {
		t.Fatal("closed window registered again")
	}
}

func TestVisibilityWaitRequiresVisibleWindowAndHonoursCancellation(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 50*time.Millisecond)
	defer cancel()
	if err := waitVisible(ctx, func() bool { return false }); err == nil {
		t.Fatal("hidden window accepted")
	}
	if err := waitVisible(context.Background(), func() bool { return true }); err != nil {
		t.Fatal(err)
	}
}
