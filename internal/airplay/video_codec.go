package airplay

import (
	"fmt"
	"math"
)

// VideoCodec selects the compressed screen codec carried by AirPlay mirroring.
// The empty value remains H.264 for backwards-compatible programmatic use;
// command-line callers may request Auto and provide the local encoder gate in
// StreamConfig after capture preflight.
type VideoCodec string

const (
	VideoCodecAuto VideoCodec = "auto"
	VideoCodecH264 VideoCodec = "h264"
	VideoCodecHEVC VideoCodec = "hevc"
)

const featureScreenMultiCodec uint = 42

const (
	maximumH264MirrorWidth  = 1920
	maximumH264MirrorHeight = 1080
)

func normalizeVideoCodec(codec VideoCodec) VideoCodec {
	if codec == "" {
		return VideoCodecH264
	}
	return codec
}

// ValidateVideoCodec checks a command/config codec value.
func ValidateVideoCodec(codec string) error {
	switch VideoCodec(codec) {
	case "", VideoCodecAuto, VideoCodecH264, VideoCodecHEVC:
		return nil
	default:
		return fmt.Errorf("unknown video codec %q (want auto, h264, or hevc)", codec)
	}
}

func (i *ReceiverInfo) supportsVideoCodec(codec VideoCodec) bool {
	switch normalizeVideoCodec(codec) {
	case VideoCodecH264:
		return true
	case VideoCodecHEVC:
		// Apple's SupportsScreenMultiCodec property is feature index 42.
		return i != nil && i.HasFeature(featureScreenMultiCodec)
	default:
		return false
	}
}

type videoSelection struct {
	codec         VideoCodec
	width, height int
	reason        string
}

// selectVideo chooses one concrete codec and canvas from the final receiver
// snapshot. The interoperable screen sender path uses AVC at the receiver's
// nominal canvas. HEVC remains an explicit diagnostic override.
func (i *ReceiverInfo) selectVideo(requested VideoCodec, automaticHEVCAvailable bool) (videoSelection, error) {
	return i.selectVideoForSource(requested, automaticHEVCAvailable, 0, 0)
}

// selectVideoForSource applies the normal codec policy, then prefers the
// selected source display's native pixel size when the receiver advertises an
// explicit decoder maximum. H.264 stays at or below 1080p to preserve the
// verified low-latency software-encoder path; larger explicit HEVC profiles
// retain their existing 4K ceiling.
func (i *ReceiverInfo) selectVideoForSource(requested VideoCodec, automaticHEVCAvailable bool, sourceWidth, sourceHeight int) (videoSelection, error) {
	_ = automaticHEVCAvailable // Retained for programmatic compatibility.
	if requested == "" {
		requested = VideoCodecH264
	}
	if err := ValidateVideoCodec(string(requested)); err != nil {
		return videoSelection{}, err
	}

	switch requested {
	case VideoCodecH264:
		width, height := i.MirrorSize()
		selection := videoSelection{codec: VideoCodecH264, width: width, height: height, reason: "explicit H.264"}
		return i.preferSourceVideoCanvas(selection, sourceWidth, sourceHeight), nil
	case VideoCodecHEVC:
		width, height, err := i.videoCanvas(VideoCodecHEVC)
		if err != nil {
			return videoSelection{}, err
		}
		selection := videoSelection{codec: VideoCodecHEVC, width: width, height: height, reason: "explicit HEVC"}
		return i.preferSourceVideoCanvas(selection, sourceWidth, sourceHeight), nil
	case VideoCodecAuto:
		width, height := i.MirrorSize()
		selection := videoSelection{codec: VideoCodecH264, width: width, height: height, reason: "automatic compatibility path"}
		return i.preferSourceVideoCanvas(selection, sourceWidth, sourceHeight), nil
	default:
		panic("validated video codec has no selection policy")
	}
}

func (i *ReceiverInfo) preferSourceVideoCanvas(selection videoSelection, sourceWidth, sourceHeight int) videoSelection {
	if sourceWidth <= 0 || sourceHeight <= 0 || i == nil || len(i.Displays) == 0 {
		return selection
	}
	display := i.Displays[0]
	maximumW, maximumH := selection.width, selection.height
	if display.WidthPixelsMax > 0 && display.HeightPixelsMax > 0 {
		maximumW, maximumH = i.maximumVideoCanvas()
	} else if sourceWidth > maximumW || sourceHeight > maximumH {
		// Without an explicit decoder maximum, native source sizing may avoid an
		// unnecessary upscale but must not exceed the receiver's nominal canvas.
		return selection
	}
	if selection.codec == VideoCodecH264 {
		maximumW = min(maximumW, maximumH264MirrorWidth)
		maximumH = min(maximumH, maximumH264MirrorHeight)
	}
	if maximumW <= 0 || maximumH <= 0 {
		return selection
	}

	width, height := fitVideoSize(sourceWidth, sourceHeight, maximumW, maximumH)
	// H.264 and HEVC 4:2:0 encoders require even luma dimensions. Round down so
	// the source-aware choice never exceeds the receiver's advertised maximum.
	width &^= 1
	height &^= 1
	if width <= 0 || height <= 0 {
		return selection
	}
	selection.width = width
	selection.height = height
	if width == sourceWidth && height == sourceHeight {
		selection.reason += "; selected source display native size"
	} else {
		selection.reason += "; selected source display fitted to receiver/codec limit"
	}
	return selection
}

func (i *ReceiverInfo) selectVideoFrameRate(requested int) (int, error) {
	if requested < 0 {
		return 0, fmt.Errorf("video frame rate cannot be negative")
	}
	advertised := 0
	if i != nil && len(i.Displays) > 0 && i.Displays[0].MaxFPS > 0 {
		advertised = int(i.Displays[0].MaxFPS)
	}
	if requested > 0 {
		if advertised > 0 && requested > advertised {
			return 0, fmt.Errorf("requested %d fps exceeds receiver maximum %d fps", requested, advertised)
		}
		return requested, nil
	}
	return i.RecommendedVideoFPS(), nil
}

// highResolutionVideoCanvas implements the high-resolution display-size gate.
// A missing maximum falls back to nominal, a malformed
// maximum cannot shrink nominal, and the result is capped aspect-preserving to
// 3840x2160. A maximum of exactly 1920x1080 remains on the ordinary path.
func (i *ReceiverInfo) highResolutionVideoCanvas() (int, int, bool) {
	maximumW, maximumH := i.maximumVideoCanvas()
	if maximumW <= 1920 && maximumH <= 1080 {
		return 0, 0, false
	}
	if maximumW <= 0 || maximumH <= 0 {
		return 0, 0, false
	}
	return maximumW, maximumH, true
}

func (i *ReceiverInfo) maximumVideoCanvas() (int, int) {
	nominalW, nominalH := i.MirrorSize()
	maximumW, maximumH := i.MaxVideoSize()
	if maximumW < nominalW {
		maximumW = nominalW
	}
	if maximumH < nominalH {
		maximumH = nominalH
	}
	if maximumW <= 0 || maximumH <= 0 {
		return 0, 0
	}
	return fitVideoSize(maximumW, maximumH, 3840, 2160)
}

func fitVideoSize(width, height, maximumW, maximumH int) (int, int) {
	if width <= maximumW && height <= maximumH {
		return width, height
	}
	scale := math.Min(float64(maximumW)/float64(width), float64(maximumH)/float64(height))
	return int(math.Round(float64(width) * scale)), int(math.Round(float64(height) * scale))
}

func (i *ReceiverInfo) videoCanvas(codec VideoCodec) (int, int, error) {
	codec = normalizeVideoCodec(codec)
	if !i.supportsVideoCodec(codec) {
		return 0, 0, fmt.Errorf("receiver does not advertise AirPlay feature 42 (SupportsScreenMultiCodec) required for HEVC")
	}
	if codec == VideoCodecHEVC {
		// The high-resolution screen path uses PixelSizeMax only after HEVC/HDR
		// support has been established. The ordinary H.264 path uses the nominal
		// PixelSize instead.
		width, height := i.maximumVideoCanvas()
		return width, height, nil
	}
	width, height := i.MirrorSize()
	return width, height, nil
}
