//go:build windows

package airplay

import (
	"testing"
	"time"
)

func TestWindowsBootClockUsesSystemUptime(t *testing.T) {
	first := bootRelativeNow()
	coarse := getTickCount64Uptime()
	second := bootRelativeNow()
	if delta := first - coarse; delta < -time.Second || delta > time.Second {
		t.Fatalf("boot clock %v differs from system uptime %v by %v", first, coarse, delta)
	}
	if second < first {
		t.Fatalf("boot clock moved backward: %v then %v", first, second)
	}
}
