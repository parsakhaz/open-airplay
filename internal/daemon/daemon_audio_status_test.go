package daemon

import "testing"

// Sound that fails while video goes on shows in the status the app reads, per
// receiver, instead of only in the log.
func TestAudioFailureIsReportedPerStream(t *testing.T) {
	const target = "192.0.2.10"
	entry := &activeStream{device: "Den TV", deviceIP: target, state: StateStreaming}
	other := &activeStream{device: "Office TV", deviceIP: "192.0.2.11", state: StateStreaming}
	d := &Daemon{streams: map[string]*activeStream{target: entry, other.deviceIP: other}}

	d.recordAudioError(entry, "this PC's sound couldn't be captured: no output device")

	status := d.handleStatus()
	if len(status.Streams) != 2 {
		t.Fatalf("streams = %+v", status.Streams)
	}
	if got := status.Streams[0].AudioError; got != "this PC's sound couldn't be captured: no output device" {
		t.Fatalf("Den TV audio_error = %q", got)
	}
	if got := status.Streams[1].AudioError; got != "" {
		t.Fatalf("Office TV audio_error = %q, want none", got)
	}
}

// A late audio error from a stream that has already gone doesn't attach to
// anything.
func TestAudioFailureAfterTheStreamEndedIsIgnored(t *testing.T) {
	gone := &activeStream{deviceIP: "192.0.2.10"}
	d := &Daemon{streams: map[string]*activeStream{}}
	d.recordAudioError(gone, "the sound stream stopped")
	if gone.audioError != "" {
		t.Fatalf("audioError = %q on a stream that ended", gone.audioError)
	}
}
