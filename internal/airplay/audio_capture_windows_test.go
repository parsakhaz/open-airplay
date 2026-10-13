//go:build windows

package airplay

import (
	"context"
	"encoding/binary"
	"testing"
	"time"
	"unsafe"
)

func TestParseWaveFormatExtensibleFloat(t *testing.T) {
	b := make([]byte, 40)
	binary.LittleEndian.PutUint16(b[0:], waveFormatExtensible)
	binary.LittleEndian.PutUint16(b[2:], 2)
	binary.LittleEndian.PutUint32(b[4:], 48000)
	binary.LittleEndian.PutUint16(b[14:], 32)
	binary.LittleEndian.PutUint16(b[16:], 22)
	binary.LittleEndian.PutUint16(b[18:], 32)
	binary.LittleEndian.PutUint32(b[20:], speakerFrontLeft|speakerFrontRight)
	*(*[16]byte)(b[24:]) = *(*[16]byte)(unsafe.Pointer(&subtypeIEEEFloat))
	got, err := parseWaveFormat(b)
	if err != nil {
		t.Fatal(err)
	}
	want := deviceAudioFormat{Rate: 48000, Channels: 2, BitsPerSample: 32, ValidBits: 32, Float: true, ChannelMask: 3}
	if got != want {
		t.Fatalf("format = %+v, want %+v", got, want)
	}
}

func TestParseWaveFormatPCM16(t *testing.T) {
	b := make([]byte, 18)
	binary.LittleEndian.PutUint16(b[0:], waveFormatPCM)
	binary.LittleEndian.PutUint16(b[2:], 2)
	binary.LittleEndian.PutUint32(b[4:], 44100)
	binary.LittleEndian.PutUint16(b[14:], 16)
	got, err := parseWaveFormat(b)
	if err != nil {
		t.Fatal(err)
	}
	if got.Float || got.Rate != 44100 || got.BitsPerSample != 16 {
		t.Fatalf("format = %+v", got)
	}
}

// TestWASAPILoopbackDeliversLiveFrames opens the real default output device.
// Loopback yields no packets while nothing plays, so this also covers the idle
// silence path: frames must flow, contiguous and close to real time.
func TestWASAPILoopbackDeliversLiveFrames(t *testing.T) {
	if err := probeDefaultAudioOutput(); err != nil {
		t.Skipf("no default output device: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ac, err := StartAudioCaptureWithOptions(ctx, AudioCodecALAC, AudioCaptureOptions{})
	if err != nil {
		t.Fatal(err)
	}
	defer ac.Stop()

	buf := make([]byte, 8192)
	var last audioPCMFramePosition
	start := time.Now()
	const frames = 125 // one second of ALAC
	for i := 0; i < frames; i++ {
		_, position, err := ac.readFramePosition(buf)
		if err != nil {
			t.Fatalf("frame %d: %v", i, err)
		}
		// Loopback stamps packets with their local playout time, slightly ahead
		// of now, and the A/V alignment delays them further.
		if age := time.Since(position.PTS); age < -windowsAudioVideoAlignment-50*time.Millisecond || age > 250*time.Millisecond {
			t.Fatalf("frame %d source age %v, want near the present", i, age)
		}
		if i > 0 && position.SourceRTP-last.SourceRTP != 352 {
			t.Fatalf("frame %d source step %d, want 352", i, position.SourceRTP-last.SourceRTP)
		}
		last = position
	}
	if elapsed := time.Since(start); elapsed > 2*time.Second {
		t.Fatalf("one second of audio took %v to arrive", elapsed)
	}
}

func TestListAudioDevicesMarksDefault(t *testing.T) {
	if err := probeDefaultAudioOutput(); err != nil {
		t.Skipf("no default output device: %v", err)
	}
	devices, err := ListAudioDevices()
	if err != nil {
		t.Fatal(err)
	}
	defaults := 0
	for _, d := range devices {
		if d.ID == "" {
			t.Fatalf("device without ID: %+v", d)
		}
		if d.Default {
			defaults++
		}
	}
	if defaults != 1 {
		t.Fatalf("%d default devices among %d, want exactly 1", defaults, len(devices))
	}
}

type countingSink struct{ idles int }

func (s *countingSink) setFormat(deviceAudioFormat) error   { return nil }
func (s *countingSink) packet([]byte, int, time.Time) error { return nil }
func (s *countingSink) silence(int, time.Time)              {}
func (s *countingSink) idle(time.Time)                      { s.idles++ }

// A chosen device that goes away (unplugged headphones) is opened again once it
// is back, instead of sending silence for the rest of the mirror.
func TestReopenLoopbackReopensAChosenDeviceAfterItFails(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	attempts := 0
	capture := func(id string) error {
		attempts++
		if id != "headphones" {
			t.Fatalf("captured %q, want the chosen device", id)
		}
		if attempts == 1 {
			return hresultError{op: "start", hr: audclntEDeviceInvalidated}
		}
		cancel() // the second attempt captures until the mirror ends
		return nil
	}
	sink := &countingSink{}
	if err := reopenLoopback(ctx, sink, func() (string, error) { return "headphones", nil }, capture); err != nil {
		t.Fatalf("reopenLoopback = %v", err)
	}
	if attempts != 2 {
		t.Fatalf("capture attempts = %d, want the device reopened (2)", attempts)
	}
	if sink.idles == 0 {
		t.Fatal("no silence was sent while the device was away")
	}
}

// Other errors are retried with a fresh capture each time, and give up after five.
func TestReopenLoopbackRetriesOtherErrorsThenGivesUp(t *testing.T) {
	attempts := 0
	capture := func(string) error {
		attempts++
		return hresultError{op: "start", hr: 0x80004005}
	}
	err := reopenLoopback(context.Background(), &countingSink{}, func() (string, error) { return "speakers", nil }, capture)
	if err == nil || attempts != 5 {
		t.Fatalf("reopenLoopback = %v after %d attempts, want an error after 5", err, attempts)
	}
}
