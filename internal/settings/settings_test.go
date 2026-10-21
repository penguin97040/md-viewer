package settings

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sync"
	"testing"
)

func TestPadding(t *testing.T) {
	for _, tc := range []struct {
		name string
		json string
		want int
	}{
		{"missing", `{}`, 20},
		{"legacy width", `{"width":"narrow","fontSize":21}`, 20},
		{"zero", `{"padding":0}`, 20},
		{"below minimum", `{"padding":19}`, 20},
		{"minimum", `{"padding":20}`, 20},
		{"saved", `{"padding":28}`, 28},
		{"negative", `{"padding":-4}`, 20},
		{"maximum", `{"padding":40}`, 40},
		{"too large", `{"padding":100}`, 40},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s := Defaults()
			if err := json.Unmarshal([]byte(tc.json), &s); err != nil {
				t.Fatal(err)
			}
			s = s.Clean()
			if s.Padding != tc.want {
				t.Fatalf("padding = %d, want %d", s.Padding, tc.want)
			}
			p := filepath.Join(t.TempDir(), "settings.json")
			if err := saveFile(p, s); err != nil {
				t.Fatal(err)
			}
			b, err := os.ReadFile(p)
			if err != nil {
				t.Fatal(err)
			}
			var saved map[string]any
			if err := json.Unmarshal(b, &saved); err != nil {
				t.Fatal(err)
			}
			if saved["padding"] != float64(tc.want) {
				t.Fatalf("saved padding = %v, want %d", saved["padding"], tc.want)
			}
			if _, ok := saved["width"]; ok {
				t.Fatal("saved obsolete width setting")
			}
			if tc.name == "legacy width" && saved["fontSize"] != float64(21) {
				t.Fatal("lost unrelated legacy preference")
			}
		})
	}
}

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
