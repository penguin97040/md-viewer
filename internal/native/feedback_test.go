package native

import (
	"strings"
	"testing"
	"unicode/utf16"
)

func TestNativeDragTextRequiresExactCurrentToken(t *testing.T) {
	token := strings.Repeat("ab", 32)
	text := func(s string) []uint16 { return append(utf16.Encode([]rune(s)), 0) }
	for _, tt := range []struct {
		name  string
		token string
		text  []uint16
		want  bool
	}{
		{"current", token, text(tabTextPrefix + token), true},
		{"clipboard padding", token, append(text(tabTextPrefix+token), 0, 0), true},
		{"foreign", token, text(tabTextPrefix + strings.Repeat("cd", 32)), false},
		{"unmarked", token, text(token), false},
		{"path", token, text("C:\\document.md"), false},
		{"extra prefix", token, text("x" + tabTextPrefix + token), false},
		{"extra suffix", token, text(tabTextPrefix + token + "x"), false},
		{"embedded nul", token, text(tabTextPrefix + token[:30] + "\x00" + token[31:]), false},
		{"no terminator", token, utf16.Encode([]rune(tabTextPrefix + token)), false},
		{"empty", token, nil, false},
		{"invalid issued token", strings.Repeat("z", 64), text(tabTextPrefix + strings.Repeat("z", 64)), false},
		{"short issued token", token[:63], text(tabTextPrefix + token[:63]), false},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if got := matchesDragText(tt.token, tt.text); got != tt.want {
				t.Fatalf("matching = %v, want %v", got, tt.want)
			}
		})
	}
}
