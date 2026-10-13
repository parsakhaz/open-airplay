//go:build !windows

package airplay

import (
	"context"
	"fmt"
)

func startPlatformAudioCapture(ctx context.Context, codec AudioCodec, opts AudioCaptureOptions) (*AudioCapture, error) {
	return startGStreamerAudioCapture(ctx, codec, opts)
}

// ListAudioDevices lists capturable output devices. Only Windows enumerates
// them; elsewhere AudioCaptureOptions.Device names a PulseAudio monitor.
func ListAudioDevices() ([]AudioDevice, error) {
	return nil, fmt.Errorf("listing audio devices is only supported on Windows")
}
