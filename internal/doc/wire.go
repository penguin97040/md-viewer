package doc

import (
	"encoding/json"
	"errors"
)

// UnmarshalJSON is the inverse of the compact frontend wire format, also used
// between the coordinator and window processes.
func (h *Heading) UnmarshalJSON(data []byte) error {
	var a []json.RawMessage
	if err := json.Unmarshal(data, &a); err != nil {
		return err
	}
	if len(a) != 4 {
		return errors.New("invalid heading tuple")
	}
	for i, v := range []any{&h.Level, &h.Text, &h.ID, &h.Chunk} {
		if err := json.Unmarshal(a[i], v); err != nil {
			return err
		}
	}
	return nil
}
func (c *ChunkInfo) UnmarshalJSON(data []byte) error {
	var a []int
	if err := json.Unmarshal(data, &a); err != nil {
		return err
	}
	if len(a) != 2 {
		return errors.New("invalid chunk tuple")
	}
	c.Bytes, c.Lines = a[0], a[1]
	return nil
}
