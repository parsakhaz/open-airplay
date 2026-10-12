package airplay

import (
	"context"
	"errors"
	"io"
	"strings"
	"testing"
	"time"
)

func TestPumpedCaptureEncodesTimestampedFramesFromDevicePackets(t *testing.T) {
	base := time.Now()
	ac, err := startPumpedAudioCapture(context.Background(), AudioCodecALAC, 40*time.Millisecond,
		func(ctx context.Context, sink audioPacketSink) error {
			if err := sink.setFormat(deviceAudioFormat{Rate: 48000, Channels: 2, BitsPerSample: 16}); err != nil {
				return err
			}
			tone := newSineSource(48000, 440, 0.25)
			// 100 ms of tone in 10 ms device packets.
			for i := 0; i < 10; i++ {
				if err := sink.packet(tone.s16(480), 480, base.Add(time.Duration(i)*10*time.Millisecond)); err != nil {
					return err
				}
			}
			return nil
		})
	if err != nil {
		t.Fatal(err)
	}
	defer ac.Stop()

	buf := make([]byte, 8192)
	var positions []audioPCMFramePosition
	for {
		n, position, err := ac.readFramePosition(buf)
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			t.Fatal(err)
		}
		if n == 0 {
			t.Fatal("empty encoded frame")
		}
		positions = append(positions, position)
	}
	// 100 ms at 44.1 kHz = 4410 samples = 12 whole 352-sample ALAC frames
	// (the resampler holds back a fraction of a millisecond).
	if len(positions) != 12 {
		t.Fatalf("frames = %d, want 12", len(positions))
	}
	for i, p := range positions {
		if p.SourceRTP != uint32(i*352) {
			t.Fatalf("frame %d source position %d, want %d", i, p.SourceRTP, i*352)
		}
		// The offset delays every frame's presentation time.
		want := base.Add(40 * time.Millisecond).Add(audioSamplesDuration(uint64(i * 352)))
		if d := p.PTS.Sub(want); d < -50*time.Microsecond || d > 50*time.Microsecond {
			t.Fatalf("frame %d PTS off by %v", i, d)
		}
	}
}

func TestPumpedCaptureReportsSourceFailureAfterQueuedFrames(t *testing.T) {
	failure := errors.New("device gone")
	ac, err := startPumpedAudioCapture(context.Background(), AudioCodecALAC, 0,
		func(ctx context.Context, sink audioPacketSink) error {
			sink.setFormat(deviceAudioFormat{Rate: 44100, Channels: 2, BitsPerSample: 16})
			sink.packet(make([]byte, 352*4), 352, time.Now())
			return failure
		})
	if err != nil {
		t.Fatal(err)
	}
	defer ac.Stop()
	<-ac.waitCh
	buf := make([]byte, 8192)
	// Frames captured before the failure are still delivered, then the error.
	if n, _, err := ac.ReadFrameAt(buf); err != nil || n == 0 {
		t.Fatalf("queued frame: n=%d err=%v", n, err)
	}
	if _, _, err := ac.ReadFrameAt(buf); !errors.Is(err, failure) {
		t.Fatalf("read error = %v, want %v", err, failure)
	}
}

func TestPumpIdleFillsSilenceBehindThePresent(t *testing.T) {
	pump := newPCMFramePump(352, 0)
	if err := pump.setFormat(deviceAudioFormat{Rate: 48000, Channels: 2, BitsPerSample: 32, Float: true}); err != nil {
		t.Fatal(err)
	}
	start := time.Now()
	pump.idle(start)
	pump.idle(start.Add(audioIdleFillLag + 100*time.Millisecond))
	got := len(pump.frames)
	// 100 ms of silence = 4410 samples -> 12 frames, minus resampler look-ahead.
	if got < 11 || got > 12 {
		t.Fatalf("queued frames = %d, want 11-12", got)
	}
	frame := <-pump.frames
	if d := frame.position.PTS.Sub(start); d != 0 {
		t.Fatalf("first silent frame PTS = start%+v, want start", d)
	}
}

func TestPumpQueueDropsOldestWhenReaderStalls(t *testing.T) {
	pump := newPCMFramePump(352, 0)
	pump.setFormat(deviceAudioFormat{Rate: 44100, Channels: 2, BitsPerSample: 16})
	base := time.Now()
	total := audioPumpQueueFrames + 10
	for i := 0; i < total; i++ {
		pump.packet(make([]byte, 352*4), 352, base.Add(audioSamplesDuration(uint64(i*352))))
	}
	if len(pump.frames) != audioPumpQueueFrames {
		t.Fatalf("queued = %d, want %d", len(pump.frames), audioPumpQueueFrames)
	}
	frame := <-pump.frames
	if frame.position.SourceRTP != 10*352 {
		t.Fatalf("oldest kept frame at %d, want %d", frame.position.SourceRTP, 10*352)
	}
}

func TestResolveAudioDevice(t *testing.T) {
	devices := []AudioDevice{
		{ID: "{0.0.0.00000000}.{aaa}", Name: "Speakers (Realtek(R) Audio)", Default: true},
		{ID: "{0.0.0.00000000}.{bbb}", Name: "Headphones (USB Audio)"},
		{ID: "{0.0.0.00000000}.{ccc}", Name: "Headphones (Bluetooth)"},
	}
	for _, tc := range []struct{ spec, wantID, wantErr string }{
		{spec: "{0.0.0.00000000}.{BBB}", wantID: "{0.0.0.00000000}.{bbb}"},
		{spec: "speakers", wantID: "{0.0.0.00000000}.{aaa}"},
		{spec: "headphones (bluetooth)", wantID: "{0.0.0.00000000}.{ccc}"},
		{spec: "headphones", wantErr: "ambiguous"},
		{spec: "hdmi", wantErr: "no active output device"},
	} {
		got, err := resolveAudioDevice(devices, tc.spec)
		if tc.wantErr != "" {
			if err == nil || !strings.Contains(err.Error(), tc.wantErr) {
				t.Fatalf("%q: error %v, want %q", tc.spec, err, tc.wantErr)
			}
			continue
		}
		if err != nil || got.ID != tc.wantID {
			t.Fatalf("%q: got %+v, %v; want %s", tc.spec, got, err, tc.wantID)
		}
	}
}
