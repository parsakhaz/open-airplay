package airplay

import "math"

const (
	resamplerHalfTaps = 24  // taps on each side of the interpolation point
	resamplerPhases   = 256 // kernel table resolution between input samples
	resamplerBeta     = 8.6 // Kaiser window shape (~-90 dB side lobes)
)

// stereoResampler converts interleaved stereo float samples from the capture
// device's mix rate to the AirPlay rate with a Kaiser-windowed sinc kernel.
// Output k is the band-limited value at input position k*inRate/44100, so a
// caller can stamp every output sample with the input sample time it came from.
type stereoResampler struct {
	inRate int
	table  [][]float32 // nil for 44.1 kHz passthrough

	history   []float32 // interleaved; history[0] is input index historyAt
	historyAt uint64
	inCount   uint64
	outCount  uint64
}

func newStereoResampler(inRate int) *stereoResampler {
	r := &stereoResampler{inRate: inRate}
	if inRate != audioSampleRate {
		r.table = resamplerKernel(inRate)
	}
	return r
}

func resamplerKernel(inRate int) [][]float32 {
	cutoff := 0.97 * math.Min(1, float64(audioSampleRate)/float64(inRate))
	besselBeta := besselI0(resamplerBeta)
	table := make([][]float32, resamplerPhases+1)
	for p := range table {
		frac := float64(p) / resamplerPhases
		row := make([]float32, 2*resamplerHalfTaps)
		var sum float64
		values := make([]float64, len(row))
		for t := range row {
			x := float64(t-resamplerHalfTaps+1) - frac
			w := x / resamplerHalfTaps
			window := 0.0
			if w > -1 && w < 1 {
				window = besselI0(resamplerBeta*math.Sqrt(1-w*w)) / besselBeta
			}
			values[t] = cutoff * sinc(cutoff*x) * window
			sum += values[t]
		}
		for t := range row {
			row[t] = float32(values[t] / sum) // unity gain at DC
		}
		table[p] = row
	}
	return table
}

func sinc(x float64) float64 {
	if x == 0 {
		return 1
	}
	return math.Sin(math.Pi*x) / (math.Pi * x)
}

func besselI0(x float64) float64 {
	sum, term := 1.0, 1.0
	for k := 1; k < 50; k++ {
		term *= (x / (2 * float64(k))) * (x / (2 * float64(k)))
		sum += term
		if term < 1e-12*sum {
			break
		}
	}
	return sum
}

// push appends input samples and emits every output sample whose kernel is
// fully covered by the input received so far.
func (r *stereoResampler) push(samples []float32, emit func(left, right float32, inputPos float64)) {
	r.history = append(r.history, samples...)
	r.inCount += uint64(len(samples) / 2)
	r.produce(r.inCount, emit)
}

// flush emits the remaining outputs up to the end of the input, treating
// samples after it as silence.
func (r *stereoResampler) flush(emit func(left, right float32, inputPos float64)) {
	end := r.inCount
	if r.table != nil {
		r.history = append(r.history, make([]float32, 2*resamplerHalfTaps)...)
		r.inCount += resamplerHalfTaps
	}
	r.produceUntil(end, r.inCount, emit)
}

func (r *stereoResampler) produce(available uint64, emit func(left, right float32, inputPos float64)) {
	r.produceUntil(math.MaxUint64, available, emit)
}

// produceUntil emits outputs whose integer input index is below limit and whose
// kernel ends before available.
func (r *stereoResampler) produceUntil(limit, available uint64, emit func(left, right float32, inputPos float64)) {
	rate := uint64(r.inRate)
	for {
		num := r.outCount * rate
		index := num / audioSampleRate
		frac := num % audioSampleRate
		if index >= limit {
			break
		}
		if r.table == nil {
			if index >= available {
				break
			}
			at := 2 * (index - r.historyAt)
			emit(r.history[at], r.history[at+1], float64(index))
			r.outCount++
			continue
		}
		if index+resamplerHalfTaps >= available {
			break
		}
		phase := float64(frac) * resamplerPhases / audioSampleRate
		p0 := int(phase)
		mix := float32(phase - float64(p0))
		row0, row1 := r.table[p0], r.table[p0+1]
		var left, right float32
		first := int64(index) - resamplerHalfTaps + 1
		for t := 0; t < 2*resamplerHalfTaps; t++ {
			in := first + int64(t)
			if in < int64(r.historyAt) {
				continue // before the segment start: silence
			}
			at := 2 * (uint64(in) - r.historyAt)
			k := row0[t] + mix*(row1[t]-row0[t])
			left += k * r.history[at]
			right += k * r.history[at+1]
		}
		emit(left, right, float64(index)+float64(frac)/audioSampleRate)
		r.outCount++
	}
	r.trim()
}

// trim discards history no future output can reach.
func (r *stereoResampler) trim() {
	next := r.outCount * uint64(r.inRate) / audioSampleRate
	keepFrom := next
	if r.table != nil {
		if next >= resamplerHalfTaps {
			keepFrom = next - resamplerHalfTaps + 1
		} else {
			keepFrom = 0
		}
	}
	if keepFrom <= r.historyAt {
		return
	}
	drop := 2 * (keepFrom - r.historyAt)
	if drop > uint64(len(r.history)) {
		drop = uint64(len(r.history))
	}
	r.history = append(r.history[:0], r.history[drop:]...)
	r.historyAt += drop / 2
}
