//go:build windows && !bindings && (amd64 || arm64)

package native

import (
	"runtime"
	"strings"
	"testing"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

type testFeedbackData struct {
	VTable *feedbackDataVTable
	Text   []uint16
	Bytes  uintptr
}

var testFeedbackGetData = windows.NewCallback(func(data *testFeedbackData, format *feedbackFormat, medium *feedbackMedium) uintptr {
	if format.Format != 13 || format.Aspect != 1 || format.Index != -1 || format.Medium != 1 {
		return 0x80040064 // DV_E_FORMATETC
	}
	n := data.Bytes
	if n == 0 {
		n = uintptr(len(data.Text) * 2)
	}
	h, _, _ := kernel32.NewProc("GlobalAlloc").Call(0x42, n) // GMEM_MOVEABLE | GMEM_ZEROINIT
	if h == 0 {
		return 0x8007000e
	}
	ptr, _, _ := kernel32.NewProc("GlobalLock").Call(h)
	if ptr == 0 {
		kernel32.NewProc("GlobalFree").Call(h)
		return 0x8007000e
	}
	if len(data.Text) != 0 {
		var written uintptr
		if err := windows.WriteProcessMemory(windows.CurrentProcess(), ptr, (*byte)(unsafe.Pointer(&data.Text[0])), min(n, uintptr(len(data.Text)*2)), &written); err != nil {
			kernel32.NewProc("GlobalUnlock").Call(h)
			kernel32.NewProc("GlobalFree").Call(h)
			return 0x80004005
		}
	}
	kernel32.NewProc("GlobalUnlock").Call(h)
	*medium = feedbackMedium{Type: 1, Global: h}
	return 0
})

func TestFeedbackNativeCOMLayouts(t *testing.T) {
	if unsafe.Sizeof(feedbackWindowClass{}) != 80 || unsafe.Sizeof(feedbackFormat{}) != 32 || unsafe.Sizeof(feedbackMedium{}) != 24 || unsafe.Sizeof(feedbackDropTargetVTable{}) != 56 {
		t.Fatal("native structures do not match the 64-bit Windows ABI")
	}
	target := newFeedbackTarget(strings.Repeat("ab", 32))
	var out *feedbackDropTarget
	if feedbackQueryInterface(target, &feedbackIDropTarget, &out) != 0 || out != target || target.Refs.Load() != 2 {
		t.Fatal("IDropTarget query/reference failure")
	}
	if feedbackRelease(target) != 1 {
		t.Fatal("IDropTarget release failure")
	}
	iid := windows.GUID{Data1: 0x1234}
	if feedbackQueryInterface(target, &iid, &out) != 0x80004002 || out != nil {
		t.Fatal("unsupported COM interface accepted")
	}
	if feedbackRelease(target) != 0 {
		t.Fatal("last native reference survived release")
	}
	feedbackTargetsMu.Lock()
	_, retained := feedbackTargets[target]
	feedbackTargetsMu.Unlock()
	if retained {
		t.Fatal("COM target remained a GC root after its final release")
	}
}

func TestFeedbackNativeClipboardReadsAreBounded(t *testing.T) {
	token := strings.Repeat("ab", 32)
	target := &feedbackDropTarget{Token: token}
	for _, tt := range []struct {
		name, text string
		bytes      uintptr
		want       bool
	}{
		{"current token", tabTextPrefix + token, 0, true},
		{"foreign token", tabTextPrefix + strings.Repeat("cd", 32), 0, false},
		{"ordinary text", "document.md", 0, false},
		{"over limit", tabTextPrefix + token, 2048, false},
		{"too short", "", 2, false},
	} {
		t.Run(tt.name, func(t *testing.T) {
			text, err := windows.UTF16FromString(tt.text)
			if err != nil {
				t.Fatal(err)
			}
			data := &testFeedbackData{VTable: &feedbackDataVTable{GetData: testFeedbackGetData}, Text: text, Bytes: tt.bytes}
			got := feedbackDataMatches(target, (*feedbackDataObject)(unsafe.Pointer(data)))
			runtime.KeepAlive(data)
			if got != tt.want {
				t.Fatalf("native clipboard match = %v; want %v", got, tt.want)
			}
		})
	}
}

func TestFeedbackNativeMoveRequiresLiveSessionAndMovePermission(t *testing.T) {
	token := strings.Repeat("ab", 32)
	target := &feedbackDropTarget{Token: token, Deadline: time.Now().Add(time.Minute), Matching: true}
	mu.Lock()
	feedbackToken, feedbackPIDs, result = token, []int{int(windows.GetCurrentProcessId())}, Result{}
	mu.Unlock()
	t.Cleanup(func() { mu.Lock(); feedbackToken, feedbackPIDs, result = "", nil, Result{}; mu.Unlock() })
	check := func(want uint32) {
		t.Helper()
		effect := uint32(3)
		feedbackEffect(target, 0, &effect)
		if effect != want {
			t.Fatalf("effect = %d; want %d", effect, want)
		}
	}
	check(dropMove)
	effect := uint32(1) // A copy-only source never becomes a move.
	feedbackEffect(target, 0, &effect)
	if effect != 0 {
		t.Fatal("copy-only drag accepted")
	}
	target.Matching = false
	check(0)
	target.Matching = true
	target.Deadline = time.Now().Add(-time.Second)
	check(0)
	target.Deadline = time.Now().Add(time.Minute)
	mu.Lock()
	result.Cancelled = true
	mu.Unlock()
	check(0)
}

func TestFeedbackViewerHitTestingIncludesNegativeDisplayCoordinates(t *testing.T) {
	viewers := []feedbackViewer{{rect{-1400, -100, -200, 800}, 100}, {rect{100, 50, 1100, 850}, 200}}
	for _, tt := range []struct {
		point point
		want  int
	}{
		{point{-1400, -100}, 100}, {point{-201, 799}, 100}, {point{-200, 799}, 0},
		{point{100, 50}, 200}, {point{500, 400}, 200}, {point{1100, 400}, 0}, {point{0, 0}, 0},
	} {
		if got := feedbackViewerAt(viewers, tt.point); got != tt.want {
			t.Fatalf("hit %v = %d; want %d", tt.point, got, tt.want)
		}
	}
}
