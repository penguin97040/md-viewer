package main

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func write(t *testing.T, p, s string) {
	t.Helper()
	if err := os.WriteFile(p, []byte(s), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestStoreTabs(t *testing.T) {
	dir := t.TempDir()
	a, b := filepath.Join(dir, "a.md"), filepath.Join(dir, "sub", "b.md")
	os.Mkdir(filepath.Join(dir, "sub"), 0o755)
	write(t, a, "# A\n")
	write(t, b, "# B\n")

	s := newStore(false, nil)
	ia, err := s.open(a)
	if err != nil {
		t.Fatal(err)
	}
	ib, err := s.open(b)
	if err != nil {
		t.Fatal(err)
	}
	if ia.ID == ib.ID {
		t.Fatal("ids should differ")
	}
	if _, err := s.get(ia.ID); err != nil {
		t.Fatal(err)
	}

	// Links resolve against each document's own folder.
	p, err := s.resolve(ib.ID, "../a.md#a")
	if err != nil || p != a {
		t.Errorf("resolve = %q, %v; want %q", p, err, a)
	}
	p, _ = s.resolve(ia.ID, "sub/b%20c.md")
	if p != filepath.Join(dir, "sub", "b c.md") {
		t.Errorf("unescaped resolve = %q", p)
	}

	// Reload gives a new id and retires the old one.
	write(t, a, "# A\n\n## More\n")
	ra, err := s.reload(ia.ID)
	if err != nil {
		t.Fatal(err)
	}
	if ra.ID == ia.ID || len(ra.Headings) != 2 {
		t.Errorf("reload: id %d -> %d, %d headings", ia.ID, ra.ID, len(ra.Headings))
	}
	if _, err := s.get(ia.ID); err == nil {
		t.Error("old id still valid after reload")
	}

	s.close(ib.ID)
	if _, err := s.get(ib.ID); err == nil {
		t.Error("closed document still open")
	}
	if _, err := s.resolve(ib.ID, "x.md"); err == nil {
		t.Error("resolve on closed document should fail")
	}
	if _, err := s.open(dir); err == nil {
		t.Error("opening a folder should fail")
	}
}

func TestStoreLiveReload(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "a.md")
	write(t, p, "# One\n")
	changed := make(chan [2]int, 4)
	s := newStore(true, func(old int, info *DocInfo) { changed <- [2]int{old, info.ID} })
	defer s.closeAll()
	info, err := s.open(p)
	if err != nil {
		t.Fatal(err)
	}
	write(t, p, "# Two\n")
	select {
	case c := <-changed:
		if c[0] != info.ID || c[1] == info.ID {
			t.Errorf("change ids %v, opened %d", c, info.ID)
		}
		if _, err := s.get(c[1]); err != nil {
			t.Error(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("no live reload")
	}

	s.setLive(false)
	write(t, p, "# Three\n")
	select {
	case <-changed:
		t.Error("reloaded with live reload off")
	case <-time.After(500 * time.Millisecond):
	}
}
