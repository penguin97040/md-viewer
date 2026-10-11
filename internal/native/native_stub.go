//go:build (!windows && !linux && !darwin) || (!windows && !cgo) || bindings

package native

func Init()                   {}
func Start() bool             { return false }
func End() Result             { return Result{Cancelled: true} }
func Placement(x, y int) Area { return Area{0, 0, 1100, 820} }

func Position(area Area) bool { return false }

func Cursor() Result                        { return Result{} }
func ClientPoint(x, y int) (int, int, bool) { return 0, 0, false }

func Visible() bool { return false }
