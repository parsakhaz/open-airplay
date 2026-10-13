package airplay

import (
	"sync/atomic"
	"time"
)

var lastStreamConnectionID atomic.Int64

// newStreamConnectionID returns a time-based stream connection ID (also the
// stream's RTSP URI path) that is unique within the process. Coarse clocks can
// return the same nanosecond for the audio and video streams of one session.
func newStreamConnectionID() int64 {
	for {
		last := lastStreamConnectionID.Load()
		id := time.Now().UnixNano() & 0x7FFFFFFFFFFFFFFF
		if id <= last {
			id = last + 1
		}
		if lastStreamConnectionID.CompareAndSwap(last, id) {
			return id
		}
	}
}
