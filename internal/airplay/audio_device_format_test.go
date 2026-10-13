package airplay

import (
	"encoding/binary"
	"math"
	"testing"
)

func TestDecodeFloatStereoPassesSamplesThrough(t *testing.T) {
	format := deviceAudioFormat{Rate: 48000, Channels: 2, BitsPerSample: 32, Float: true}
	data := make([]byte, 16)
	binary.LittleEndian.PutUint32(data[0:], math.Float32bits(0.25))
	binary.LittleEndian.PutUint32(data[4:], math.Float32bits(-0.5))
	binary.LittleEndian.PutUint32(data[8:], math.Float32bits(1))
	binary.LittleEndian.PutUint32(data[12:], math.Float32bits(0))
	got, err := format.decodeStereo(nil, data, 2)
	if err != nil {
		t.Fatal(err)
	}
	want := []float32{0.25, -0.5, 1, 0}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("sample %d = %v, want %v", i, got[i], want[i])
		}
	}
}

func TestDecodeIntegerFormats(t *testing.T) {
	for _, tc := range []struct {
		name   string
		format deviceAudioFormat
		data   []byte
		want   float32
	}{
		{"s16", deviceAudioFormat{Rate: 44100, Channels: 1, BitsPerSample: 16}, []byte{0x00, 0xc0}, -0.5},
		{"s24", deviceAudioFormat{Rate: 44100, Channels: 1, BitsPerSample: 24}, []byte{0x00, 0x00, 0x40}, 0.5},
		{"s32", deviceAudioFormat{Rate: 44100, Channels: 1, BitsPerSample: 32}, []byte{0x00, 0x00, 0x00, 0x20}, 0.25},
		{"s24-in-32", deviceAudioFormat{Rate: 44100, Channels: 1, BitsPerSample: 32, ValidBits: 24}, []byte{0x00, 0x00, 0x00, 0xc0}, -0.5},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got, err := tc.format.decodeStereo(nil, tc.data, 1)
			if err != nil {
				t.Fatal(err)
			}
			// Mono goes to both channels unchanged.
			if got[0] != tc.want || got[1] != tc.want {
				t.Fatalf("decoded (%v,%v), want %v on both", got[0], got[1], tc.want)
			}
		})
	}
}

func TestDecodeSurroundDownmixesToStereo(t *testing.T) {
	// 5.1: FL FR FC LFE BL BR.
	format := deviceAudioFormat{Rate: 48000, Channels: 6, BitsPerSample: 32, Float: true,
		ChannelMask: speakerFrontLeft | speakerFrontRight | speakerFrontCenter | speakerLowFrequency | speakerBackLeft | speakerBackRight}
	values := []float32{0.4, 0, 0.2, 0.9, 0, 0.2}
	data := make([]byte, 4*len(values))
	for i, v := range values {
		binary.LittleEndian.PutUint32(data[4*i:], math.Float32bits(v))
	}
	got, err := format.decodeStereo(nil, data, 1)
	if err != nil {
		t.Fatal(err)
	}
	// Left = FL + 0.707*FC, right = FR + 0.707*FC + 0.707*BR, scaled by the
	// shared headroom factor so a full-scale 5.1 mix cannot clip; LFE is dropped.
	norm := float32(1 / (1 + 2*math.Sqrt2/2))
	wantL := (0.4 + 0.2*float32(math.Sqrt2/2)) * norm
	wantR := (0.2*float32(math.Sqrt2/2) + 0.2*float32(math.Sqrt2/2)) * norm
	if math.Abs(float64(got[0]-wantL)) > 1e-6 || math.Abs(float64(got[1]-wantR)) > 1e-6 {
		t.Fatalf("downmix = (%v,%v), want (%v,%v)", got[0], got[1], wantL, wantR)
	}
}

func TestDecodeRejectsUnsupportedFormats(t *testing.T) {
	for _, format := range []deviceAudioFormat{
		{Rate: 48000, Channels: 2, BitsPerSample: 8},
		{Rate: 48000, Channels: 2, BitsPerSample: 64, Float: true},
		{Rate: 0, Channels: 2, BitsPerSample: 16},
		{Rate: 48000, Channels: 0, BitsPerSample: 16},
	} {
		if err := format.validate(); err == nil {
			t.Fatalf("format %+v accepted, want error", format)
		}
	}
}
