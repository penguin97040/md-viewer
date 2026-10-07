// Package settings stores viewer preferences as JSON in the user's config
// directory.
package settings

import (
	"encoding/json"
	"os"
	"path/filepath"
)

// Settings are the user's viewer preferences.
type Settings struct {
	Theme      string  `json:"theme"`      // "dark" or "light"
	Wrap       bool    `json:"wrap"`       // wrap code blocks and tables
	FontSize   int     `json:"fontSize"`   // px
	LineHeight float64 `json:"lineHeight"` // unitless
	Width      string  `json:"width"`      // "narrow", "medium" or "full"
	Font       string  `json:"font"`       // "sans" or "serif"
	LiveReload bool    `json:"liveReload"`
	TOC        bool    `json:"toc"` // sidebar open
}

// Defaults returns the settings used on first run.
func Defaults() Settings {
	return Settings{
		Theme:      "dark",
		Wrap:       true,
		FontSize:   17,
		LineHeight: 1.65,
		Width:      "medium",
		Font:       "sans",
		LiveReload: true,
		TOC:        true,
	}
}

func path() (string, error) {
	dir, err := os.UserConfigDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(dir, "md-viewer", "settings.json"), nil
}

// Load returns saved settings, falling back to defaults for anything
// missing or invalid.
func Load() Settings {
	s := Defaults()
	p, err := path()
	if err != nil {
		return s
	}
	b, err := os.ReadFile(p)
	if err != nil {
		return s
	}
	_ = json.Unmarshal(b, &s)
	return s.Clean()
}

// Save writes settings to disk.
func Save(s Settings) error {
	p, err := path()
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
		return err
	}
	b, err := json.MarshalIndent(s.Clean(), "", "  ")
	if err != nil {
		return err
	}
	tmp := p + ".tmp"
	if err := os.WriteFile(tmp, b, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, p)
}

// Clean clamps values into their allowed ranges.
func (s Settings) Clean() Settings {
	d := Defaults()
	if s.Theme != "dark" && s.Theme != "light" {
		s.Theme = d.Theme
	}
	s.FontSize = min(max(s.FontSize, 12), 28)
	if s.LineHeight < 1.2 || s.LineHeight > 2.2 {
		s.LineHeight = d.LineHeight
	}
	switch s.Width {
	case "narrow", "medium", "full":
	default:
		s.Width = d.Width
	}
	if s.Font != "sans" && s.Font != "serif" {
		s.Font = d.Font
	}
	return s
}
