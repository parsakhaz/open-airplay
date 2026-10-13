package airplay

import (
	"encoding/binary"
	"math"
	"time"
)

const (
	// assemblerContinuityTolerance absorbs timestamp jitter between consecutive
	// device packets. Larger forward gaps are filled with silence; larger
	// overlaps are trimmed so no sample is emitted twice.
	assemblerContinuityTolerance = 2 * time.Millisecond
	// assemblerMaxGapFill bounds silence filling. A longer gap (sleep, device
	// outage) restarts the timeline with a matching source-position gap.
	assemblerMaxGapFill = 2 * time.Second
)

// capturedPCMFrame is one codec frame of interleaved stereo S16LE PCM at
// audioSampleRate, positioned on the capture timeline.
type capturedPCMFrame struct {
	pcm      []byte
	position audioPCMFramePosition
}

// pcmFrameAssembler turns timestamped device packets of any size and sample
// rate into exact AirPlay codec frames. Each frame carries the presentation
// time of its first sample and a sample-counted source position, so the
// existing audioRTPClock can map it onto the mirror clock.
type pcmFrameAssembler struct {
	frameSamples int

	rate       int
	resampler  *stereoResampler
	started    bool
	anchorPTS  time.Time // PTS of input sample anchorIndex in this segment
	anchorIdx  uint64
	inputCount uint64 // input samples fed into this segment

	segmentSource uint32 // source position of this segment's first output
	segmentOut    uint64 // outputs produced in this segment

	partial    []byte
	partialPos audioPCMFramePosition
	ready      []capturedPCMFrame
}

func newPCMFrameAssembler(frameSamples int) *pcmFrameAssembler {
	return &pcmFrameAssembler{frameSamples: frameSamples}
}

// setInputRate selects the sample rate of subsequent packets. A change of rate
// (for example after the default device changes) keeps the output timeline
// contiguous.
func (a *pcmFrameAssembler) setInputRate(rate int) {
	if rate == a.rate && a.resampler != nil {
		return
	}
	var endPTS time.Time
	if a.started {
		endPTS = a.expectedInputPTS()
		a.resampler.flush(a.emit)
	}
	a.rate = rate
	a.startSegment(endPTS, a.nextSourcePosition())
	a.started = !endPTS.IsZero()
}

func (a *pcmFrameAssembler) startSegment(pts time.Time, source uint32) {
	a.resampler = newStereoResampler(a.rate)
	a.anchorPTS = pts
	a.anchorIdx = 0
	a.inputCount = 0
	a.segmentSource = source
	a.segmentOut = 0
}

func (a *pcmFrameAssembler) nextSourcePosition() uint32 {
	return a.segmentSource + uint32(a.segmentOut)
}

func (a *pcmFrameAssembler) expectedInputPTS() time.Time {
	return a.anchorPTS.Add(samplesDurationAt(a.inputCount-a.anchorIdx, a.rate))
}

// writeStereo appends interleaved stereo float samples whose first sample was
// captured at pts.
func (a *pcmFrameAssembler) writeStereo(samples []float32, pts time.Time) {
	if a.resampler == nil {
		a.setInputRate(audioSampleRate)
	}
	frames := len(samples) / 2
	if frames == 0 {
		return
	}
	if !a.started {
		a.started = true
		a.anchorPTS = pts
	} else {
		delta := pts.Sub(a.expectedInputPTS())
		switch {
		case delta > assemblerMaxGapFill || delta < -assemblerMaxGapFill:
			a.restart(pts)
		case delta > assemblerContinuityTolerance:
			a.feedSilence(samplesAt(delta, a.rate))
		case delta < -assemblerContinuityTolerance:
			drop := samplesAt(-delta, a.rate)
			if drop >= frames {
				return
			}
			samples = samples[drop*2:]
			pts = pts.Add(samplesDurationAt(uint64(drop), a.rate))
		}
	}
	a.anchorPTS = pts
	a.anchorIdx = a.inputCount
	a.feed(samples)
}

// fillSilenceUntil extends the timeline with silence up to until. Loopback
// capture delivers no packets while nothing plays; this keeps frames flowing.
// startAt positions the first sample if nothing was captured yet.
func (a *pcmFrameAssembler) fillSilenceUntil(until, startAt time.Time) {
	if a.resampler == nil {
		a.setInputRate(audioSampleRate)
	}
	if !a.started {
		a.started = true
		a.anchorPTS = startAt
		a.anchorIdx = 0
	}
	delta := until.Sub(a.expectedInputPTS())
	if delta <= 0 {
		return
	}
	if delta > assemblerMaxGapFill {
		a.restart(until.Add(-assemblerMaxGapFill))
		delta = assemblerMaxGapFill
	}
	a.feedSilence(samplesAt(delta, a.rate))
}

// restart ends the current segment (padding its last frame with silence) and
// starts a new one at pts, leaving a source-position gap equal to the time gap.
func (a *pcmFrameAssembler) restart(pts time.Time) {
	a.resampler.flush(a.emit)
	for len(a.partial) > 0 {
		a.emit(0, 0, float64(a.inputCount))
	}
	next := a.nextSourcePosition()
	nextPTS := a.outputPTS(float64(a.segmentOut) * float64(a.rate) / audioSampleRate)
	if gap := pts.Sub(nextPTS); gap > 0 {
		next += uint32(durationToAudioSamples(gap))
	}
	a.startSegment(pts, next)
}

func (a *pcmFrameAssembler) feedSilence(frames int) {
	if frames <= 0 {
		return
	}
	a.feed(make([]float32, frames*2))
}

func (a *pcmFrameAssembler) feed(samples []float32) {
	a.inputCount += uint64(len(samples) / 2)
	a.resampler.push(samples, a.emit)
}

func (a *pcmFrameAssembler) outputPTS(inputPos float64) time.Time {
	offset := (inputPos - float64(a.anchorIdx)) / float64(a.rate)
	return a.anchorPTS.Add(time.Duration(math.Round(offset * float64(time.Second))))
}

func (a *pcmFrameAssembler) emit(left, right float32, inputPos float64) {
	if len(a.partial) == 0 {
		a.partial = make([]byte, 0, a.frameSamples*audioBytesPerSampleFrame)
		a.partialPos = audioPCMFramePosition{
			PTS:          a.outputPTS(inputPos),
			SourceRTP:    a.nextSourcePosition(),
			HasSourceRTP: true,
		}
	}
	a.partial = binary.LittleEndian.AppendUint16(a.partial, uint16(floatToS16(left)))
	a.partial = binary.LittleEndian.AppendUint16(a.partial, uint16(floatToS16(right)))
	a.segmentOut++
	if len(a.partial) == cap(a.partial) {
		a.ready = append(a.ready, capturedPCMFrame{pcm: a.partial, position: a.partialPos})
		a.partial = nil
	}
}

func (a *pcmFrameAssembler) popFrame() (capturedPCMFrame, bool) {
	if len(a.ready) == 0 {
		return capturedPCMFrame{}, false
	}
	frame := a.ready[0]
	a.ready[0] = capturedPCMFrame{}
	a.ready = a.ready[1:]
	return frame, true
}

func floatToS16(v float32) int16 {
	scaled := math.Round(float64(v) * 32768)
	if scaled > math.MaxInt16 {
		return math.MaxInt16
	}
	if scaled < math.MinInt16 {
		return math.MinInt16
	}
	return int16(scaled)
}

func samplesDurationAt(samples uint64, rate int) time.Duration {
	seconds := samples / uint64(rate)
	remainder := samples % uint64(rate)
	return time.Duration(seconds)*time.Second +
		time.Duration((remainder*uint64(time.Second)+uint64(rate)/2)/uint64(rate))
}

func samplesAt(d time.Duration, rate int) int {
	return int((int64(d)*int64(rate) + int64(time.Second)/2) / int64(time.Second))
}
