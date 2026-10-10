//go:build !windows

package airplay

import (
	"context"
	"fmt"
)

func prepareWindowsCapture(context.Context, CaptureConfig) (*CapturePreparation, error) {
	return nil, fmt.Errorf("Windows Desktop Duplication capture is unavailable on this platform")
}

func startPreparedWindowsCapture(context.Context, context.Context, CaptureConfig) (*ScreenCapture, error) {
	return nil, fmt.Errorf("Windows Desktop Duplication capture is unavailable on this platform")
}
