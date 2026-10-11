//go:build windows && !bindings && !amd64 && !arm64

package native

// The desktop distribution uses 64-bit Windows. Other architectures retain
// outside-release observation without registering a POINTL-by-value callback.
const DragFeedback = false
const feedbackStopMessage = 0x8001

type dragFeedback struct{}

func newDragFeedback(token string, pids []int) *dragFeedback { return nil }
func (f *dragFeedback) update() bool                         { return false }
func (f *dragFeedback) close()                               {}
func detachTargetWindow(hwnd uintptr) bool                   { return false }
