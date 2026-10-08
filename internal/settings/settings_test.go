package settings

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"testing"
)

func TestConcurrentSave(t *testing.T) {
	p := filepath.Join(t.TempDir(), "md-viewer", "settings.json")
	var wg sync.WaitGroup
	for i := 0; i < 32; i++ {
		wg.Go(func() {
			s := Defaults()
			s.FontSize = 12 + i%17
			if err := saveFile(p, s); err != nil {
				t.Error(err)
			}
		})
	}
	wg.Wait()
	b, err := os.ReadFile(p)
	var s Settings
	if err != nil || json.Unmarshal(b, &s) != nil || s != s.Clean() {
		t.Fatalf("invalid saved settings: %s, %v", b, err)
	}
	files, _ := filepath.Glob(filepath.Join(filepath.Dir(p), "*.tmp"))
	if len(files) != 0 {
		t.Fatalf("temporary files left behind: %v", files)
	}
}

func TestSaveFailureCleansTemporaryFile(t *testing.T) {
	p := filepath.Join(t.TempDir(), "md-viewer", "settings.json")
	if err := os.MkdirAll(p, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := saveFile(p, Defaults()); err == nil {
		t.Fatal("replaced a directory")
	}
	files, _ := filepath.Glob(filepath.Join(filepath.Dir(p), "*.tmp"))
	if len(files) != 0 {
		t.Fatalf("temporary files left behind: %v", files)
	}
}
