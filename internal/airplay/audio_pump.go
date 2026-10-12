package airplay

import (
	"context"
	"encoding/binary"
	"fmt"
	"io"
	"math"
	"strings"
	"sync"
	"time"
)

const (
	// audioIdleFillLag is how far behind the present the pump synthesizes
	// silence while the device delivers nothing. It must exceed one device
	// period plus the poll interval so real packets rarely overlap fill.
	audioIdleFillLag = 10 * time.Millisecond
	// audioPumpQueueFrames bounds queued codec frames (about 2 s of ALAC). A
	// reader that falls further behind loses the oldest frames, which the
	// stream loop would discard as stale anyway.
	audioPumpQueueFrames = 256
)

// AudioCaptureOptions configures desktop audio capture for a mirror.
type AudioCaptureOptions struct {
	// TestTone replaces the desktop with a 440 Hz sine.
	TestTone bool
	// Device selects the output device whose audio is captured. On Windows it
	// is an endpoint ID or a case-insensitive friendly-name match from
	// ListAudioDevices; empty follows the default output device as it changes.
	// On Linux it names a PulseAudio monitor source.
	Device string
	// Offset shifts audio presentation time relative to the capture instant.
	// Positive values delay audio, negative values advance it.
	Offset time.Duration
}

// AudioDevice is one output device whose audio can be captured.
type AudioDevice struct {
	ID      string `json:"id"`
	Name    string `json:"name"`
	Default bool   `json:"default"`
}

// audioPacketSink receives device packets from a platform capture loop.
type audioPacketSink interface {
	setFormat(format deviceAudioFormat) error
	packet(data []byte, frames int, pts time.Time) error
	silence(frames int, pts time.Time)
	idle(now time.Time)
}

// pcmFramePump assembles packets from a platform capture loop into codec
// frames and hands them to AudioCapture through ReadPCMFramePosition.
type pcmFramePump struct {
	mu      sync.Mutex
	asm     *pcmFrameAssembler
	format  deviceAudioFormat
	scratch []float32
	offset  time.Duration
	started time.Time

	frames  chan capturedPCMFrame
	done    chan struct{}
	doneErr error
	once    sync.Once
	dropped uint64
}

func newPCMFramePump(frameSamples int, offset time.Duration) *pcmFramePump {
	return &pcmFramePump{
		asm:    newPCMFrameAssembler(frameSamples),
		offset: offset,
		frames: make(chan capturedPCMFrame, audioPumpQueueFrames),
		done:   make(chan struct{}),
	}
}

func (p *pcmFramePump) setFormat(format deviceAudioFormat) error {
	if err := format.validate(); err != nil {
		return err
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	p.format = format
	p.asm.setInputRate(format.Rate)
	return nil
}

func (p *pcmFramePump) packet(data []byte, frames int, pts time.Time) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	var err error
	p.scratch, err = p.format.decodeStereo(p.scratch[:0], data, frames)
	if err != nil {
		return err
	}
	p.asm.writeStereo(p.scratch, pts)
	p.publishLocked()
	return nil
}

// silence records frames the device flagged as silent without decoding them.
func (p *pcmFramePump) silence(frames int, pts time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if cap(p.scratch) < 2*frames {
		p.scratch = make([]float32, 2*frames)
	}
	p.scratch = p.scratch[:2*frames]
	clear(p.scratch)
	p.asm.writeStereo(p.scratch, pts)
	p.publishLocked()
}

func (p *pcmFramePump) idle(now time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.started.IsZero() {
		p.started = now
	}
	until := now.Add(-audioIdleFillLag)
	if until.Before(p.started) {
		return
	}
	p.asm.fillSilenceUntil(until, p.started)
	p.publishLocked()
}

func (p *pcmFramePump) publishLocked() {
	for {
		frame, ok := p.asm.popFrame()
		if !ok {
			return
		}
		frame.position.PTS = frame.position.PTS.Add(p.offset)
		select {
		case p.frames <- frame:
			continue
		default:
		}
		// Queue full: drop the oldest frame to keep the newest audio.
		select {
		case <-p.frames:
			p.dropped++
			if p.dropped == 1 || p.dropped%100 == 0 {
				dbg("[AUDIO] capture queue full; dropped %d old frames", p.dropped)
			}
		default:
		}
		select {
		case p.frames <- frame:
		default:
		}
	}
}

// finish ends the stream; queued frames remain readable before err (or EOF).
func (p *pcmFramePump) finish(err error) {
	p.once.Do(func() {
		p.doneErr = err
		close(p.done)
	})
}

func (p *pcmFramePump) ReadPCMFrame(dst []byte) (time.Time, error) {
	position, err := p.ReadPCMFramePosition(dst)
	return position.PTS, err
}

func (p *pcmFramePump) ReadPCMFramePosition(dst []byte) (audioPCMFramePosition, error) {
	var frame capturedPCMFrame
	select {
	case frame = <-p.frames:
	case <-p.done:
		select {
		case frame = <-p.frames:
		default:
			if p.doneErr != nil {
				return audioPCMFramePosition{}, p.doneErr
			}
			return audioPCMFramePosition{}, io.EOF
		}
	}
	if len(dst) != len(frame.pcm) {
		return audioPCMFramePosition{}, fmt.Errorf("audio capture frame is %d bytes, reader wants %d", len(frame.pcm), len(dst))
	}
	copy(dst, frame.pcm)
	return frame.position, nil
}

// startPumpedAudioCapture runs a platform capture loop on its own goroutine and
// returns an AudioCapture reading the frames it produces.
func startPumpedAudioCapture(ctx context.Context, codec AudioCodec, offset time.Duration,
	run func(ctx context.Context, sink audioPacketSink) error) (*AudioCapture, error) {
	ac, captureCtx, err := newAudioCaptureForCodec(ctx, codec)
	if err != nil {
		return nil, err
	}
	_, spf, _, _, _, _ := codec.Info()
	pump := newPCMFramePump(int(spf), offset)
	ac.pcmFrames = pump
	ac.framesOwnEnd = true
	go func() {
		err := run(captureCtx, pump)
		if captureCtx.Err() != nil {
			err = nil
		}
		if err != nil {
			err = fmt.Errorf("audio capture exited: %w", err)
		}
		ac.waitErr = err
		close(ac.waitCh)
		pump.finish(err)
	}()
	return ac, nil
}

// runTestTone feeds a 440 Hz sine at real-time pace in 10 ms packets.
func runTestTone(ctx context.Context, sink audioPacketSink) error {
	format := deviceAudioFormat{Rate: 48000, Channels: 2, BitsPerSample: 16}
	if err := sink.setFormat(format); err != nil {
		return err
	}
	const packetFrames = 480
	tone := newSineSource(format.Rate, 440, 0.25)
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	start := time.Now()
	var produced uint64
	for {
		select {
		case <-ctx.Done():
			return nil
		case now := <-ticker.C:
			for samplesDurationAt(produced+packetFrames, format.Rate) <= now.Sub(start) {
				sink.packet(tone.s16(packetFrames), packetFrames, start.Add(samplesDurationAt(produced, format.Rate)))
				produced += packetFrames
			}
		}
	}
}

type sineSource struct {
	rate, freq, amp float64
	n               uint64
}

func newSineSource(rate int, freq, amp float64) *sineSource {
	return &sineSource{rate: float64(rate), freq: freq, amp: amp}
}

// s16 returns the next frames of the tone as interleaved stereo S16LE.
func (s *sineSource) s16(frames int) []byte {
	out := make([]byte, frames*audioBytesPerSampleFrame)
	for i := 0; i < frames; i++ {
		v := uint16(int16(math.Round(s.amp * 32767 * math.Sin(2*math.Pi*s.freq*float64(s.n)/s.rate))))
		binary.LittleEndian.PutUint16(out[4*i:], v)
		binary.LittleEndian.PutUint16(out[4*i+2:], v)
		s.n++
	}
	return out
}

// offsetPCMFrameReader shifts capture PTS by a fixed offset.
type offsetPCMFrameReader struct {
	reader audioPCMFramePositionReader
	offset time.Duration
}

func (r offsetPCMFrameReader) ReadPCMFrame(dst []byte) (time.Time, error) {
	position, err := r.ReadPCMFramePosition(dst)
	return position.PTS, err
}

func (r offsetPCMFrameReader) ReadPCMFramePosition(dst []byte) (audioPCMFramePosition, error) {
	position, err := r.reader.ReadPCMFramePosition(dst)
	if err == nil && !position.PTS.IsZero() {
		position.PTS = position.PTS.Add(r.offset)
	}
	return position, err
}

// resolveAudioDevice matches an endpoint ID, then an exact friendly name, then
// a unique friendly-name substring (all case-insensitive).
func resolveAudioDevice(devices []AudioDevice, spec string) (AudioDevice, error) {
	want := strings.ToLower(strings.TrimSpace(spec))
	for _, d := range devices {
		if strings.ToLower(d.ID) == want || strings.ToLower(d.Name) == want {
			return d, nil
		}
	}
	var matches []AudioDevice
	for _, d := range devices {
		if strings.Contains(strings.ToLower(d.Name), want) {
			matches = append(matches, d)
		}
	}
	switch len(matches) {
	case 1:
		return matches[0], nil
	case 0:
		return AudioDevice{}, fmt.Errorf("no active output device matches %q (see -list-audio-devices)", spec)
	}
	names := make([]string, len(matches))
	for i, d := range matches {
		names[i] = d.Name
	}
	return AudioDevice{}, fmt.Errorf("output device %q is ambiguous: %s", spec, strings.Join(names, ", "))
}
