//go:build windows

package airplay

import (
	"bytes"
	"context"
	"fmt"
	"log"
	"os/exec"
	"strconv"
	"strings"
	"time"
)

const windowsFFmpegProbeTimeout = 5 * time.Second

func prepareWindowsCapture(ctx context.Context, cfg CaptureConfig) (*CapturePreparation, error) {
	codec := normalizeVideoCodec(cfg.VideoCodec)
	if cfg.VideoCodec == VideoCodecAuto {
		codec = VideoCodecH264
	}
	if codec != VideoCodecH264 {
		return nil, fmt.Errorf("Windows Desktop Duplication capture currently supports H.264 only")
	}
	if cfg.DisplayIndex < 0 {
		return nil, fmt.Errorf("Windows display index must be zero or greater")
	}
	switch cfg.HWAccel {
	case "", "auto", "none", "nvenc":
	default:
		return nil, fmt.Errorf("Windows FFmpeg capture supports -hwaccel auto, none, or nvenc (got %q)", cfg.HWAccel)
	}

	ffmpeg, err := exec.LookPath("ffmpeg")
	if err != nil {
		return nil, fmt.Errorf("FFmpeg is required for Windows capture and was not found on PATH: %w", err)
	}
	probeCtx, cancel := context.WithTimeout(normalizeContext(ctx), windowsFFmpegProbeTimeout)
	defer cancel()
	probe := exec.CommandContext(probeCtx, ffmpeg, "-hide_banner", "-h", "filter=ddagrab")
	var output bytes.Buffer
	probe.Stdout = &output
	probe.Stderr = &output
	if err := probe.Run(); err != nil {
		if probeCtx.Err() != nil {
			return nil, fmt.Errorf("probe FFmpeg Desktop Duplication support: %w", probeCtx.Err())
		}
		return nil, fmt.Errorf("FFmpeg does not expose the ddagrab Desktop Duplication filter: %w; output: %s", err, strings.TrimSpace(output.String()))
	}
	if !strings.Contains(output.String(), "Grab Windows Desktop images using Desktop Duplication API") {
		return nil, fmt.Errorf("FFmpeg ddagrab filter is unavailable; install a Windows build with Desktop Duplication support")
	}

	displays, err := ListDisplays()
	if err != nil {
		return nil, err
	}
	if err := validateWindowsDisplayIndex(displays, cfg.DisplayIndex); err != nil {
		return nil, err
	}

	return &CapturePreparation{
		ctx:               ctx,
		cfg:               cfg,
		kind:              capturePreparationWindows,
		timestampedOutput: false,
	}, nil
}

func validateWindowsDisplayIndex(displays []Display, index int) error {
	for _, display := range displays {
		if display.Index == index {
			return nil
		}
	}
	return fmt.Errorf("display index %d is not an attached output on the default D3D11 adapter (use -list-displays to see available indices)", index)
}

func buildWindowsFFmpegArgs(cfg CaptureConfig) ([]string, error) {
	fps := cfg.FPS
	if fps <= 0 {
		fps = 30
	}
	if cfg.DisplayIndex < 0 {
		return nil, fmt.Errorf("display index must be zero or greater")
	}
	if normalizeVideoCodec(cfg.VideoCodec) != VideoCodecH264 {
		return nil, fmt.Errorf("Windows FFmpeg capture currently supports H.264 only")
	}

	drawMouse := 0
	if cfg.ShowCursor {
		drawMouse = 1
	}
	input := strings.Join([]string{
		"ddagrab=output_idx=" + strconv.Itoa(cfg.DisplayIndex),
		"draw_mouse=" + strconv.Itoa(drawMouse),
		"framerate=" + strconv.Itoa(fps),
		"output_fmt=8bit",
	}, ":")

	filters := []string{"hwdownload", "format=bgra"}
	if cfg.MaxWidth > 0 && cfg.MaxHeight > 0 {
		width := cfg.MaxWidth &^ 1
		height := cfg.MaxHeight &^ 1
		if width <= 0 || height <= 0 {
			return nil, fmt.Errorf("receiver canvas %dx%d is too small for 4:2:0 H.264", cfg.MaxWidth, cfg.MaxHeight)
		}
		filters = append(filters,
			fmt.Sprintf("scale=w=%d:h=%d:force_original_aspect_ratio=decrease:flags=fast_bilinear", width, height),
			fmt.Sprintf("pad=%d:%d:(ow-iw)/2:(oh-ih)/2:color=black", width, height),
			"setsar=1",
		)
	}
	filters = append(filters, "format=yuv420p")

	bitrate := captureBitrateKbps(cfg)
	maxrate := bitrate + bitrate/4
	bufsize := vbvBufferKbit(bitrate, fps)
	keyint := keyframeIntervalFrames(fps)
	encoder := "libx264"
	encoderArgs := []string{
		"-preset", "superfast",
		"-tune", "zerolatency",
		"-x264-params", "repeat-headers=1:aud=1:sliced-threads=1",
	}
	if cfg.HWAccel == "nvenc" {
		encoder = "h264_nvenc"
		encoderArgs = []string{
			"-preset", "p4",
			"-tune", "ull",
			"-forced-idr", "1",
			"-aud", "1",
		}
	}

	args := []string{
		"-hide_banner",
		"-loglevel", "warning",
		"-nostdin",
		"-f", "lavfi",
		"-i", input,
		"-vf", strings.Join(filters, ","),
		"-an",
		"-c:v", encoder,
	}
	args = append(args, encoderArgs...)
	args = append(args,
		"-b:v", fmt.Sprintf("%dk", bitrate),
		"-maxrate", fmt.Sprintf("%dk", maxrate),
		"-bufsize", fmt.Sprintf("%dk", bufsize),
		"-g", strconv.Itoa(keyint),
		"-keyint_min", strconv.Itoa(keyint),
		"-sc_threshold", "0",
		"-bf", "0",
		"-pix_fmt", "yuv420p",
		"-flush_packets", "1",
		"-f", "h264",
		"pipe:1",
	)
	return args, nil
}

func startPreparedWindowsCapture(startupCtx, lifetimeCtx context.Context, cfg CaptureConfig) (*ScreenCapture, error) {
	startupCtx, cancelStartup := context.WithTimeout(normalizeContext(startupCtx), captureStartupBudget)
	defer cancelStartup()
	lifetimeCtx = normalizeContext(lifetimeCtx)
	captureCtx, cancel := context.WithCancel(lifetimeCtx)

	args, err := buildWindowsFFmpegArgs(cfg)
	if err != nil {
		cancel()
		return nil, err
	}
	ffmpeg, err := exec.LookPath("ffmpeg")
	if err != nil {
		cancel()
		return nil, fmt.Errorf("find FFmpeg: %w", err)
	}
	log.Printf("[CAPTURE] using FFmpeg Desktop Duplication display %d with H.264", cfg.DisplayIndex)
	dbg("[CAPTURE] ffmpeg %s", strings.Join(args, " "))
	cmd := exec.CommandContext(captureCtx, ffmpeg, args...)

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		cancel()
		return nil, fmt.Errorf("FFmpeg stdout pipe: %w", err)
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		cancel()
		_ = stdout.Close()
		return nil, fmt.Errorf("FFmpeg stderr pipe: %w", err)
	}
	waitResult, err := startGStreamerCommand(cmd)
	if err != nil {
		cancel()
		_ = stdout.Close()
		_ = stderr.Close()
		return nil, fmt.Errorf("start FFmpeg: %w", err)
	}

	stderrTail := newCaptureStderrTail()
	go logStderr("FFMPEG", stderr, stderrTail)
	capture := &ScreenCapture{
		cmd:    cmd,
		stdout: stdout,
		cancel: cancel,
		waitCh: make(chan struct{}),
		stderr: stderrTail,
	}
	go func() {
		capture.waitErr = <-waitResult
		close(capture.waitCh)
	}()

	fps := cfg.FPS
	if fps <= 0 {
		fps = 30
	}
	probeCtx, cancelProbe := context.WithTimeout(startupCtx, captureStartupProbeTimeout(fps))
	err = probeVideoCaptureStartup(probeCtx, capture, VideoCodecH264, false)
	cancelProbe()
	if err != nil {
		capture.Stop()
		result := fmt.Errorf("Desktop Duplication capture for display %d failed startup validation: %w", cfg.DisplayIndex, err)
		if diagnostics := capture.stderrText(); diagnostics != "" {
			result = fmt.Errorf("%w; FFmpeg stderr: %s", result, diagnostics)
		}
		return nil, result
	}
	return capture, nil
}
