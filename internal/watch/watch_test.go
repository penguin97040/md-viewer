package watch

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestWatchWriteAndRename(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "a.md")
	if err := os.WriteFile(p, []byte("one"), 0o644); err != nil {
		t.Fatal(err)
	}
	hits := make(chan struct{}, 10)
	w, err := New(p, func() { hits <- struct{}{} })
	if err != nil {
		t.Fatal(err)
	}
	defer w.Close()

	expect := func(what string) {
		t.Helper()
		select {
		case <-hits:
		case <-time.After(3 * time.Second):
			t.Fatalf("no change event after %s", what)
		}
	}

	os.WriteFile(p, []byte("two"), 0o644)
	expect("write")

	// Save by rename, as many editors do.
	tmp := filepath.Join(dir, "a.md.tmp")
	os.WriteFile(tmp, []byte("three"), 0o644)
	os.Rename(tmp, p)
	expect("rename")

	// Other files in the folder are ignored.
	os.WriteFile(filepath.Join(dir, "b.md"), []byte("x"), 0o644)
	select {
	case <-hits:
		t.Fatal("event for unrelated file")
	case <-time.After(500 * time.Millisecond):
	}
}
