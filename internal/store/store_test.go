package store

import (
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/penguin97040/md-viewer/internal/doc"
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

	s := New(false, nil)
	ia, err := s.Open(a)
	if err != nil {
		t.Fatal(err)
	}
	ib, err := s.Open(b)
	if err != nil {
		t.Fatal(err)
	}
	if ia.ID == ib.ID {
		t.Fatal("ids should differ")
	}
	if _, err := s.Get(ia.ID); err != nil {
		t.Fatal(err)
	}

	// Links resolve against each document's own folder.
	p, err := s.Resolve(ib.Key, "../a.md#a")
	if err != nil || p != a {
		t.Errorf("resolve = %q, %v; want %q", p, err, a)
	}
	p, _ = s.Resolve(ia.Key, "sub/b%20c.md")
	if p != filepath.Join(dir, "sub", "b c.md") {
		t.Errorf("unescaped resolve = %q", p)
	}

	// Reload gives a new id and retires the old one.
	write(t, a, "# A\n\n## More\n")
	ra, err := s.Reload(ia.Key)
	if err != nil {
		t.Fatal(err)
	}
	if ra.ID == ia.ID || ra.Key != ia.Key || len(ra.Headings) != 2 {
		t.Errorf("reload: id %d -> %d, %d headings", ia.ID, ra.ID, len(ra.Headings))
	}
	if _, err := s.Get(ia.ID); err == nil {
		t.Error("old id still valid after reload")
	}

	s.Close(ib.Key)
	if _, err := s.Get(ib.ID); err == nil {
		t.Error("closed document still open")
	}
	if _, err := s.Resolve(ib.Key, "x.md"); err == nil {
		t.Error("resolve on closed document should fail")
	}
	if _, err := s.Open(dir); err == nil {
		t.Error("opening a folder should fail")
	}
}

func TestStoreLiveReload(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "a.md")
	write(t, p, "# One\n")
	changed := make(chan *Info, 4)
	s := New(true, func(info *Info) { changed <- info })
	defer s.CloseAll()
	info, err := s.Open(p)
	if err != nil {
		t.Fatal(err)
	}
	write(t, p, "# Two\n")
	select {
	case c := <-changed:
		if c.Key != info.Key || c.ID == info.ID {
			t.Errorf("changed key %d id %d, opened key %d id %d", c.Key, c.ID, info.Key, info.ID)
		}
		if _, err := s.Get(c.ID); err != nil {
			t.Error(err)
		}
		// A manual reload by key still works after the live one, and
		// closing by key stops the watcher whatever the current id is.
		r, err := s.Reload(info.Key)
		if err != nil || r.ID <= c.ID {
			t.Errorf("reload after live reload: %v, id %d", err, r.ID)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("no live reload")
	}

	s.SetLive(false)
	write(t, p, "# Three\n")
	select {
	case <-changed:
		t.Error("reloaded with live reload off")
	case <-time.After(500 * time.Millisecond):
	}
}

// A tab that closes while a live reload is in flight must not leave the new
// parse (and its watcher) behind.
func TestStoreCloseDuringLiveReload(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "a.md")
	write(t, p, "# One\n")
	s := New(true, nil)
	defer s.CloseAll()
	info, err := s.Open(p)
	if err != nil {
		t.Fatal(err)
	}
	e := s.byKey[info.Key]
	s.fileChanged(e, e.watchGen) // the id moves on before the frontend closes the tab
	s.Close(info.Key)
	if len(s.byKey) != 0 || len(s.byID) != 0 || e.watcher != nil {
		t.Errorf("left behind: %d keys, %d ids, watcher %v", len(s.byKey), len(s.byID), e.watcher != nil)
	}
	s.fileChanged(e, e.watchGen) // a late event for a closed document does nothing
	if len(s.byID) != 0 {
		t.Error("closed document reopened by a late file event")
	}
}

// A parse that began reading earlier never replaces one that began later,
// e.g. a manual reload overtaken by a live reload of a newer save.
func TestStoreOlderReadLoses(t *testing.T) {
	dir := t.TempDir()
	p := filepath.Join(dir, "a.md")
	write(t, p, "# One\n")
	s := New(true, nil)
	defer s.CloseAll()
	info, err := s.Open(p)
	if err != nil {
		t.Fatal(err)
	}
	e := s.byKey[info.Key]
	early := s.startLoad()
	stale := e.doc
	newer := doc.Parse([]byte("# One\n\n## Two\n"))
	s.load = func(string) (*doc.Doc, error) { return newer, nil }
	s.fileChanged(e, e.watchGen)
	live := e.id

	s.mu.Lock()
	got, ok := s.install(e, stale, early)
	s.mu.Unlock()
	if ok || got.ID != live || len(got.Headings) != 2 {
		t.Errorf("older parse went in: ok %v, id %d (live %d), %d headings", ok, got.ID, live, len(got.Headings))
	}
}

func TestWindowsShare(t *testing.T) {
	for p, want := range map[string]string{
		`C:\docs\a.md`:                   "",
		`/home/me/a.md`:                  "",
		`\\Server\Share\a.png`:           `\\server\share`,
		`//server/share/dir/a.png`:       `\\server\share`,
		`\/server\share\a.png`:           `\\server\share`,
		`\\server`:                       deviceShare,
		`\\server\`:                      deviceShare,
		`\\?\C:\a.png`:                   deviceShare,
		`\\?\UNC\server\share\a.png`:     deviceShare,
		`\\.\pipe\x.png`:                 deviceShare,
		`\\\server\share\a.png`:          deviceShare,
		`\\attacker.example@SSL\s\x.png`: `\\attacker.example@ssl\s`,
	} {
		if got := windowsShare(p); got != want {
			t.Errorf("windowsShare(%q) = %q, want %q", p, got, want)
		}
	}
}

// A document may only reach a network share that an open document is on:
// reading from any other would send the user's credentials to that server.
func TestReachable(t *testing.T) {
	s := New(false, nil)
	s.byKey[1] = &entry{key: 1, path: `\\nas\docs\readme.md`}
	for p, want := range map[string]bool{
		`C:\pics\a.png`:          true,
		`\\nas\docs\img\a.png`:   true,
		`\\NAS\Docs\a.png`:       true,
		`\\nas\other\a.png`:      false,
		`\\attacker\share\a.png`: false,
		`\\?\UNC\nas\docs\a.png`: false,
		`//attacker/share/x.md`:  false,
		`\\.\GLOBALROOT\x\a.png`: false,
	} {
		if got := s.reachable(p); got != want {
			t.Errorf("reachable(%q) = %v, want %v", p, got, want)
		}
	}
}

func TestWebPathIsSecret(t *testing.T) {
	a, b := New(false, nil).WebPath(), New(false, nil).WebPath()
	if a == b || len(a) != len("/web/")+32 {
		t.Errorf("web paths %q and %q", a, b)
	}
}

func TestOpenRefusesNonFiles(t *testing.T) {
	s := New(false, nil)
	if _, err := s.Open(t.TempDir()); err == nil {
		t.Error("opened a folder")
	}
	if _, err := os.Stat("/dev/null"); err == nil {
		if _, err := s.Open("/dev/null"); err == nil {
			t.Error("opened a device")
		}
	}
}

func TestLiveReloadInvalidatedDuringRead(t *testing.T) {
	for _, replace := range []bool{false, true} {
		t.Run(map[bool]string{false: "disabled", true: "replaced"}[replace], func(t *testing.T) {
			p := filepath.Join(t.TempDir(), "a.md")
			write(t, p, "# One\n")
			changed := make(chan *Info, 1)
			s := New(true, func(info *Info) { changed <- info })
			defer s.CloseAll()
			info, err := s.Open(p)
			if err != nil {
				t.Fatal(err)
			}
			e := s.byKey[info.Key]
			gen := e.watchGen
			started, release, done := make(chan struct{}), make(chan struct{}), make(chan struct{})
			newer := doc.Parse([]byte("# Two\n"))
			s.load = func(string) (*doc.Doc, error) {
				close(started)
				<-release
				return newer, nil
			}
			go func() { defer close(done); s.fileChanged(e, gen) }()
			<-started
			s.SetLive(false)
			if replace {
				s.SetLive(true)
			}
			close(release)
			<-done
			if e.id != info.ID || e.doc.Headings[0].Text != "One" {
				t.Fatal("stale watcher installed a parse")
			}
			select {
			case <-changed:
				t.Fatal("stale watcher emitted a change")
			default:
			}
			// A callback that starts only after replacement must also do nothing.
			s.fileChanged(e, gen)
		})
	}
}
