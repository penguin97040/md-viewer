//go:build !windows || bindings

package native

const DragFeedback = false

func StartForTab(token string, pids []int) bool  { return Start() }
func UpdateDragWindows(token string, pids []int) {}
