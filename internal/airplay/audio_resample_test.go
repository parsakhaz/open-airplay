package airplay

import (
	"math"
	"testing"
)

func resampleAll(t *testing.T, inRate int, input []float32) (left []float64, positions []float64) {
	t.Helper()
	r := newStereoResampler(inRate)
	collect := func(l, _ float32, pos float64) {
		left = append(left, float64(l))
		positions = append(positions, pos)
	}
	// Push in uneven chunks to exercise history handling.
	for start := 0; start < len(input); {
		end := min(len(input), start+2*(97+start%113))
		r.push(input[start:end], collect)
		start = end
	}
	r.flush(collect)
	return left, positions
}

func stereoSine(rate, frames int, freq, amp float64) []float32 {
	out := make([]float32, 2*frames)
	for i := 0; i < frames; i++ {
		v := float32(amp * math.Sin(2*math.Pi*freq*float64(i)/float64(rate)))
		out[2*i], out[2*i+1] = v, v
	}
	return out
}

func TestResampler48kTo44k1PreservesToneAndLength(t *testing.T) {
	const inRate = 48000
	input := stereoSine(inRate, inRate, 1000, 0.5) // one second of 1 kHz
	left, positions := resampleAll(t, inRate, input)
	if len(left) != audioSampleRate {
		t.Fatalf("output samples = %d, want %d", len(left), audioSampleRate)
	}
	// Compare against the ideal 1 kHz tone at 44.1 kHz away from the edges.
	var maxErr float64
	for k := 200; k < len(left)-200; k++ {
		want := 0.5 * math.Sin(2*math.Pi*1000*float64(k)/audioSampleRate)
		maxErr = math.Max(maxErr, math.Abs(left[k]-want))
	}
	if maxErr > 0.002 {
		t.Fatalf("max error vs ideal tone = %.5f, want < 0.002", maxErr)
	}
	// Output k sits at input position k*48000/44100.
	if want := 441.0 * 48000 / 44100; math.Abs(positions[441]-want) > 1e-9 {
		t.Fatalf("position of output 441 = %v, want %v", positions[441], want)
	}
}

func TestResamplerRejectsContentAboveNewNyquist(t *testing.T) {
	const inRate = 48000
	// 23 kHz is representable at 48 kHz but would alias to 21.1 kHz at 44.1 kHz.
	left, _ := resampleAll(t, inRate, stereoSine(inRate, inRate/2, 23000, 0.5))
	var peak float64
	for k := 200; k < len(left)-200; k++ {
		peak = math.Max(peak, math.Abs(left[k]))
	}
	if peak > 0.02 {
		t.Fatalf("aliased peak = %.4f, want < 0.02 (-28 dB)", peak)
	}
}

func TestResamplerPassesThroughAt44k1(t *testing.T) {
	input := stereoSine(audioSampleRate, 1000, 440, 0.7)
	left, positions := resampleAll(t, audioSampleRate, input)
	if len(left) != 1000 {
		t.Fatalf("output samples = %d, want 1000", len(left))
	}
	for k := range left {
		if left[k] != float64(input[2*k]) || positions[k] != float64(k) {
			t.Fatalf("sample %d = %v at %v, want %v at %d", k, left[k], positions[k], input[2*k], k)
		}
	}
}
