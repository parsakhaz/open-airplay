package airplay

import (
	"encoding/binary"
	"fmt"
	"math"
	"math/bits"
)

// Speaker position bits from WAVEFORMATEXTENSIBLE.dwChannelMask.
const (
	speakerFrontLeft         = 0x1
	speakerFrontRight        = 0x2
	speakerFrontCenter       = 0x4
	speakerLowFrequency      = 0x8
	speakerBackLeft          = 0x10
	speakerBackRight         = 0x20
	speakerFrontLeftOfCenter = 0x40
	speakerFrontRightOfCntr  = 0x80
	speakerBackCenter        = 0x100
	speakerSideLeft          = 0x200
	speakerSideRight         = 0x400
)

// deviceAudioFormat describes interleaved PCM as a capture device delivers it
// (for WASAPI, the shared-mode mix format).
type deviceAudioFormat struct {
	Rate          int
	Channels      int
	BitsPerSample int // container size
	ValidBits     int // 0 means BitsPerSample
	Float         bool
	ChannelMask   uint32
}

func (f deviceAudioFormat) String() string {
	kind := "int"
	if f.Float {
		kind = "float"
	}
	return fmt.Sprintf("%d Hz %d ch %s%d", f.Rate, f.Channels, kind, f.BitsPerSample)
}

func (f deviceAudioFormat) validate() error {
	if f.Rate < 8000 || f.Rate > 768000 {
		return fmt.Errorf("unsupported capture sample rate %d", f.Rate)
	}
	if f.Channels < 1 || f.Channels > 32 {
		return fmt.Errorf("unsupported capture channel count %d", f.Channels)
	}
	if f.Float {
		if f.BitsPerSample != 32 {
			return fmt.Errorf("unsupported float capture sample size %d", f.BitsPerSample)
		}
		return nil
	}
	switch f.BitsPerSample {
	case 16, 24, 32:
		return nil
	}
	return fmt.Errorf("unsupported integer capture sample size %d", f.BitsPerSample)
}

func (f deviceAudioFormat) bytesPerFrame() int {
	return f.Channels * f.BitsPerSample / 8
}

// stereoWeights returns per-channel left and right mix weights, scaled so
// neither output can exceed full scale.
func (f deviceAudioFormat) stereoWeights() (left, right []float32) {
	left = make([]float32, f.Channels)
	right = make([]float32, f.Channels)
	if f.Channels == 1 {
		left[0], right[0] = 1, 1
		return left, right
	}
	mask := f.ChannelMask
	if mask == 0 || bits.OnesCount32(mask) < f.Channels {
		mask = defaultChannelMask(f.Channels)
	}
	const half = math.Sqrt2 / 2
	channel := 0
	for bit := uint32(1); bit != 0 && channel < f.Channels; bit <<= 1 {
		if mask&bit == 0 {
			continue
		}
		var l, r float64
		switch bit {
		case speakerFrontLeft:
			l = 1
		case speakerFrontRight:
			r = 1
		case speakerFrontCenter:
			l, r = half, half
		case speakerLowFrequency:
		case speakerBackLeft, speakerSideLeft, speakerFrontLeftOfCenter:
			l = half
		case speakerBackRight, speakerSideRight, speakerFrontRightOfCntr:
			r = half
		default:
			l, r = 0.5, 0.5
		}
		left[channel], right[channel] = float32(l), float32(r)
		channel++
	}
	var sumL, sumR float32
	for i := range left {
		sumL += left[i]
		sumR += right[i]
	}
	if norm := max(sumL, sumR); norm > 1 {
		for i := range left {
			left[i] /= norm
			right[i] /= norm
		}
	}
	return left, right
}

func defaultChannelMask(channels int) uint32 {
	switch channels {
	case 2:
		return speakerFrontLeft | speakerFrontRight
	case 3:
		return speakerFrontLeft | speakerFrontRight | speakerFrontCenter
	case 4:
		return speakerFrontLeft | speakerFrontRight | speakerBackLeft | speakerBackRight
	case 6:
		return speakerFrontLeft | speakerFrontRight | speakerFrontCenter | speakerLowFrequency | speakerBackLeft | speakerBackRight
	case 8:
		return speakerFrontLeft | speakerFrontRight | speakerFrontCenter | speakerLowFrequency |
			speakerBackLeft | speakerBackRight | speakerSideLeft | speakerSideRight
	}
	return 1<<uint(channels) - 1
}

// decodeStereo converts frames of device PCM into interleaved stereo float in
// [-1, 1), appending to dst.
func (f deviceAudioFormat) decodeStereo(dst []float32, data []byte, frames int) ([]float32, error) {
	if err := f.validate(); err != nil {
		return dst, err
	}
	stride := f.bytesPerFrame()
	if len(data) < frames*stride {
		return dst, fmt.Errorf("capture packet has %d bytes, want %d", len(data), frames*stride)
	}
	weightsL, weightsR := f.stereoWeights()
	stereo := f.Channels == 2 && weightsL[0] == 1 && weightsR[1] == 1
	sampleBytes := f.BitsPerSample / 8
	for frame := 0; frame < frames; frame++ {
		base := frame * stride
		if stereo {
			dst = append(dst, f.sample(data[base:]), f.sample(data[base+sampleBytes:]))
			continue
		}
		var l, r float32
		for ch := 0; ch < f.Channels; ch++ {
			v := f.sample(data[base+ch*sampleBytes:])
			l += weightsL[ch] * v
			r += weightsR[ch] * v
		}
		dst = append(dst, l, r)
	}
	return dst, nil
}

func (f deviceAudioFormat) sample(b []byte) float32 {
	if f.Float {
		return math.Float32frombits(binary.LittleEndian.Uint32(b))
	}
	switch f.BitsPerSample {
	case 16:
		return float32(int16(binary.LittleEndian.Uint16(b))) / (1 << 15)
	case 24:
		v := int32(uint32(b[0])<<8|uint32(b[1])<<16|uint32(b[2])<<24) >> 8
		return float32(v) / (1 << 23)
	default:
		// Valid bits are left-justified in the 32-bit container.
		return float32(float64(int32(binary.LittleEndian.Uint32(b))) / (1 << 31))
	}
}
