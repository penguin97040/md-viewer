package native

const tabTextPrefix = "md-viewer-tab:"

func validDragToken(token string) bool {
	if len(token) != 64 {
		return false
	}
	for _, c := range token {
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}

// The native pasteboard fallback carries the same opaque token as the custom
// MIME format. Require its exact ASCII payload and a UTF-16 terminator.
func matchesDragText(token string, text []uint16) bool {
	if !validDragToken(token) {
		return false
	}
	expected := tabTextPrefix + token
	if len(text) <= len(expected) || text[len(expected)] != 0 {
		return false
	}
	var different uint16
	for i := range expected {
		different |= text[i] ^ uint16(expected[i])
	}
	return different == 0
}
