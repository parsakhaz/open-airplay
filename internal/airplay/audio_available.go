package airplay

// SystemAudioCaptureAvailable reports whether this build can capture the
// computer's own sound for a mirror. Windows captures the output device with
// WASAPI loopback; elsewhere a PulseAudio or PipeWire monitor is used.
// The stdio control protocol announces it as hello.capabilities.audio.
func SystemAudioCaptureAvailable() bool {
	return true
}
