//go:build !linux && !windows

package airplay

import "time"

// Platforms without a native implementation retain the process-relative
// monotonic fallback.
func bootRelativeNow() time.Duration {
	return time.Since(appStartTime)
}
