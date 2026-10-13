//go:build windows

package airplay

import (
	"context"
	"errors"
	"fmt"
	"log"
	"runtime"
	"syscall"
	"time"
	"unsafe"

	"golang.org/x/sys/windows"
)

// WASAPI loopback capture of a render endpoint, written against the COM
// vtables directly so the sender needs no cgo or extra runtime for sound.

var (
	wasapiOle32              = windows.NewLazySystemDLL("ole32.dll")
	procCoCreateInstance     = wasapiOle32.NewProc("CoCreateInstance")
	procPropVariantClear     = wasapiOle32.NewProc("PropVariantClear")
	wasapiKernel32           = windows.NewLazySystemDLL("kernel32.dll")
	procQueryPerformanceCntr = wasapiKernel32.NewProc("QueryPerformanceCounter")
	procQueryPerformanceFreq = wasapiKernel32.NewProc("QueryPerformanceFrequency")

	clsidMMDeviceEnumerator = windows.GUID{Data1: 0xBCDE0395, Data2: 0xE52F, Data3: 0x467C, Data4: [8]byte{0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E}}
	iidIMMDeviceEnumerator  = windows.GUID{Data1: 0xA95664D2, Data2: 0x9614, Data3: 0x4F35, Data4: [8]byte{0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6}}
	iidIAudioClient         = windows.GUID{Data1: 0x1CB9AD4C, Data2: 0xDBFA, Data3: 0x4C32, Data4: [8]byte{0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2}}
	iidIAudioCaptureClient  = windows.GUID{Data1: 0xC8ADBD64, Data2: 0xE71E, Data3: 0x48A0, Data4: [8]byte{0xA4, 0xDE, 0x18, 0x5C, 0x39, 0x5C, 0xD3, 0x17}}
	subtypeIEEEFloat        = windows.GUID{Data1: 0x00000003, Data2: 0x0000, Data3: 0x0010, Data4: [8]byte{0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71}}
	pkeyDeviceFriendlyName  = propertyKey{
		fmtid: windows.GUID{Data1: 0xA45C254E, Data2: 0xDF1C, Data3: 0x4EFD, Data4: [8]byte{0x80, 0x20, 0x67, 0xD1, 0x46, 0xA8, 0x50, 0xE0}},
		pid:   14,
	}
)

const (
	eRender                    = 0
	eConsole                   = 0
	deviceStateActive          = 1
	clsctxAll                  = 0x17
	stgmRead                   = 0
	audclntShareModeShared     = 0
	audclntStreamFlagsLoopback = 0x00020000
	audclntBufferFlagsSilent   = 0x2
	audclntBufferFlagsTSError  = 0x4
	audclntEDeviceInvalidated  = 0x88890004
	hresultENotFound           = 0x80070490
	vtLPWSTR                   = 31
	waveFormatPCM              = 1
	waveFormatIEEEFloat        = 3
	waveFormatExtensible       = 0xFFFE

	wasapiBufferDuration = 200 * time.Millisecond
	wasapiPollInterval   = 5 * time.Millisecond
	wasapiDeviceCheck    = time.Second
	wasapiReopenDelay    = 500 * time.Millisecond

	// windowsAudioVideoAlignment delays audio presentation to match the video
	// path. Windows video is stamped when FFmpeg hands over the encoded frame,
	// after capture and encode; loopback audio is stamped at its local playout
	// instant. Measured with a flash-and-beep clip through ddagrab + libx264 at
	// 30 fps, video trailed audio by 5-55 ms (medians 13 and 41 ms).
	windowsAudioVideoAlignment = 25 * time.Millisecond
)

type comObject struct{ vtbl *[32]uintptr }

type propertyKey struct {
	fmtid windows.GUID
	pid   uint32
}

type propVariant struct {
	vt  uint16
	_   [3]uint16
	val uintptr
	_   uintptr
}

var errAudioDeviceChanged = errors.New("default output device changed")

type hresultError struct {
	op string
	hr uint32
}

func (e hresultError) Error() string { return fmt.Sprintf("%s: HRESULT 0x%08X", e.op, e.hr) }

func checkHR(op string, hr uintptr) error {
	if int32(hr) < 0 {
		return hresultError{op: op, hr: uint32(hr)}
	}
	return nil
}

func isHR(err error, hr uint32) bool {
	var h hresultError
	return errors.As(err, &h) && h.hr == hr
}

func (o *comObject) release() {
	if o != nil {
		syscall.SyscallN(o.vtbl[2], uintptr(unsafe.Pointer(o)))
	}
}

// withCOM runs fn on a locked OS thread inside a multithreaded apartment.
func withCOM(fn func() error) error {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	// S_FALSE (already initialized on this thread) arrives as Errno(1) and
	// still needs the matching CoUninitialize.
	if err := windows.CoInitializeEx(0, windows.COINIT_MULTITHREADED); err != nil && err != syscall.Errno(1) {
		return fmt.Errorf("initialize COM: %w", err)
	}
	defer windows.CoUninitialize()
	return fn()
}

func newDeviceEnumerator() (*comObject, error) {
	var enum *comObject
	hr, _, _ := procCoCreateInstance.Call(
		uintptr(unsafe.Pointer(&clsidMMDeviceEnumerator)), 0, clsctxAll,
		uintptr(unsafe.Pointer(&iidIMMDeviceEnumerator)), uintptr(unsafe.Pointer(&enum)))
	if err := checkHR("create MMDeviceEnumerator", hr); err != nil {
		return nil, err
	}
	return enum, nil
}

func defaultRenderDevice(enum *comObject) (*comObject, error) {
	var device *comObject
	hr, _, _ := syscall.SyscallN(enum.vtbl[4], uintptr(unsafe.Pointer(enum)), eRender, eConsole, uintptr(unsafe.Pointer(&device)))
	if err := checkHR("get default output device", hr); err != nil {
		return nil, err
	}
	return device, nil
}

func deviceID(device *comObject) (string, error) {
	var id *uint16
	hr, _, _ := syscall.SyscallN(device.vtbl[5], uintptr(unsafe.Pointer(device)), uintptr(unsafe.Pointer(&id)))
	if err := checkHR("get device id", hr); err != nil {
		return "", err
	}
	defer windows.CoTaskMemFree(unsafe.Pointer(id))
	return windows.UTF16PtrToString(id), nil
}

func deviceFriendlyName(device *comObject) string {
	var store *comObject
	hr, _, _ := syscall.SyscallN(device.vtbl[4], uintptr(unsafe.Pointer(device)), stgmRead, uintptr(unsafe.Pointer(&store)))
	if checkHR("open property store", hr) != nil {
		return ""
	}
	defer store.release()
	var value propVariant
	hr, _, _ = syscall.SyscallN(store.vtbl[5], uintptr(unsafe.Pointer(store)),
		uintptr(unsafe.Pointer(&pkeyDeviceFriendlyName)), uintptr(unsafe.Pointer(&value)))
	if checkHR("read friendly name", hr) != nil {
		return ""
	}
	defer procPropVariantClear.Call(uintptr(unsafe.Pointer(&value)))
	if value.vt != vtLPWSTR || value.val == 0 {
		return ""
	}
	return windows.UTF16PtrToString(*(**uint16)(unsafe.Pointer(&value.val)))
}

func defaultRenderDeviceID(enum *comObject) (string, error) {
	device, err := defaultRenderDevice(enum)
	if err != nil {
		return "", err
	}
	defer device.release()
	return deviceID(device)
}

// listRenderDevices enumerates active output devices. The caller releases
// nothing; devices are released before returning.
func listRenderDevices(enum *comObject) ([]AudioDevice, error) {
	defaultID, _ := defaultRenderDeviceID(enum)
	var collection *comObject
	hr, _, _ := syscall.SyscallN(enum.vtbl[3], uintptr(unsafe.Pointer(enum)), eRender, deviceStateActive, uintptr(unsafe.Pointer(&collection)))
	if err := checkHR("enumerate output devices", hr); err != nil {
		return nil, err
	}
	defer collection.release()
	var count uint32
	hr, _, _ = syscall.SyscallN(collection.vtbl[3], uintptr(unsafe.Pointer(collection)), uintptr(unsafe.Pointer(&count)))
	if err := checkHR("count output devices", hr); err != nil {
		return nil, err
	}
	devices := make([]AudioDevice, 0, count)
	for i := uint32(0); i < count; i++ {
		var device *comObject
		hr, _, _ = syscall.SyscallN(collection.vtbl[4], uintptr(unsafe.Pointer(collection)), uintptr(i), uintptr(unsafe.Pointer(&device)))
		if checkHR("get output device", hr) != nil {
			continue
		}
		id, err := deviceID(device)
		if err == nil {
			devices = append(devices, AudioDevice{ID: id, Name: deviceFriendlyName(device), Default: id == defaultID})
		}
		device.release()
	}
	return devices, nil
}

func getDevice(enum *comObject, id string) (*comObject, error) {
	wide, err := windows.UTF16PtrFromString(id)
	if err != nil {
		return nil, err
	}
	var device *comObject
	hr, _, _ := syscall.SyscallN(enum.vtbl[5], uintptr(unsafe.Pointer(enum)), uintptr(unsafe.Pointer(wide)), uintptr(unsafe.Pointer(&device)))
	if err := checkHR("open output device", hr); err != nil {
		return nil, err
	}
	return device, nil
}

// ListAudioDevices lists active output devices whose audio can be captured.
func ListAudioDevices() ([]AudioDevice, error) {
	var devices []AudioDevice
	err := runOnCOMThread(func() error {
		enum, err := newDeviceEnumerator()
		if err != nil {
			return err
		}
		defer enum.release()
		devices, err = listRenderDevices(enum)
		return err
	})
	return devices, err
}

// probeDefaultAudioOutput returns nil when a default output device exists for
// WASAPI loopback capture.
func probeDefaultAudioOutput() error {
	return runOnCOMThread(func() error {
		enum, err := newDeviceEnumerator()
		if err != nil {
			return err
		}
		defer enum.release()
		_, err = defaultRenderDeviceID(enum)
		if isHR(err, hresultENotFound) {
			return fmt.Errorf("no audio output device is enabled")
		}
		return err
	})
}

func runOnCOMThread(fn func() error) error {
	result := make(chan error, 1)
	go func() { result <- withCOM(fn) }()
	return <-result
}

func startPlatformAudioCapture(ctx context.Context, codec AudioCodec, opts AudioCaptureOptions) (*AudioCapture, error) {
	opts.Offset += windowsAudioVideoAlignment
	if opts.TestTone {
		dbg("[AUDIO] using test tone (440 Hz sine wave)")
		return startPumpedAudioCapture(ctx, codec, opts.Offset, runTestTone)
	}
	// Resolve an explicit device up front so a typo fails the start, not later.
	if opts.Device != "" {
		devices, err := ListAudioDevices()
		if err != nil {
			return nil, err
		}
		device, err := resolveAudioDevice(devices, opts.Device)
		if err != nil {
			return nil, err
		}
		opts.Device = device.ID
		log.Printf("[AUDIO] capturing output device %q", device.Name)
	} else if err := probeDefaultAudioOutput(); err != nil {
		return nil, err
	}
	return startPumpedAudioCapture(ctx, codec, opts.Offset, func(ctx context.Context, sink audioPacketSink) error {
		return withCOM(func() error { return runWASAPILoopback(ctx, sink, opts.Device) })
	})
}

// runWASAPILoopback captures until ctx ends. With deviceID empty it follows
// the default output device; otherwise it keeps reopening that device.
func runWASAPILoopback(ctx context.Context, sink audioPacketSink, fixedID string) error {
	enum, err := newDeviceEnumerator()
	if err != nil {
		return err
	}
	defer enum.release()
	clock := newQPCClock()
	resolve := func() (string, error) {
		if fixedID != "" {
			return fixedID, nil
		}
		return defaultRenderDeviceID(enum)
	}
	capture := func(id string) error {
		return captureLoopback(ctx, enum, id, fixedID == "", sink, clock)
	}
	return reopenLoopback(ctx, sink, resolve, capture)
}

// reopenLoopback runs capture attempts until ctx ends: it resolves the device,
// captures until that fails, and after a failure keeps silence flowing for
// wasapiReopenDelay before resolving and opening the device again.
func reopenLoopback(ctx context.Context, sink audioPacketSink, resolve func() (string, error), capture func(id string) error) error {
	failures := 0
	for ctx.Err() == nil {
		// Each attempt starts clean: an earlier failure must not skip opening the device again.
		id, err := resolve()
		if err == nil {
			err = capture(id)
		}
		switch {
		case ctx.Err() != nil:
			return nil
		case errors.Is(err, errAudioDeviceChanged):
			log.Printf("[AUDIO] default output device changed; following it")
			failures = 0
			continue
		case isHR(err, audclntEDeviceInvalidated), isHR(err, hresultENotFound):
			dbg("[AUDIO] output device unavailable (%v); retrying", err)
		default:
			failures++
			if failures >= 5 {
				return err
			}
			log.Printf("[AUDIO] loopback capture error: %v; retrying", err)
		}
		// Keep silent frames flowing to the receiver while no device is open.
		deadline := time.Now().Add(wasapiReopenDelay)
		for time.Now().Before(deadline) && ctx.Err() == nil {
			sink.idle(time.Now())
			time.Sleep(wasapiPollInterval)
		}
	}
	return nil
}

func captureLoopback(ctx context.Context, enum *comObject, id string, followDefault bool, sink audioPacketSink, clock *qpcClock) error {
	device, err := getDevice(enum, id)
	if err != nil {
		return err
	}
	defer device.release()

	var client *comObject
	hr, _, _ := syscall.SyscallN(device.vtbl[3], uintptr(unsafe.Pointer(device)),
		uintptr(unsafe.Pointer(&iidIAudioClient)), clsctxAll, 0, uintptr(unsafe.Pointer(&client)))
	if err := checkHR("activate audio client", hr); err != nil {
		return err
	}
	defer client.release()

	var mix *byte
	hr, _, _ = syscall.SyscallN(client.vtbl[8], uintptr(unsafe.Pointer(client)), uintptr(unsafe.Pointer(&mix)))
	if err := checkHR("get mix format", hr); err != nil {
		return err
	}
	defer windows.CoTaskMemFree(unsafe.Pointer(mix))
	format, err := parseWaveFormat(unsafe.Slice(mix, waveFormatSize(mix)))
	if err != nil {
		return err
	}

	args := []uintptr{uintptr(unsafe.Pointer(client)), audclntShareModeShared, audclntStreamFlagsLoopback}
	args = appendReferenceTime(args, wasapiBufferDuration)
	args = appendReferenceTime(args, 0)
	args = append(args, uintptr(unsafe.Pointer(mix)), 0)
	hr, _, _ = syscall.SyscallN(client.vtbl[3], args...)
	if err := checkHR("initialize loopback capture", hr); err != nil {
		return err
	}

	var capture *comObject
	hr, _, _ = syscall.SyscallN(client.vtbl[14], uintptr(unsafe.Pointer(client)),
		uintptr(unsafe.Pointer(&iidIAudioCaptureClient)), uintptr(unsafe.Pointer(&capture)))
	if err := checkHR("get capture client", hr); err != nil {
		return err
	}
	defer capture.release()

	if err := sink.setFormat(format); err != nil {
		return err
	}
	hr, _, _ = syscall.SyscallN(client.vtbl[10], uintptr(unsafe.Pointer(client)))
	if err := checkHR("start loopback capture", hr); err != nil {
		return err
	}
	defer syscall.SyscallN(client.vtbl[11], uintptr(unsafe.Pointer(client)))
	dbg("[AUDIO] WASAPI loopback capture started: %s", format)

	blockAlign := format.bytesPerFrame()
	nextDeviceCheck := time.Now().Add(wasapiDeviceCheck)
	ticker := time.NewTicker(wasapiPollInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
		}
		gotPacket := false
		for {
			var packetFrames uint32
			hr, _, _ = syscall.SyscallN(capture.vtbl[5], uintptr(unsafe.Pointer(capture)), uintptr(unsafe.Pointer(&packetFrames)))
			if err := checkHR("get next packet size", hr); err != nil {
				return err
			}
			if packetFrames == 0 {
				break
			}
			var data *byte
			var frames, flags uint32
			var devicePosition, qpcPosition uint64
			hr, _, _ = syscall.SyscallN(capture.vtbl[3], uintptr(unsafe.Pointer(capture)),
				uintptr(unsafe.Pointer(&data)), uintptr(unsafe.Pointer(&frames)), uintptr(unsafe.Pointer(&flags)),
				uintptr(unsafe.Pointer(&devicePosition)), uintptr(unsafe.Pointer(&qpcPosition)))
			if err := checkHR("get capture buffer", hr); err != nil {
				return err
			}
			var pts time.Time
			if flags&audclntBufferFlagsTSError == 0 && qpcPosition != 0 {
				pts = clock.timeAt(qpcPosition)
			} else {
				pts = time.Now().Add(-samplesDurationAt(uint64(frames), format.Rate))
			}
			var packetErr error
			if flags&audclntBufferFlagsSilent != 0 || data == nil {
				sink.silence(int(frames), pts)
			} else {
				packetErr = sink.packet(unsafe.Slice(data, int(frames)*blockAlign), int(frames), pts)
			}
			hr, _, _ = syscall.SyscallN(capture.vtbl[4], uintptr(unsafe.Pointer(capture)), uintptr(frames))
			if packetErr != nil {
				return packetErr
			}
			if err := checkHR("release capture buffer", hr); err != nil {
				return err
			}
			gotPacket = true
		}
		now := time.Now()
		if !gotPacket {
			sink.idle(now)
		}
		if followDefault && now.After(nextDeviceCheck) {
			nextDeviceCheck = now.Add(wasapiDeviceCheck)
			clock.reanchor()
			if current, err := defaultRenderDeviceID(enum); err == nil && current != id {
				return errAudioDeviceChanged
			}
		}
	}
}

func waveFormatSize(format *byte) int {
	header := unsafe.Slice(format, 18)
	if uint16(header[0])|uint16(header[1])<<8 == waveFormatExtensible {
		return 40
	}
	return 18
}

// parseWaveFormat decodes WAVEFORMATEX / WAVEFORMATEXTENSIBLE bytes.
func parseWaveFormat(b []byte) (deviceAudioFormat, error) {
	if len(b) < 18 {
		return deviceAudioFormat{}, fmt.Errorf("mix format is %d bytes", len(b))
	}
	le16 := func(o int) uint16 { return uint16(b[o]) | uint16(b[o+1])<<8 }
	le32 := func(o int) uint32 { return uint32(le16(o)) | uint32(le16(o+2))<<16 }
	format := deviceAudioFormat{
		Rate:          int(le32(4)),
		Channels:      int(le16(2)),
		BitsPerSample: int(le16(14)),
	}
	switch tag := le16(0); tag {
	case waveFormatPCM:
	case waveFormatIEEEFloat:
		format.Float = true
	case waveFormatExtensible:
		if len(b) < 40 {
			return deviceAudioFormat{}, fmt.Errorf("extensible mix format is %d bytes", len(b))
		}
		format.ValidBits = int(le16(18))
		format.ChannelMask = le32(20)
		sub := *(*windows.GUID)(unsafe.Pointer(&b[24]))
		format.Float = sub == subtypeIEEEFloat
	default:
		return deviceAudioFormat{}, fmt.Errorf("unsupported mix format tag 0x%04X", tag)
	}
	return format, format.validate()
}

func appendReferenceTime(args []uintptr, d time.Duration) []uintptr {
	hns := int64(d / 100)
	if unsafe.Sizeof(uintptr(0)) == 8 {
		return append(args, uintptr(hns))
	}
	return append(args, uintptr(uint32(hns)), uintptr(uint32(uint64(hns)>>32)))
}

// qpcClock maps WASAPI QPC positions (100 ns units) onto time.Time values in
// the same monotonic domain as the video frames' timestamps.
type qpcClock struct {
	frequency  int64
	anchorQPC  uint64
	anchorTime time.Time
}

func newQPCClock() *qpcClock {
	c := &qpcClock{}
	procQueryPerformanceFreq.Call(uintptr(unsafe.Pointer(&c.frequency)))
	c.reanchor()
	return c
}

func (c *qpcClock) now100ns() uint64 {
	var counter int64
	procQueryPerformanceCntr.Call(uintptr(unsafe.Pointer(&counter)))
	seconds := counter / c.frequency
	remainder := counter % c.frequency
	return uint64(seconds)*10_000_000 + uint64(remainder*10_000_000/c.frequency)
}

// reanchor samples both clocks, keeping the tightest of a few readings so the
// mapping error stays well under a millisecond.
func (c *qpcClock) reanchor() {
	best := time.Duration(1<<63 - 1)
	for i := 0; i < 3; i++ {
		before := time.Now()
		qpc := c.now100ns()
		after := time.Now()
		if spread := after.Sub(before); spread < best {
			best = spread
			c.anchorQPC = qpc
			c.anchorTime = before.Add(spread / 2)
		}
	}
}

func (c *qpcClock) timeAt(qpc100ns uint64) time.Time {
	return c.anchorTime.Add(time.Duration(int64(qpc100ns-c.anchorQPC)) * 100)
}
