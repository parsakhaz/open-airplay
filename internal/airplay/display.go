package airplay

import "fmt"

// Display describes one active desktop output. Index is the zero-based output
// selector accepted by the Windows Desktop Duplication capture backend.
type Display struct {
	Index   int
	Name    string
	X       int
	Y       int
	Width   int
	Height  int
	Primary bool
}

func (d Display) String() string {
	primary := ""
	if d.Primary {
		primary = " (primary)"
	}
	return fmt.Sprintf("[%d] %s — %dx%d at %d,%d%s", d.Index, d.Name, d.Width, d.Height, d.X, d.Y, primary)
}
