package airplay

import (
	"context"
	"strings"
	"testing"
)

func TestReceiverVideoCanvasUsesMaximumOnlyForCapabilityGatedHEVC(t *testing.T) {
	info := &ReceiverInfo{
		Features: uint64(1) << featureScreenMultiCodec,
		Displays: []DisplayInfo{{
			WidthPixels: 1280, HeightPixels: 720,
			WidthPixelsMax: 3840, HeightPixelsMax: 2160,
		}},
	}
	if width, height, err := info.videoCanvas(VideoCodecH264); err != nil || width != 1280 || height != 720 {
		t.Fatalf("H.264 canvas = %dx%d err=%v, want nominal 1280x720", width, height, err)
	}
	if width, height, err := info.videoCanvas(VideoCodecHEVC); err != nil || width != 3840 || height != 2160 {
		t.Fatalf("HEVC canvas = %dx%d err=%v, want maximum 3840x2160", width, height, err)
	}
	info.Features &^= uint64(1) << featureScreenMultiCodec
	if _, _, err := info.videoCanvas(VideoCodecHEVC); err == nil {
		t.Fatal("HEVC canvas accepted without feature 42")
	}
}

func TestAutomaticVideoSelectionUsesNominalH264(t *testing.T) {
	tests := []struct {
		name string
		info *ReceiverInfo
	}{
		{name: "multi-codec high resolution", info: &ReceiverInfo{
			Features: uint64(1) << featureScreenMultiCodec,
			Displays: []DisplayInfo{{WidthPixels: 1280, HeightPixels: 720, WidthPixelsMax: 3840, HeightPixelsMax: 2160}},
		}},
		{name: "legacy receiver", info: &ReceiverInfo{
			Displays: []DisplayInfo{{WidthPixels: 1280, HeightPixels: 720}},
		}},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			selection, err := test.info.selectVideo(VideoCodecAuto, true)
			if err != nil {
				t.Fatal(err)
			}
			if selection.codec != VideoCodecH264 || selection.width != 1280 || selection.height != 720 {
				t.Fatalf("selection = %s %dx%d, want H.264 1280x720", selection.codec, selection.width, selection.height)
			}
			if !strings.Contains(selection.reason, "compatibility") {
				t.Fatalf("reason = %q, want compatibility reason", selection.reason)
			}
		})
	}
}

func TestAutomaticVideoSelectionPrefersNativeSourceWithinAdvertisedMaximum(t *testing.T) {
	info := &ReceiverInfo{Displays: []DisplayInfo{{
		WidthPixels: 1280, HeightPixels: 720,
		WidthPixelsMax: 3840, HeightPixelsMax: 2160,
	}}}
	selection, err := info.selectVideoForSource(VideoCodecAuto, false, 1440, 900)
	if err != nil {
		t.Fatal(err)
	}
	if selection.codec != VideoCodecH264 || selection.width != 1440 || selection.height != 900 {
		t.Fatalf("selection = %s %dx%d, want H.264 1440x900", selection.codec, selection.width, selection.height)
	}
	if !strings.Contains(selection.reason, "native size") {
		t.Fatalf("reason = %q, want native source reason", selection.reason)
	}
}

func TestSourceVideoSelectionFitsCodecLimitAndRoundsEven(t *testing.T) {
	info := &ReceiverInfo{Displays: []DisplayInfo{{
		WidthPixels: 1280, HeightPixels: 720,
		WidthPixelsMax: 3840, HeightPixelsMax: 2160,
	}}}
	selection, err := info.selectVideoForSource(VideoCodecH264, false, 2561, 1601)
	if err != nil {
		t.Fatal(err)
	}
	if selection.width != 1728 || selection.height != 1080 {
		t.Fatalf("H.264 fitted source = %dx%d, want 1728x1080", selection.width, selection.height)
	}
	if selection.width%2 != 0 || selection.height%2 != 0 {
		t.Fatalf("H.264 fitted source = %dx%d, want even dimensions", selection.width, selection.height)
	}
}

func TestSourceVideoSelectionAvoidsUpscaleWithoutExplicitMaximum(t *testing.T) {
	info := &ReceiverInfo{Displays: []DisplayInfo{{WidthPixels: 1280, HeightPixels: 720}}}

	smaller, err := info.selectVideoForSource(VideoCodecAuto, false, 1024, 576)
	if err != nil {
		t.Fatal(err)
	}
	if smaller.width != 1024 || smaller.height != 576 {
		t.Fatalf("smaller source = %dx%d, want native 1024x576", smaller.width, smaller.height)
	}

	larger, err := info.selectVideoForSource(VideoCodecAuto, false, 1440, 900)
	if err != nil {
		t.Fatal(err)
	}
	if larger.width != 1280 || larger.height != 720 {
		t.Fatalf("larger source without maximum = %dx%d, want nominal 1280x720", larger.width, larger.height)
	}
}

func TestVideoFrameRateSelection(t *testing.T) {
	for _, test := range []struct {
		name      string
		info      *ReceiverInfo
		requested int
		want      int
		wantErr   string
	}{
		{name: "auto receiver 60", info: &ReceiverInfo{Displays: []DisplayInfo{{MaxFPS: 60}}}, want: 60},
		{name: "auto caps above 60", info: &ReceiverInfo{Displays: []DisplayInfo{{MaxFPS: 120}}}, want: 60},
		{name: "auto receiver 30", info: &ReceiverInfo{Displays: []DisplayInfo{{MaxFPS: 30}}}, want: 30},
		{name: "auto missing metadata", info: &ReceiverInfo{}, want: 30},
		{name: "explicit override", info: &ReceiverInfo{Displays: []DisplayInfo{{MaxFPS: 60}}}, requested: 50, want: 50},
		{name: "explicit without metadata", info: &ReceiverInfo{}, requested: 60, want: 60},
		{name: "explicit above maximum", info: &ReceiverInfo{Displays: []DisplayInfo{{MaxFPS: 30}}}, requested: 60, wantErr: "exceeds receiver maximum"},
		{name: "negative", info: &ReceiverInfo{}, requested: -1, wantErr: "cannot be negative"},
	} {
		t.Run(test.name, func(t *testing.T) {
			got, err := test.info.selectVideoFrameRate(test.requested)
			if test.wantErr != "" {
				if err == nil || !strings.Contains(err.Error(), test.wantErr) {
					t.Fatalf("selectVideoFrameRate(%d) error = %v, want %q", test.requested, err, test.wantErr)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if got != test.want {
				t.Fatalf("selectVideoFrameRate(%d) = %d, want %d", test.requested, got, test.want)
			}
		})
	}
}

func TestExplicitHEVCSelectionCapsMaximumPreservingAspect(t *testing.T) {
	info := &ReceiverInfo{
		Features: uint64(1) << featureScreenMultiCodec,
		Displays: []DisplayInfo{{
			WidthPixels: 1920, HeightPixels: 1080,
			WidthPixelsMax: 7680, HeightPixelsMax: 2160,
		}},
	}
	forced, err := info.selectVideo(VideoCodecHEVC, false)
	if err != nil {
		t.Fatal(err)
	}
	if forced.width != 3840 || forced.height != 1080 {
		t.Fatalf("forced HEVC canvas = %dx%d, want capped 3840x1080", forced.width, forced.height)
	}
}

func TestExplicitVideoCodecOverridesAutomaticPolicy(t *testing.T) {
	info := &ReceiverInfo{
		Features: uint64(1) << featureScreenMultiCodec,
		Displays: []DisplayInfo{{
			WidthPixels: 1280, HeightPixels: 720,
			WidthPixelsMax: 3840, HeightPixelsMax: 2160,
		}},
	}
	h264, err := info.selectVideo(VideoCodecH264, true)
	if err != nil {
		t.Fatal(err)
	}
	if h264.codec != VideoCodecH264 || h264.width != 1280 || h264.height != 720 {
		t.Fatalf("explicit H.264 = %s %dx%d", h264.codec, h264.width, h264.height)
	}

	hevc, err := info.selectVideo(VideoCodecHEVC, false)
	if err != nil {
		t.Fatal(err)
	}
	if hevc.codec != VideoCodecHEVC || hevc.width != 3840 || hevc.height != 2160 {
		t.Fatalf("explicit HEVC = %s %dx%d", hevc.codec, hevc.width, hevc.height)
	}
}

func TestValidateVideoCodecAcceptsAuto(t *testing.T) {
	for _, codec := range []string{"", "auto", "h264", "hevc"} {
		if err := ValidateVideoCodec(codec); err != nil {
			t.Errorf("ValidateVideoCodec(%q): %v", codec, err)
		}
	}
	if err := ValidateVideoCodec("vp9"); err == nil || !strings.Contains(err.Error(), "auto, h264, or hevc") {
		t.Fatalf("ValidateVideoCodec(vp9) error = %v", err)
	}
}

func TestLegacyMirrorPreparationAPIRejectsAutomaticCodec(t *testing.T) {
	client := &AirPlayClient{}
	if _, err := client.SetupMirror(context.Background(), StreamConfig{VideoCodec: VideoCodecAuto}); err == nil || !strings.Contains(err.Error(), "VideoCodecPreparation") {
		t.Fatalf("SetupMirror(auto) error = %v", err)
	}
	if _, err := client.SetupMirrorWithVideoPreparation(context.Background(), StreamConfig{VideoCodec: VideoCodecAuto}, func(int, int) error { return nil }); err == nil || !strings.Contains(err.Error(), "VideoCodecPreparation") {
		t.Fatalf("legacy preparation(auto) error = %v", err)
	}
}
