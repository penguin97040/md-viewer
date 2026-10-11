//go:build darwin && cgo && !bindings

package native

/*
#cgo CFLAGS: -x objective-c
#cgo LDFLAGS: -framework Cocoa -framework ApplicationServices
#include "native_darwin.h"
*/
import "C"

func Init()       { C.md_install() }
func Start() bool { return C.md_start() != 0 }
func End() Result {
	r := C.md_end()
	return Result{r.valid != 0, r.cancelled != 0, int(r.x), int(r.y), int(r.pid)}
}
func Placement(x, y int) Area {
	r := C.md_placement(C.int(x), C.int(y))
	return Area{int(r.x), int(r.y), int(r.w), int(r.h)}
}

func Position(area Area) bool {
	C.md_position(C.MDArea{x: C.int(area.X), y: C.int(area.Y), w: C.int(area.Width), h: C.int(area.Height)})
	return true
}

func Cursor() Result { return Result{} }
func ClientPoint(x, y int) (int, int, bool) {
	r := C.md_client_point(C.int(x), C.int(y))
	return int(r.x), int(r.y), r.valid != 0
}

func Visible() bool { return C.md_visible() != 0 }
