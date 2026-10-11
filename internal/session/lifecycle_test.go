package session

import (
	"os"
	"os/exec"
	"strings"
	"testing"
	"time"
)

func TestAcknowledgementRequiresNativeWindowReadiness(t *testing.T) {
	s, source, dest, path := fixture(t)
	key := opened(t, s, source, path)
	tok := begin(t, s, source, key)
	s.windows[dest].visible = false
	if err := s.Accept(dest, tok, 0); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Ack(dest, tok, key); err == nil {
		t.Fatal("hidden destination committed ownership")
	}
	if s.docs[key].owner != source || len(s.windows[source].order) != 1 {
		t.Fatal("source was removed before visibility")
	}
	if err := s.WindowReady(dest); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Ack(dest, tok, key); err != nil {
		t.Fatal(err)
	}
	if s.docs[key].owner != dest {
		t.Fatal("ready destination did not commit")
	}
}

func TestFailedDetachedWindowClosesWithoutLosingSource(t *testing.T) {
	s, source, _, path := fixture(t)
	key := opened(t, s, source, path)
	tok := begin(t, s, source, key)
	if err := s.Detach(source, tok, Position{X: 250, Y: 200}); err != nil {
		t.Fatal(err)
	}
	dest := s.transfers[tok].destination
	if err := s.WindowReady(dest); err == nil {
		t.Fatal("unregistered child reported readiness")
	}
	if err := s.Register(dest, 300); err != nil {
		t.Fatal(err)
	}
	if _, err := s.Ack(dest, tok, key); err == nil {
		t.Fatal("hidden child accepted document")
	}
	drain(s, dest)
	if err := s.Cancel(dest, tok); err != nil {
		t.Fatal(err)
	}
	q := drain(s, dest)
	if len(q) != 2 || q[0].Name != "tab:aborted" || q[1].Name != "window:close" {
		t.Fatal("failed provisional window was left running", q)
	}
	if s.docs[key].owner != source {
		t.Fatal("failed detach lost source tab")
	}
}

func TestFailedMoveDoesNotCloseAnExistingEmptyWindow(t *testing.T) {
	s, source, dest, path := fixture(t)
	key := opened(t, s, source, path)
	tok := begin(t, s, source, key)
	if err := s.Accept(dest, tok, 0); err != nil {
		t.Fatal(err)
	}
	drain(s, dest)
	if err := s.Cancel(dest, tok); err != nil {
		t.Fatal(err)
	}
	for _, e := range drain(s, dest) {
		if e.Name == "window:close" {
			t.Fatal("existing empty viewer was closed")
		}
	}
}

func TestChildExitBeforeRegistrationRollsBackImmediately(t *testing.T) {
	s, source, _, path := fixture(t)
	key := opened(t, s, source, path)
	tok := begin(t, s, source, key)
	if err := s.Detach(source, tok, Position{}); err != nil {
		t.Fatal(err)
	}
	dest := s.transfers[tok].destination
	credential := s.windows[dest].credential
	s.ProcessExited(credential)
	s.ProcessExited(credential)
	if s.windows[dest] != nil || s.transfers[tok] != nil || s.docs[key].owner != source {
		t.Fatal("exited child retained reservation or lost source")
	}
}

func TestChildWaiterRemovesWindowAndAllowsFinalShutdown(t *testing.T) {
	if os.Getenv("MD_VIEWER_EXIT_HELPER") == "1" {
		return
	}
	s, source, dest, path := fixture(t)
	opened(t, s, source, path)
	credential := s.windows[source].credential
	s.Unregister(dest)
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command(exe, "-test.run=^TestChildWaiterRemovesWindowAndAllowsFinalShutdown$")
	cmd.Env = append(os.Environ(), "MD_VIEWER_EXIT_HELPER=1")
	done := make(chan struct{})
	if err := startChild(cmd, func() { s.ProcessExited(credential); close(done) }); err != nil {
		t.Fatal(err)
	}
	select {
	case <-done:
	case <-time.After(5 * time.Second):
		t.Fatal("child exit was not observed")
	}
	if !s.Sweep(time.Now().Add(3*time.Second)) || len(s.docs) != 0 {
		t.Fatal("child exit left coordinator or documents alive")
	}
}

func TestBootstrapCredentialsStayOutOfCommandLine(t *testing.T) {
	b := Bootstrap{Address: "127.0.0.1:1234", Credential: token(), Web: token()}
	cmd, err := childCommand("viewer", "--window", &b)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(strings.Join(cmd.Args, " "), b.Credential) || cmd.Stdin == nil {
		t.Fatal("bootstrap exposed in command line or missing")
	}
}

func TestUnregisterReturnsWhenCoordinatorStopsResponding(t *testing.T) {
	c, err := NewClient(Bootstrap{Address: "127.0.0.1:1", Credential: token()})
	if err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	if err := c.Unregister(); err == nil {
		t.Fatal("unavailable coordinator accepted unregister")
	}
	if time.Since(start) > 3*time.Second {
		t.Fatal("unregister was not bounded")
	}
}
