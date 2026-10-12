package airplay

import "runtime"

// SystemAudioCaptureAvailable reports whether this build can capture the
// computer's own sound for a mirror. Windows has no capture yet (WASAPI
// loopback is in progress); elsewhere a PulseAudio or PipeWire monitor is used.
// The stdio control protocol announces it as hello.capabilities.audio.
func SystemAudioCaptureAvailable() bool {
	return runtime.GOOS != "windows"
}
