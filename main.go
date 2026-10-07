package main

import (
	"embed"
	"io/fs"
	"log"
	"os"
	"strings"

	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	"github.com/wailsapp/wails/v2/pkg/options/linux"
	"github.com/wailsapp/wails/v2/pkg/options/mac"
	"github.com/wailsapp/wails/v2/pkg/options/windows"

	"github.com/penguin97040/md-viewer/internal/settings"
)

// version is set at build time with -ldflags "-X main.version=...".
var version = "dev"

//go:embed all:frontend
var embedded embed.FS

func main() {
	files, err := fs.Sub(embedded, "frontend")
	if err != nil {
		log.Fatal(err)
	}

	s := settings.Load()
	app := NewApp(fileArgs(os.Args[1:]), s)

	bg := &options.RGBA{R: 0x17, G: 0x19, B: 0x1e, A: 255}
	winTheme := windows.Dark
	if s.Theme == "light" {
		bg = &options.RGBA{R: 255, G: 255, B: 255, A: 255}
		winTheme = windows.Light
	}

	err = wails.Run(&options.App{
		Title:            appName,
		Width:            1100,
		Height:           820,
		MinWidth:         420,
		MinHeight:        300,
		BackgroundColour: bg,
		AssetServer: &assetserver.Options{
			Assets:  files,
			Handler: &assetHandler{files: files},
		},
		DragAndDrop: &options.DragAndDrop{
			EnableFileDrop:     true,
			DisableWebViewDrop: true,
		},
		OnStartup:  app.startup,
		OnShutdown: app.shutdown,
		Bind:       []any{app},
		Windows: &windows.Options{
			Theme: winTheme,
		},
		SingleInstanceLock: &options.SingleInstanceLock{
			UniqueId:               "io.github.penguin97040.mdviewer",
			OnSecondInstanceLaunch: app.secondInstance,
		},
		Mac: &mac.Options{
			OnFileOpen: app.openFromOS,
		},
		Linux: &linux.Options{
			ProgramName: "md-viewer",
		},
	})
	if err != nil {
		log.Fatal(err)
	}
}

// fileArgs returns the command line arguments that are not flags.
func fileArgs(args []string) []string {
	var out []string
	for _, a := range args {
		if a != "" && !strings.HasPrefix(a, "-") {
			out = append(out, a)
		}
	}
	return out
}
