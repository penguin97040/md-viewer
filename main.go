package main

import (
	"embed"
	"github.com/penguin97040/md-viewer/internal/session"
	"io/fs"
	"log"
	"os"
	"path/filepath"
	"runtime"
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
	var remote *session.Client
	var position *session.Position
	if !bindingMode {
		switch {
		case len(os.Args) == 2 && os.Args[1] == "--coordinator":
			if err := session.RunCoordinator(); err != nil {
				log.Fatal(err)
			}
			return
		case len(os.Args) == 2 && os.Args[1] == "--window":
			b, err := session.ReadBootstrap()
			if err != nil {
				log.Fatal(err)
			}
			remote, err = session.NewClient(b)
			if err != nil {
				log.Fatal(err)
			}
			position = b.Position
		default:
			paths := fileArgs(os.Args[1:])
			for i, p := range paths {
				if abs, e := filepath.Abs(p); e == nil {
					paths[i] = abs
				}
			}
			if err := session.Launch(paths); err != nil {
				log.Fatal(err)
			}
			return
		}
	}

	files, err := fs.Sub(embedded, "frontend")
	if err != nil {
		log.Fatal(err)
	}

	s := settings.Load()
	if remote != nil {
		if saved, e := remote.Settings(); e == nil {
			s = saved
		}
	}
	app := NewApp(remote, position)

	bg := &options.RGBA{R: darkBackground[0], G: darkBackground[1], B: darkBackground[2], A: 255}
	winTheme := windows.Dark
	if s.Theme == "light" {
		bg = &options.RGBA{R: 255, G: 255, B: 255, A: 255}
		winTheme = windows.Light
	}

	err = wails.Run(&options.App{
		Title:            appName,
		Frameless:        runtime.GOOS != "darwin",
		Width:            1100,
		Height:           820,
		MinWidth:         420,
		MinHeight:        300,
		BackgroundColour: bg,
		AssetServer: &assetserver.Options{
			Assets:  files,
			Handler: &assetHandler{files: files, remote: remote},
		},
		DragAndDrop: &options.DragAndDrop{
			EnableFileDrop:     true,
			DisableWebViewDrop: false,
		},
		OnStartup:     app.startup,
		OnDomReady:    app.domReady,
		OnBeforeClose: app.beforeClose,
		OnShutdown:    app.shutdown,
		Bind:          []any{app},
		Windows: &windows.Options{
			Theme:                             winTheme,
			DisableFramelessWindowDecorations: true,
		},
		Mac: &mac.Options{
			OnFileOpen: app.openFromOS,
			TitleBar:   mac.TitleBarHiddenInset(),
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
