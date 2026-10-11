package main

import (
	"context"
	"github.com/penguin97040/md-viewer/internal/native"
	"github.com/penguin97040/md-viewer/internal/session"
	"github.com/wailsapp/wails/v2/pkg/runtime"
)

func placeWindow(ctx context.Context, pos session.Position) {
	area := native.Placement(pos.X, pos.Y)
	runtime.WindowSetSize(ctx, area.Width, area.Height)
	if !native.Position(area) {
		runtime.WindowSetPosition(ctx, area.X, area.Y)
	}
}
