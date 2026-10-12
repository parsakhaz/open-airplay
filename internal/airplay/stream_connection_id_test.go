package airplay

import "testing"

// Windows' clock can return the same nanosecond twice, which gave the audio
// and video streams the same RTSP URI.
func TestStreamConnectionIDsAreUniqueAndPositive(t *testing.T) {
	previous := newStreamConnectionID()
	for i := 0; i < 10000; i++ {
		id := newStreamConnectionID()
		if id <= previous {
			t.Fatalf("call %d returned %d after %d, want strictly increasing", i, id, previous)
		}
		previous = id
	}
}
