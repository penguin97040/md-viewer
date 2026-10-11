//go:build windows && !bindings && (amd64 || arm64)

package native

import (
	"runtime"
	"slices"
	"sync"
	"sync/atomic"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

const DragFeedback = true
const feedbackStopMessage = 0x8001
const detachTargetClass = "mdViewerTabDetachTarget"
const dropMove = 2

var ole32 = windows.NewLazySystemDLL("ole32.dll")
var gdi32 = windows.NewLazySystemDLL("gdi32.dll")
var kernel32 = windows.NewLazySystemDLL("kernel32.dll")
var feedbackClassOnce sync.Once
var feedbackClassOK bool
var feedbackTargetsMu sync.Mutex
var feedbackTargets = make(map[*feedbackDropTarget]struct{})

type feedbackWindowClass struct {
	Size, Style       uint32
	Proc              uintptr
	ClassExtra, Extra int32
	Instance, Icon    uintptr
	Cursor, Brush     uintptr
	Menu, Name        *uint16
	SmallIcon         uintptr
}

var feedbackWindowProc = windows.NewCallback(func(hwnd uintptr, message uint32, wparam, lparam uintptr) uintptr {
	if message == 0x21 { // WM_MOUSEACTIVATE: never focus the temporary target.
		return 3 // MA_NOACTIVATE
	}
	r, _, _ := user32.NewProc("DefWindowProcW").Call(hwnd, uintptr(message), wparam, lparam)
	return r
})

func registerFeedbackClass() bool {
	feedbackClassOnce.Do(func() {
		name, _ := windows.UTF16PtrFromString(detachTargetClass)
		var module windows.Handle
		_ = windows.GetModuleHandleEx(0, nil, &module)
		brush, _, _ := gdi32.NewProc("GetStockObject").Call(4) // BLACK_BRUSH
		wc := feedbackWindowClass{Size: uint32(unsafe.Sizeof(feedbackWindowClass{})), Proc: feedbackWindowProc, Instance: uintptr(module), Brush: brush, Name: name}
		atom, _, _ := user32.NewProc("RegisterClassExW").Call(uintptr(unsafe.Pointer(&wc)))
		feedbackClassOK = atom != 0
	})
	return feedbackClassOK
}

func detachTargetWindow(hwnd uintptr) bool {
	var name [64]uint16
	user32.NewProc("GetClassNameW").Call(hwnd, uintptr(unsafe.Pointer(&name[0])), uintptr(len(name)))
	return windows.UTF16ToString(name[:]) == detachTargetClass
}

type feedbackViewer struct {
	Bounds rect
	PID    int
}

type feedbackEnumeration struct {
	PIDs    []int
	Viewers []feedbackViewer
}

var feedbackEnumProc = windows.NewCallback(func(hwnd uintptr, scan *feedbackEnumeration) uintptr {
	var pid uint32
	user32.NewProc("GetWindowThreadProcessId").Call(hwnd, uintptr(unsafe.Pointer(&pid)))
	if !slices.Contains(scan.PIDs, int(pid)) {
		return 1
	}
	var class [64]uint16
	user32.NewProc("GetClassNameW").Call(hwnd, uintptr(unsafe.Pointer(&class[0])), uintptr(len(class)))
	if windows.UTF16ToString(class[:]) != "wailsWindow" {
		return 1
	}
	visible, _, _ := user32.NewProc("IsWindowVisible").Call(hwnd)
	iconic, _, _ := user32.NewProc("IsIconic").Call(hwnd)
	if visible == 0 || iconic != 0 {
		return 1
	}
	var bounds rect
	ok, _, _ := user32.NewProc("GetWindowRect").Call(hwnd, uintptr(unsafe.Pointer(&bounds)))
	if ok != 0 && bounds.Right > bounds.Left && bounds.Bottom > bounds.Top {
		scan.Viewers = append(scan.Viewers, feedbackViewer{bounds, int(pid)})
	}
	return 1
})

func feedbackViewers(pids []int) []feedbackViewer {
	scan := feedbackEnumeration{PIDs: pids}
	user32.NewProc("EnumWindows").Call(feedbackEnumProc, uintptr(unsafe.Pointer(&scan)))
	return scan.Viewers
}

func feedbackScreen() rect {
	metric := func(i uintptr) int32 { n, _, _ := user32.NewProc("GetSystemMetrics").Call(i); return int32(n) }
	x, y := metric(76), metric(77) // SM_X/YVIRTUALSCREEN, including negative origins.
	return rect{x, y, x + metric(78), y + metric(79)}
}

func feedbackViewerAt(viewers []feedbackViewer, p point) int {
	for _, w := range viewers {
		if p.X >= w.Bounds.Left && p.X < w.Bounds.Right && p.Y >= w.Bounds.Top && p.Y < w.Bounds.Bottom {
			return w.PID
		}
	}
	return 0
}

type dragFeedback struct {
	Window, Timer   uintptr
	PreviousDPI     uintptr
	OLE, Registered bool
	Screen          rect
	Viewers         []feedbackViewer
	Target          *feedbackDropTarget
}

// A zero-alpha layered window is also transparent to native hit-testing.
// Alpha 1 keeps this nearly transparent, non-activating target hittable.
// Real viewers are cut out of its region, leaving their native drops intact.
func newDragFeedback(token string, pids []int) *dragFeedback {
	if !validDragToken(token) || len(pids) == 0 || !registerFeedbackClass() {
		return nil
	}
	f := &dragFeedback{}
	hr, _, _ := ole32.NewProc("OleInitialize").Call(0)
	if int32(hr) < 0 {
		return nil
	}
	f.OLE = true
	success := false
	defer func() {
		if !success {
			f.close()
		}
	}()
	// The observing thread owns this window and COM apartment for its lifetime.
	if dpi := user32.NewProc("SetThreadDpiAwarenessContext"); dpi.Find() == nil {
		f.PreviousDPI, _, _ = dpi.Call(^uintptr(3)) // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 (-4).
	}
	f.Screen = feedbackScreen()
	if f.Screen.Right <= f.Screen.Left || f.Screen.Bottom <= f.Screen.Top {
		return nil
	}
	name, _ := windows.UTF16PtrFromString(detachTargetClass)
	var module windows.Handle
	_ = windows.GetModuleHandleEx(0, nil, &module)
	// WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED; WS_POPUP.
	f.Window, _, _ = user32.NewProc("CreateWindowExW").Call(0x80|0x08000000|0x80000, uintptr(unsafe.Pointer(name)), 0, 0x80000000,
		uintptr(f.Screen.Left), uintptr(f.Screen.Top), uintptr(f.Screen.Right-f.Screen.Left), uintptr(f.Screen.Bottom-f.Screen.Top), 0, 0, uintptr(module), 0)
	if f.Window == 0 {
		return nil
	}
	ok, _, _ := user32.NewProc("SetLayeredWindowAttributes").Call(f.Window, 0, 1, 2)
	if ok == 0 {
		return nil
	}
	f.Target = newFeedbackTarget(token)
	hr, _, _ = ole32.NewProc("RegisterDragDrop").Call(f.Window, uintptr(unsafe.Pointer(f.Target)))
	if int32(hr) < 0 {
		return nil
	}
	f.Registered = true
	if !f.updateRegion(pids) {
		return nil
	}
	// HWND_TOPMOST; SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER.
	ok, _, _ = user32.NewProc("SetWindowPos").Call(f.Window, ^uintptr(0), 0, 0, 0, 0, 0x1|0x2|0x10|0x40|0x200)
	if ok == 0 {
		return nil
	}
	f.Timer, _, _ = user32.NewProc("SetTimer").Call(0, 0, 40, 0)
	if f.Timer == 0 {
		return nil
	}
	success = true
	return f
}

func (f *dragFeedback) update() bool {
	mu.Lock()
	active := feedbackToken == f.Target.Token && !result.Cancelled && len(feedbackPIDs) != 0
	pids := append([]int(nil), feedbackPIDs...)
	mu.Unlock()
	return active && time.Now().Before(f.Target.Deadline) && f.updateRegion(pids)
}

func (f *dragFeedback) updateRegion(pids []int) bool {
	// Display reconfiguration is rare during a drag; fall back rather than
	// leave any stale screen geometry covering a viewer.
	if feedbackScreen() != f.Screen {
		return false
	}
	viewers := feedbackViewers(pids)
	if len(viewers) == 0 {
		return false
	}
	if slices.Equal(viewers, f.Viewers) {
		return true
	}
	region, _, _ := gdi32.NewProc("CreateRectRgn").Call(0, 0, uintptr(f.Screen.Right-f.Screen.Left), uintptr(f.Screen.Bottom-f.Screen.Top))
	if region == 0 {
		return false
	}
	owned := true
	defer func() {
		if owned {
			gdi32.NewProc("DeleteObject").Call(region)
		}
	}()
	for _, viewer := range viewers {
		r := viewer.Bounds
		hole, _, _ := gdi32.NewProc("CreateRectRgn").Call(uintptr(r.Left-f.Screen.Left), uintptr(r.Top-f.Screen.Top), uintptr(r.Right-f.Screen.Left), uintptr(r.Bottom-f.Screen.Top))
		if hole == 0 {
			return false
		}
		combined, _, _ := gdi32.NewProc("CombineRgn").Call(region, region, hole, 4) // RGN_DIFF
		gdi32.NewProc("DeleteObject").Call(hole)
		if combined == 0 {
			return false
		}
	}
	ok, _, _ := user32.NewProc("SetWindowRgn").Call(f.Window, region, 0)
	if ok == 0 {
		return false
	}
	owned = false // The window owns the region after SetWindowRgn succeeds.
	f.Viewers = viewers
	return true
}

func (f *dragFeedback) close() {
	if f.Timer != 0 {
		user32.NewProc("KillTimer").Call(0, f.Timer)
		f.Timer = 0
	}
	if f.Window != 0 {
		user32.NewProc("ShowWindow").Call(f.Window, 0)
		if f.Registered {
			ole32.NewProc("RevokeDragDrop").Call(f.Window)
			f.Registered = false
		}
		user32.NewProc("DestroyWindow").Call(f.Window)
		f.Window = 0
	}
	if f.OLE {
		ole32.NewProc("OleUninitialize").Call()
		f.OLE = false
	}
	if f.PreviousDPI != 0 {
		user32.NewProc("SetThreadDpiAwarenessContext").Call(f.PreviousDPI)
		f.PreviousDPI = 0
	}
	runtime.KeepAlive(f.Target)
	if f.Target != nil {
		feedbackRelease(f.Target)
		f.Target = nil
	}
}

type feedbackDropTarget struct {
	VTable   *feedbackDropTargetVTable
	Refs     atomic.Int32
	Token    string
	Deadline time.Time
	Matching bool
}
type feedbackDropTargetVTable struct {
	QueryInterface, AddRef, Release      uintptr
	DragEnter, DragOver, DragLeave, Drop uintptr
}

func newFeedbackTarget(token string) *feedbackDropTarget {
	t := &feedbackDropTarget{VTable: &feedbackDropVTable, Token: token, Deadline: time.Now().Add(2 * time.Minute)}
	t.Refs.Store(1)
	// COM pointers are outside Go's GC roots. Retain the object until its last
	// native reference releases, including a source still ending its drag.
	feedbackTargetsMu.Lock()
	feedbackTargets[t] = struct{}{}
	feedbackTargetsMu.Unlock()
	return t
}

var feedbackDropVTable = feedbackDropTargetVTable{
	windows.NewCallback(feedbackQueryInterface), windows.NewCallback(feedbackAddRef), windows.NewCallback(feedbackRelease),
	windows.NewCallback(feedbackDragEnter), windows.NewCallback(feedbackDragOver), windows.NewCallback(feedbackDragLeave), windows.NewCallback(feedbackDrop),
}
var feedbackIUnknown = windows.GUID{Data4: [8]byte{0xc0, 0, 0, 0, 0, 0, 0, 0x46}}
var feedbackIDropTarget = windows.GUID{Data1: 0x122, Data4: [8]byte{0xc0, 0, 0, 0, 0, 0, 0, 0x46}}

func feedbackQueryInterface(t *feedbackDropTarget, iid *windows.GUID, out **feedbackDropTarget) uintptr {
	if out == nil || iid == nil {
		return 0x80004003 // E_POINTER
	}
	*out = nil
	if *iid != feedbackIUnknown && *iid != feedbackIDropTarget {
		return 0x80004002 // E_NOINTERFACE
	}
	*out = t
	feedbackAddRef(t)
	return 0
}
func feedbackAddRef(t *feedbackDropTarget) uintptr { return uintptr(t.Refs.Add(1)) }
func feedbackRelease(t *feedbackDropTarget) uintptr {
	refs := t.Refs.Add(-1)
	if refs == 0 {
		feedbackTargetsMu.Lock()
		delete(feedbackTargets, t)
		feedbackTargetsMu.Unlock()
	}
	return uintptr(refs)
}

type feedbackDataObject struct{ VTable *feedbackDataVTable }
type feedbackDataVTable struct{ QueryInterface, AddRef, Release, GetData uintptr }
type feedbackFormat struct {
	Format       uint16
	TargetDevice uintptr
	Aspect       uint32
	Index        int32
	Medium       uint32
}
type feedbackMedium struct {
	Type    uint32
	Global  uintptr
	Release unsafe.Pointer
}

func feedbackDataMatches(t *feedbackDropTarget, data *feedbackDataObject) bool {
	if data == nil || data.VTable == nil || data.VTable.GetData == 0 {
		return false
	}
	format := feedbackFormat{Format: 13, Aspect: 1, Index: -1, Medium: 1} // CF_UNICODETEXT, DVASPECT_CONTENT, TYMED_HGLOBAL
	var medium feedbackMedium
	hr, _, _ := syscall.SyscallN(data.VTable.GetData, uintptr(unsafe.Pointer(data)), uintptr(unsafe.Pointer(&format)), uintptr(unsafe.Pointer(&medium)))
	if int32(hr) < 0 {
		return false
	}
	defer ole32.NewProc("ReleaseStgMedium").Call(uintptr(unsafe.Pointer(&medium)))
	if medium.Type != 1 || medium.Global == 0 {
		return false
	}
	n, _, _ := kernel32.NewProc("GlobalSize").Call(medium.Global)
	if n < 2 || n > 1024 || n%2 != 0 {
		return false
	}
	ptr, _, _ := kernel32.NewProc("GlobalLock").Call(medium.Global)
	if ptr == 0 {
		return false
	}
	defer kernel32.NewProc("GlobalUnlock").Call(medium.Global)
	text := make([]uint16, n/2)
	var read uintptr
	if windows.ReadProcessMemory(windows.CurrentProcess(), ptr, (*byte)(unsafe.Pointer(&text[0])), n, &read) != nil || read != n {
		return false
	}
	return matchesDragText(t.Token, text)
}

func feedbackPoint(packed uint64) point { return point{int32(packed), int32(packed >> 32)} }

func feedbackEffect(t *feedbackDropTarget, packed uint64, effect *uint32) int {
	if effect == nil {
		return 0
	}
	allowed := *effect
	*effect = 0
	mu.Lock()
	active := feedbackToken == t.Token && !result.Cancelled && len(feedbackPIDs) != 0
	pids := append([]int(nil), feedbackPIDs...)
	mu.Unlock()
	if !active || !t.Matching || time.Now().After(t.Deadline) {
		return 0
	}
	pid := feedbackViewerAt(feedbackViewers(pids), feedbackPoint(packed))
	if pid == 0 && allowed&dropMove != 0 {
		*effect = dropMove
	}
	return pid
}

func feedbackDragEnter(t *feedbackDropTarget, data *feedbackDataObject, keys uint32, packed uint64, effect *uint32) uintptr {
	t.Matching = feedbackDataMatches(t, data)
	feedbackEffect(t, packed, effect)
	return 0
}
func feedbackDragOver(t *feedbackDropTarget, keys uint32, packed uint64, effect *uint32) uintptr {
	feedbackEffect(t, packed, effect)
	return 0
}
func feedbackDragLeave(t *feedbackDropTarget) uintptr { t.Matching = false; return 0 }
func feedbackDrop(t *feedbackDropTarget, data *feedbackDataObject, keys uint32, packed uint64, effect *uint32) uintptr {
	t.Matching = feedbackDataMatches(t, data)
	pid := feedbackEffect(t, packed, effect)
	mu.Lock()
	if t.Matching && feedbackToken == t.Token && !result.Cancelled {
		// If a viewer moved under the pointer between region updates, preserve
		// its identity so the coordinator cancels/merges rather than detaching.
		p := feedbackPoint(packed)
		result = Result{Valid: true, Cancelled: pid == 0 && (effect == nil || *effect != dropMove), X: int(p.X), Y: int(p.Y), PID: pid}
	}
	mu.Unlock()
	user32.NewProc("PostThreadMessageW").Call(uintptr(windows.GetCurrentThreadId()), feedbackStopMessage, 0, 0)
	t.Matching = false
	return 0
}
