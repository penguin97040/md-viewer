// Package native observes the web view's OS drag session. A failed HTML drop
// alone cannot distinguish a release outside the app from Escape cancellation.
package native

type Result struct {
	Valid     bool
	Cancelled bool
	X, Y      int
	PID       int
}
type Area struct{ X, Y, Width, Height int }
