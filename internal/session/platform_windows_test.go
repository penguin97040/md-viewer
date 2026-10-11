package session

import "testing"

func TestViewerStartupIsVisibleAndCoordinatorIsHidden(t *testing.T) {
	for _, mode := range []string{"--window", "--coordinator"} {
		cmd, err := childCommand("viewer.exe", mode, nil)
		if err != nil {
			t.Fatal(err)
		}
		if cmd.SysProcAttr.HideWindow != (mode == "--coordinator") {
			t.Fatalf("incorrect startup visibility for %s", mode)
		}
	}
}
