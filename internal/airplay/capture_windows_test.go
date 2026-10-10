//go:build windows

package airplay

import (
	"strings"
	"testing"
)

func TestBuildWindowsFFmpegArgsUsesDesktopDuplicationAndAnnexB(t *testing.T) {
	args, err := buildWindowsFFmpegArgs(CaptureConfig{
		FPS:          60,
		Bitrate:      4000,
		HWAccel:      "none",
		VideoCodec:   VideoCodecH264,
		MaxWidth:     1921,
		MaxHeight:    1081,
		DisplayIndex: 2,
		ShowCursor:   true,
	})
	if err != nil {
		t.Fatal(err)
	}
	joined := strings.Join(args, " ")
	for _, want := range []string{
		"-f lavfi",
		"ddagrab=output_idx=2:draw_mouse=1:framerate=60:output_fmt=8bit",
		"hwdownload,format=bgra,scale=w=1920:h=1080",
		"pad=1920:1080",
		"-c:v libx264",
		"repeat-headers=1:aud=1:sliced-threads=1",
		"-bf 0",
		"-f h264 pipe:1",
	} {
		if !strings.Contains(joined, want) {
			t.Errorf("FFmpeg args %q do not contain %q", joined, want)
		}
	}
}

func TestBuildWindowsFFmpegArgsCanSelectNVENC(t *testing.T) {
	args, err := buildWindowsFFmpegArgs(CaptureConfig{
		FPS:          30,
		HWAccel:      "nvenc",
		VideoCodec:   VideoCodecH264,
		DisplayIndex: 0,
	})
	if err != nil {
		t.Fatal(err)
	}
	if joined := strings.Join(args, " "); !strings.Contains(joined, "-c:v h264_nvenc") {
		t.Fatalf("FFmpeg args = %q, want h264_nvenc", joined)
	}
}

func TestBuildWindowsFFmpegArgsRejectsInvalidDisplay(t *testing.T) {
	_, err := buildWindowsFFmpegArgs(CaptureConfig{
		VideoCodec:   VideoCodecH264,
		DisplayIndex: -1,
	})
	if err == nil || !strings.Contains(err.Error(), "display index") {
		t.Fatalf("error = %v, want invalid display index", err)
	}
}
