//go:build windows && avsync

package airplay

// Manual A/V offset measurement for Windows mirroring. It plays a clip that
// flashes white and beeps at the same instant once a second (ffplay keeps the
// two in sync), captures the desktop and its sound through the same paths a
// mirror uses, and reports how far the video timestamps of each flash trail
// the audio timestamps of its beep. It flashes part of display 0 and plays
// sound, so it only runs with:
//
//	go test -tags avsync -run TestManualAVOffset -v ./internal/airplay
//
// Needs ffmpeg and ffplay on PATH and some output volume.

import (
	"context"
	"encoding/binary"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestManualAVOffset(t *testing.T) {
	dir := t.TempDir()
	clip := filepath.Join(dir, "sync.mp4")
	if out, err := exec.Command("ffmpeg", "-y", "-hide_banner", "-loglevel", "error",
		"-f", "lavfi", "-i", "color=c=black:s=640x480:r=60:d=12",
		"-f", "lavfi", "-i", "sine=f=1000:r=48000:d=12",
		"-filter_complex", `[0]drawbox=c=white:t=fill:enable='lt(mod(t\,1)\,0.1)'[v];[1]volume=enable='gte(mod(t\,1)\,0.1)':volume=0[a]`,
		"-map", "[v]", "-map", "[a]", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", clip).CombinedOutput(); err != nil {
		t.Fatalf("make sync clip: %v %s", err, out)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	prep, err := PrepareCapture(ctx, CaptureConfig{FPS: 30, DisplayIndex: 0, HWAccel: "auto", ShowCursor: true})
	if err != nil {
		t.Fatal(err)
	}
	capture, err := prep.Start(1920, 1080)
	if err != nil {
		t.Fatal(err)
	}
	defer capture.Stop()
	pump := newPCMFramePump(352, windowsAudioVideoAlignment)
	go withCOM(func() error { return runWASAPILoopback(ctx, pump, "") })

	// Windows video carries no source PTS: the mirror stamps each access unit
	// when it is read, so record the read time of every AUD-delimited picture.
	var mu sync.Mutex
	var pictureAt []time.Time
	var stream []byte
	videoDone := make(chan struct{})
	go func() {
		defer close(videoDone)
		parser := newH264Parser()
		buf := make([]byte, 1<<20)
		for ctx.Err() == nil {
			n, err := capture.Read(buf)
			if err != nil {
				return
			}
			now := time.Now()
			mu.Lock()
			for _, nal := range parser.Push(buf[:n]) {
				header := 0
				for header < len(nal) && nal[header] == 0 {
					header++
				}
				if header < len(nal) && nal[header] == 1 && header+1 < len(nal) && nal[header+1]&0x1f == 9 {
					pictureAt = append(pictureAt, now)
				}
				stream = append(stream, nal...)
			}
			mu.Unlock()
		}
	}()

	var beepAt []time.Time
	go func() {
		buf := make([]byte, 352*4)
		quietFrames := 0
		for ctx.Err() == nil {
			position, err := pump.ReadPCMFramePosition(buf)
			if err != nil {
				return
			}
			loud := -1
			for offset := 0; offset < len(buf); offset += 4 {
				v := int16(binary.LittleEndian.Uint16(buf[offset:]))
				if v > 40 || v < -40 {
					loud = offset / 4
					break
				}
			}
			if loud >= 0 && quietFrames > 50 {
				mu.Lock()
				beepAt = append(beepAt, position.PTS.Add(audioSamplesDuration(uint64(loud))))
				mu.Unlock()
			}
			if loud < 0 {
				quietFrames++
			} else {
				quietFrames = 0
			}
		}
	}()

	time.Sleep(time.Second)
	if out, err := exec.Command("ffplay", "-hide_banner", "-loglevel", "error", "-autoexit",
		"-alwaysontop", "-noborder", "-left", "300", "-top", "200", clip).CombinedOutput(); err != nil {
		t.Fatalf("ffplay: %v %s", err, out)
	}
	time.Sleep(time.Second)
	cancel()
	capture.Stop()
	<-videoDone

	mu.Lock()
	defer mu.Unlock()
	h264 := filepath.Join(dir, "mirror.h264")
	if err := os.WriteFile(h264, stream, 0o644); err != nil {
		t.Fatal(err)
	}
	stats, err := exec.Command("ffmpeg", "-hide_banner", "-loglevel", "error", "-i", h264,
		"-vf", "signalstats,metadata=print:key=lavfi.signalstats.YAVG:file=-", "-f", "null", "-").Output()
	if err != nil {
		t.Fatal(err)
	}
	var luma []float64
	for _, field := range strings.Fields(string(stats)) {
		if value, ok := strings.CutPrefix(field, "lavfi.signalstats.YAVG="); ok {
			v, _ := strconv.ParseFloat(value, 64)
			luma = append(luma, v)
		}
	}
	if len(luma) != len(pictureAt) {
		t.Fatalf("decoded %d pictures for %d access units", len(luma), len(pictureAt))
	}

	// A flash is the largest brightness rise near each beep.
	var offsets []time.Duration
	for _, beep := range beepAt {
		best, bestRise := -1, 0.5
		for i := 1; i < len(luma); i++ {
			d := pictureAt[i].Sub(beep)
			if d > -300*time.Millisecond && d < 300*time.Millisecond && luma[i]-luma[i-1] > bestRise {
				best, bestRise = i, luma[i]-luma[i-1]
			}
		}
		if best >= 0 {
			offsets = append(offsets, pictureAt[best].Sub(beep))
		}
	}
	if len(offsets) < 5 {
		t.Fatalf("matched %d flash/beep pairs, want at least 5 (is the output muted?)", len(offsets))
	}
	t.Logf("video minus audio per event: %v", offsets)
	sort.Slice(offsets, func(i, j int) bool { return offsets[i] < offsets[j] })
	t.Logf("median %v over %d events (min %v, max %v); positive means audio plays early",
		offsets[len(offsets)/2], len(offsets), offsets[0], offsets[len(offsets)-1])
}
