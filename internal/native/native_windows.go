//go:build windows && !bindings

package native

import (
	"golang.org/x/sys/windows"
	"runtime"
	"sync"
	"time"
	"unsafe"
)

var user32 = windows.NewLazySystemDLL("user32.dll")
var mu sync.Mutex
var result Result
var thread uint32
var ready chan bool
var hookDone chan struct{}
var hookStopping bool
var feedbackToken string
var feedbackPIDs []int
var keyboardCallback = windows.NewCallback(func(code int32, wparam uintptr, lparam unsafe.Pointer) uintptr {
	if code >= 0 && (wparam == 0x100 || wparam == 0x104) {
		key := (*uint32)(lparam)
		if *key == 0x1b {
			mu.Lock()
			result.Cancelled = true
			id := thread
			mu.Unlock()
			if id != 0 {
				user32.NewProc("PostThreadMessageW").Call(uintptr(id), feedbackStopMessage, 0, 0)
			}
		}
	}
	v, _, _ := user32.NewProc("CallNextHookEx").Call(0, uintptr(code), wparam, uintptr(lparam))
	return v
})

type point struct{ X, Y int32 }
type mouseData struct {
	Point             point
	Data, Flags, Time uint32
	Extra             uintptr
}

var mouseCallback = windows.NewCallback(func(code int32, wparam uintptr, lparam unsafe.Pointer) uintptr {
	if code >= 0 && wparam == 0x202 {
		data := (*mouseData)(lparam)
		pid := pointPID(data.Point)
		mu.Lock()
		result.Valid = true
		result.X = int(data.Point.X)
		result.Y = int(data.Point.Y)
		result.PID = pid
		mu.Unlock()
	}
	v, _, _ := user32.NewProc("CallNextHookEx").Call(0, uintptr(code), wparam, uintptr(lparam))
	return v
})

func pointPID(p point) int {
	packed := uint64(uint32(p.X)) | uint64(uint32(p.Y))<<32
	hwnd, _, _ := user32.NewProc("WindowFromPoint").Call(uintptr(packed))
	hwnd, _, _ = user32.NewProc("GetAncestor").Call(hwnd, 2)
	if detachTargetWindow(hwnd) {
		return 0
	}
	var pid uint32
	user32.NewProc("GetWindowThreadProcessId").Call(hwnd, uintptr(unsafe.Pointer(&pid)))
	return int(pid)
}
func Start() bool { return StartForTab("", nil) }

func StartForTab(token string, pids []int) bool {
	mu.Lock()
	if hookDone != nil {
		mu.Unlock()
		return false
	}
	result = Result{}
	hookStopping = false
	feedbackToken = token
	feedbackPIDs = append([]int(nil), pids...)
	ready = make(chan bool, 1)
	ch := ready
	done := make(chan struct{})
	hookDone = done
	mu.Unlock()
	go func() {
		runtime.LockOSThread()
		defer runtime.UnlockOSThread()
		var msg struct {
			Hwnd           uintptr
			Message        uint32
			Wparam, Lparam uintptr
			Time           uint32
			Point          point
			Private        uint32
		}
		user32.NewProc("PeekMessageW").Call(uintptr(unsafe.Pointer(&msg)), 0, 0, 0, 0)
		id := windows.GetCurrentThreadId()
		mu.Lock()
		thread = id
		mu.Unlock()
		var module windows.Handle
		_ = windows.GetModuleHandleEx(0, nil, &module)
		kh, _, _ := user32.NewProc("SetWindowsHookExW").Call(13, keyboardCallback, uintptr(module), 0)
		mh, _, _ := user32.NewProc("SetWindowsHookExW").Call(14, mouseCallback, uintptr(module), 0)
		var feedback *dragFeedback
		mu.Lock()
		stopping := hookStopping
		mu.Unlock()
		if kh != 0 && mh != 0 && !stopping {
			feedback = newDragFeedback(token, pids)
		}
		mu.Lock()
		stopping = hookStopping
		mu.Unlock()
		ch <- kh != 0 && mh != 0 && !stopping
		if kh != 0 && mh != 0 && !stopping {
			for {
				n, _, _ := user32.NewProc("GetMessageW").Call(uintptr(unsafe.Pointer(&msg)), 0, 0, 0)
				if int32(n) <= 0 {
					break
				}
				if msg.Hwnd == 0 && msg.Message == feedbackStopMessage {
					if feedback != nil {
						feedback.close()
						feedback = nil
					}
					continue
				}
				if msg.Hwnd == 0 && msg.Message == 0x113 && feedback != nil {
					if !feedback.update() {
						feedback.close()
						feedback = nil
					}
				}
				user32.NewProc("TranslateMessage").Call(uintptr(unsafe.Pointer(&msg)))
				user32.NewProc("DispatchMessageW").Call(uintptr(unsafe.Pointer(&msg)))
			}
		}
		if feedback != nil {
			feedback.close()
		}
		if kh != 0 {
			user32.NewProc("UnhookWindowsHookEx").Call(kh)
		}
		if mh != 0 {
			user32.NewProc("UnhookWindowsHookEx").Call(mh)
		}
		mu.Lock()
		thread = 0
		hookDone = nil
		close(done)
		mu.Unlock()
	}()
	return <-ch
}
func End() Result {
	mu.Lock()
	r := result
	id := thread
	done := hookDone
	hookStopping = true
	feedbackToken = ""
	feedbackPIDs = nil
	mu.Unlock()
	if id != 0 {
		user32.NewProc("PostThreadMessageW").Call(uintptr(id), 0x12, 0, 0)
	}
	if done != nil {
		select {
		case <-done:
		case <-time.After(250 * time.Millisecond):
		}
	}
	return r
}

func UpdateDragWindows(token string, pids []int) {
	mu.Lock()
	if feedbackToken != token || hookDone == nil {
		mu.Unlock()
		return
	}
	feedbackPIDs = append([]int(nil), pids...)
	id := thread
	mu.Unlock()
	if len(pids) == 0 && id != 0 {
		user32.NewProc("PostThreadMessageW").Call(uintptr(id), feedbackStopMessage, 0, 0)
	}
}
func Init() {}

type rect struct{ Left, Top, Right, Bottom int32 }
type monitorInfo struct {
	Size          uint32
	Monitor, Work rect
	Flags         uint32
}

func Placement(x, y int) Area {
	p := point{int32(x), int32(y)}
	packed := uint64(uint32(p.X)) | uint64(uint32(p.Y))<<32
	h, _, _ := user32.NewProc("MonitorFromPoint").Call(uintptr(packed), 2)
	mi := monitorInfo{Size: uint32(unsafe.Sizeof(monitorInfo{}))}
	ok, _, _ := user32.NewProc("GetMonitorInfoW").Call(h, uintptr(unsafe.Pointer(&mi)))
	if ok == 0 {
		return Area{0, 0, 1100, 820}
	}
	// Position uses desktop pixels; Wails sizes use logical pixels.
	var dx, dy uint32
	shcore := windows.NewLazySystemDLL("shcore.dll")
	shcore.NewProc("GetDpiForMonitor").Call(h, 0, uintptr(unsafe.Pointer(&dx)), uintptr(unsafe.Pointer(&dy)))
	if dx == 0 {
		dx = 96
	}
	if dy == 0 {
		dy = 96
	}
	pw := min(1100*int(dx)/96, int(mi.Work.Right-mi.Work.Left))
	ph := min(820*int(dy)/96, int(mi.Work.Bottom-mi.Work.Top))
	return Area{max(int(mi.Work.Left), min(x-80, int(mi.Work.Right)-pw)), max(int(mi.Work.Top), min(y-20, int(mi.Work.Bottom)-ph)), pw * 96 / int(dx), ph * 96 / int(dy)}
}

func Position(area Area) bool {
	hwnd := windowHandle()
	if hwnd == 0 {
		return false
	}
	// Wails SetPos adds the current monitor's work-area origin. This area uses
	// absolute desktop coordinates, including negative monitor origins.
	p := point{int32(area.X), int32(area.Y)}
	packed := uint64(uint32(p.X)) | uint64(uint32(p.Y))<<32
	monitor, _, _ := user32.NewProc("MonitorFromPoint").Call(uintptr(packed), 2)
	var dx, dy uint32
	windows.NewLazySystemDLL("shcore.dll").NewProc("GetDpiForMonitor").Call(monitor, 0, uintptr(unsafe.Pointer(&dx)), uintptr(unsafe.Pointer(&dy)))
	if dx == 0 {
		dx = 96
	}
	if dy == 0 {
		dy = 96
	}
	ok, _, _ := user32.NewProc("SetWindowPos").Call(hwnd, 0, uintptr(area.X), uintptr(area.Y), uintptr(area.Width*int(dx)/96), uintptr(area.Height*int(dy)/96), 0x4|0x10)
	return ok != 0
}

func Cursor() Result { return Result{} }

var pointMu sync.Mutex
var pointWindow uintptr
var pointBiggest int32
var clientPointCallback = windows.NewCallback(func(hwnd, lparam uintptr) uintptr {
	var pid uint32
	user32.NewProc("GetWindowThreadProcessId").Call(hwnd, uintptr(unsafe.Pointer(&pid)))
	var name [256]uint16
	user32.NewProc("GetClassNameW").Call(hwnd, uintptr(unsafe.Pointer(&name[0])), uintptr(len(name)))
	if pid == windows.GetCurrentProcessId() && windows.UTF16ToString(name[:]) == "wailsWindow" {
		var r rect
		user32.NewProc("GetClientRect").Call(hwnd, uintptr(unsafe.Pointer(&r)))
		area := (r.Right - r.Left) * (r.Bottom - r.Top)
		if area > pointBiggest {
			pointBiggest = area
			pointWindow = hwnd
		}
	}
	return 1
})

func windowHandle() uintptr {
	pointMu.Lock()
	defer pointMu.Unlock()
	pointWindow = 0
	pointBiggest = 0

	user32.NewProc("EnumWindows").Call(clientPointCallback, 0)
	return pointWindow
}

func ClientPoint(x, y int) (int, int, bool) {
	hwnd := windowHandle()
	if hwnd == 0 {
		return 0, 0, false
	}
	p := point{int32(x), int32(y)}
	ok, _, _ := user32.NewProc("ScreenToClient").Call(hwnd, uintptr(unsafe.Pointer(&p)))
	if ok == 0 {
		return 0, 0, false
	}
	dpi, _, _ := user32.NewProc("GetDpiForWindow").Call(hwnd)
	if dpi == 0 {
		dpi = 96
	}
	return int(p.X) * 96 / int(dpi), int(p.Y) * 96 / int(dpi), true
}

func Visible() bool {
	hwnd := windowHandle()
	if hwnd == 0 {
		return false
	}
	visible, _, _ := user32.NewProc("IsWindowVisible").Call(hwnd)
	iconic, _, _ := user32.NewProc("IsIconic").Call(hwnd)
	if visible == 0 || iconic != 0 {
		return false
	}
	var r rect
	ok, _, _ := user32.NewProc("GetWindowRect").Call(hwnd, uintptr(unsafe.Pointer(&r)))
	if ok == 0 || r.Right <= r.Left || r.Bottom <= r.Top {
		return false
	}
	monitor, _, _ := user32.NewProc("MonitorFromRect").Call(uintptr(unsafe.Pointer(&r)), 0)
	return monitor != 0
}
