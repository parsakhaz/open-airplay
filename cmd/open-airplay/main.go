package main

import (
	"bufio"
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"os/signal"
	"runtime"
	"sort"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/parsakhaz/open-airplay/internal/airplay"
	"github.com/parsakhaz/open-airplay/internal/daemon"
)

func parseXID(s string) (uint64, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return 0, nil
	}
	return strconv.ParseUint(s, 0, 64) // accepts decimal or 0xhex
}

// parsePortRange parses a "min-max" string into inclusive port bounds.
// An empty string returns (0, 0, nil) meaning "let the OS pick".
func parsePortRange(s string) (int, int, error) {
	s = strings.TrimSpace(s)
	if s == "" {
		return 0, 0, nil
	}
	parts := strings.SplitN(s, "-", 2)
	if len(parts) != 2 {
		return 0, 0, fmt.Errorf("expected MIN-MAX, got %q", s)
	}
	lo, err := strconv.Atoi(strings.TrimSpace(parts[0]))
	if err != nil {
		return 0, 0, fmt.Errorf("min: %w", err)
	}
	hi, err := strconv.Atoi(strings.TrimSpace(parts[1]))
	if err != nil {
		return 0, 0, fmt.Errorf("max: %w", err)
	}
	if lo < 1 || hi > 65535 || lo > hi {
		return 0, 0, fmt.Errorf("range %d-%d out of bounds (1-65535, min<=max)", lo, hi)
	}
	if hi-lo+1 < 3 {
		return 0, 0, fmt.Errorf("range %d-%d too small; need 3 consecutive UDP ports", lo, hi)
	}
	return lo, hi, nil
}

func defaultCredentialBackend() string {
	if runtime.GOOS == "windows" {
		return "keyring"
	}
	return "file"
}

func hwAccelFlagHelp() string {
	if runtime.GOOS == "windows" {
		return "Encoder: auto, nvenc, or none (libx264); Windows capture currently supports H.264 only"
	}
	return "Encoder: auto, nvenc, vaapi, openh264, or none (x264/x265)"
}

func videoCodecFlagHelp() string {
	if runtime.GOOS == "windows" {
		return "Screen codec: auto or h264 (Windows Desktop Duplication currently supports H.264 only)"
	}
	return "Screen codec: auto, h264, or hevc (auto uses H.264 for receiver compatibility)"
}

func validateWindowsCaptureFlags(hwaccel, videoCodec string) error {
	switch hwaccel {
	case "", "auto", "none", "nvenc":
	default:
		return fmt.Errorf("encoder %q is not supported by Windows capture (want auto, nvenc, or none)", hwaccel)
	}
	switch videoCodec {
	case "", "auto", "h264":
	default:
		return fmt.Errorf("video codec %q is not supported by Windows capture (want auto or h264)", videoCodec)
	}
	return nil
}

func validateVideoRateFlags(fps, bitrate int) error {
	if fps < 0 {
		return fmt.Errorf("-fps must be 0 (auto) or a positive value")
	}
	if bitrate < 0 {
		return fmt.Errorf("-bitrate must be 0 (auto) or a positive value")
	}
	return nil
}

func main() {
	target := flag.String("target", "", "AirPlay receiver IP address or hostname (skip discovery)")
	receiver := flag.String("receiver", "", "Exact receiver name or 1-based index from -list-receivers (discovery mode only)")
	port := flag.Int("port", 7000, "AirPlay port")
	code := flag.String("code", "", "Pairing PIN shown on the receiver, or the password set on it when \"Require Password\" is enabled; prefer $OPEN_AIRPLAY_CODE so it stays out of shell history and process listings")
	credFile := flag.String("creds", airplay.DefaultCredentialsPath(), "Path to saved pairing credentials")
	credBackend := flag.String("cred-backend", defaultCredentialBackend(), "Credential storage backend: file or keyring (Windows Credential Manager or the platform keyring)")
	forcePair := flag.Bool("pair", false, "Force new pairing even if credentials exist")
	fps := flag.Int("fps", 0, "Frames per second (0 = auto, up to receiver-advertised 60 fps)")
	bitrate := flag.Int("bitrate", 0, "Video bitrate in kbps (0 = auto, default tunes for resolution/FPS)")
	targetLatencyMs := flag.Int("target-latency-ms", 0, "Joint audio/video playout latency override in milliseconds (0 = automatic connection profile)")
	hwaccel := flag.String("hwaccel", "auto", hwAccelFlagHelp())
	videoCodec := flag.String("video-codec", "auto", videoCodecFlagHelp())
	testMode := flag.Bool("test", false, "Use synthetic video (videotestsrc) instead of screen capture for debugging")
	noEncrypt := flag.Bool("no-encrypt", false, "Disable RTSP header encryption (debugging only; video frames are always encrypted)")
	directKey := flag.Bool("direct-key", false, "Use shk/shiv directly without SHA-512 derivation")
	noAudio := flag.Bool("no-audio", runtime.GOOS == "windows", "Disable audio streaming (default true on Windows until WASAPI capture is implemented)")
	portRange := flag.String("port-range", "", "Local UDP port range for receiver timing/audio (e.g. \"60000-60010\"); empty = OS ephemeral. Needs at least 3 ports.")
	debug := flag.Bool("debug", false, "Enable verbose debug logging")
	daemonize := flag.Bool("daemonize", false, "Run as background daemon with Unix socket control interface (unsupported on Windows)")
	socketPath := flag.String("socket", daemon.DefaultSocketPath(), "Unix socket path for daemon control interface (non-Windows only)")
	x11WindowID := flag.String("x11-window-id", "", "X11 window id to capture, decimal or 0xhex (non-Windows only)")
	x11WindowName := flag.String("x11-window-name", "", "X11 window name to capture; prefer -x11-window-id (non-Windows only)")
	displayIndex := flag.Int("display", -1, "Windows display index to capture (omit for picker)")
	listDisplays := flag.Bool("list-displays", false, "List active Windows displays and exit")
	listReceivers := flag.Bool("list-receivers", false, "Discover all AirPlay receivers for five seconds, show mirroring capability, and exit")
	captureSmoke := flag.Bool("capture-smoke", false, "Validate display capture through SPS/PPS/IDR and exit without connecting")
	noCursor := flag.Bool("no-cursor", false, "Don't show the mouse cursor in the captured video")
	flag.Parse()
	consoleReader := bufio.NewReader(os.Stdin)
	if *target != "" && *receiver != "" {
		log.Fatal("-target and -receiver are mutually exclusive")
	}
	if runtime.GOOS == "windows" && *daemonize {
		log.Fatal("-daemonize is not supported on Windows; run the sender in the foreground")
	}
	if runtime.GOOS == "windows" && !*noAudio && !*listDisplays && !*listReceivers && !*captureSmoke {
		log.Fatal("Windows audio capture is not implemented; keep -no-audio enabled")
	}
	if err := airplay.ValidateHWAccel(*hwaccel); err != nil {
		log.Fatalf("invalid -hwaccel: %v", err)
	}
	if err := airplay.ValidateVideoCodec(*videoCodec); err != nil {
		log.Fatalf("invalid -video-codec: %v", err)
	}
	if err := validateVideoRateFlags(*fps, *bitrate); err != nil {
		log.Fatalf("invalid video rate: %v", err)
	}
	if runtime.GOOS == "windows" {
		if err := validateWindowsCaptureFlags(*hwaccel, *videoCodec); err != nil {
			log.Fatalf("invalid Windows capture options: %v", err)
		}
	}
	portMin, portMax, err := parsePortRange(*portRange)
	if err != nil {
		log.Fatalf("invalid -port-range: %v", err)
	}
	if *listReceivers {
		discoverCtx, cancelDiscover := context.WithTimeout(context.Background(), 5*time.Second)
		defer cancelDiscover()
		devices, err := airplay.DiscoverAirPlayDevices(discoverCtx)
		if err != nil {
			log.Fatalf("discover receivers: %v", err)
		}
		sortAirPlayDevices(devices)
		if len(devices) == 0 {
			fmt.Println("no AirPlay receivers found")
			return
		}
		printReceivers(os.Stdout, devices)
		return
	}

	selectedDisplay := *displayIndex
	sourceWidth, sourceHeight := 0, 0
	if *listDisplays {
		displays, err := airplay.ListDisplays()
		if err != nil {
			log.Fatalf("list displays: %v", err)
		}
		printDisplays(os.Stdout, displays)
		return
	}
	if runtime.GOOS == "windows" && !*testMode {
		displays, err := airplay.ListDisplays()
		if err != nil {
			log.Fatalf("enumerate displays: %v", err)
		}
		selectedDisplay, err = selectDisplay(
			displays,
			selectedDisplay,
			consoleReader,
			os.Stdout,
		)
		if err != nil {
			log.Fatalf("select display: %v", err)
		}
		for _, display := range displays {
			if display.Index == selectedDisplay {
				sourceWidth, sourceHeight = display.Width, display.Height
				break
			}
		}
	} else if selectedDisplay < 0 {
		selectedDisplay = 0
	}
	if *captureSmoke {
		smokeCtx, cancelSmoke := context.WithTimeout(context.Background(), 20*time.Second)
		defer cancelSmoke()
		smokeFPS := *fps
		if smokeFPS == 0 {
			smokeFPS = 60
		}
		smokeWidth, smokeHeight := sourceWidth, sourceHeight
		if smokeWidth <= 0 || smokeHeight <= 0 {
			smokeWidth, smokeHeight = 1920, 1080
		}
		cfg := airplay.CaptureConfig{
			FPS:          smokeFPS,
			Bitrate:      *bitrate,
			HWAccel:      *hwaccel,
			VideoCodec:   airplay.VideoCodecH264,
			MaxWidth:     smokeWidth,
			MaxHeight:    smokeHeight,
			DisplayIndex: selectedDisplay,
			ShowCursor:   !*noCursor,
		}
		capture, err := airplay.StartCapture(smokeCtx, cfg)
		if err != nil {
			log.Fatalf("capture smoke test failed: %v", err)
		}
		capture.Stop()
		fmt.Printf("capture smoke test passed for display %d: Desktop Duplication produced H.264 SPS/PPS/IDR\n", selectedDisplay)
		return
	}
	// The environment wins over the flag: it keeps the code out of shell history
	// and out of `ps`, where a command line is readable by other users.
	credential := *code
	if env := os.Getenv("OPEN_AIRPLAY_CODE"); env != "" {
		credential = env
	} else if env := os.Getenv("DOUBLETAKE_CODE"); env != "" {
		// Compatibility with credentials supplied to the imported upstream CLI.
		credential = env
	}

	airplay.SetTargetLatency(time.Duration(*targetLatencyMs) * time.Millisecond)

	airplay.SetDebugMode(*debug)

	if *daemonize {
		runDaemon(daemon.Config{
			SocketPath:   *socketPath,
			CredFile:     *credFile,
			CredBackend:  *credBackend,
			FPS:          *fps,
			Bitrate:      *bitrate,
			PortMin:      portMin,
			PortMax:      portMax,
			HWAccel:      *hwaccel,
			VideoCodec:   airplay.VideoCodec(*videoCodec),
			Debug:        *debug,
			TestMode:     *testMode,
			NoEncrypt:    *noEncrypt,
			DirectKey:    *directKey,
			NoAudio:      *noAudio,
			ShowCursor:   !*noCursor,
			DisplayIndex: selectedDisplay,
			Code:         credential,
		})
		return
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	xid, err := parseXID(*x11WindowID)
	if err != nil {
		log.Fatalf("invalid -x11-window-id: %v", err)
	}

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sigCh
		log.Println("shutting down...")
		cancel()
		// Give goroutines a moment to clean up, then force exit
		go func() {
			time.Sleep(3 * time.Second)
			log.Println("forced exit (timeout)")
			os.Exit(1)
		}()
		// Also force exit on second signal
		<-sigCh
		log.Println("forced exit")
		os.Exit(1)
	}()

	var addr string
	var advertisement *airplay.AirPlayDevice
	if *target != "" {
		addr = *target
		fmt.Printf("direct target: endpoint=%s (receiver identity will be verified after connection)\n",
			net.JoinHostPort(addr, strconv.Itoa(*port)))
	} else {
		device, err := selectDevice(ctx, *receiver, consoleReader, os.Stdout)
		if err != nil {
			log.Fatalf("discovery failed: %v", err)
		}
		addr = device.IP
		*port = device.Port
		advertisement = device
		fmt.Printf("selected receiver: name=%s model=%s endpoint=%s\n",
			strconv.Quote(device.Name), strconv.Quote(device.Model),
			net.JoinHostPort(device.IP, strconv.Itoa(device.Port)))
	}

	newClient := func() *airplay.AirPlayClient {
		if advertisement != nil {
			device := *advertisement
			device.IP = addr
			device.Port = *port
			return airplay.NewAirPlayClientForDevice(device)
		}
		return airplay.NewAirPlayClient(addr, *port)
	}
	client := newClient()
	client.SetPassword(credential)
	if err := client.Connect(ctx); err != nil {
		log.Fatalf("connect failed: %v", err)
	}
	defer func() { _ = client.Close() }()

	info, err := client.GetInfo()
	if err != nil {
		log.Fatalf("get info failed: %v", err)
	}
	log.Printf("connected to: %q (model: %q, initialVolume: %.1f)", info.Name, info.Model, info.InitialVolume)
	if credential == "" && info.RequiresPassword() {
		credential = readCredential(consoleReader, fmt.Sprintf(
			"\nPASSWORD REQUIRED for %s.\nEnter its configured password: ",
			receiverPromptLabel(info.Name),
		))
		if credential == "" {
			log.Fatal("password cannot be empty")
		}
		client.SetPassword(credential)
	}

	// Pairing flow:
	// 1. If --pair is forced, do full pair-setup and save credentials.
	// 2. If saved credentials exist, load them and do pair-verify only.
	// 3. Pair directly when a legacy receiver requires its configured password,
	//    or when the receiver advertises one-time PIN pairing.
	// 4. Otherwise, try transient pairing and use a fresh connection for fallback.
	//
	// -code alone does not force pairing. A receiver with "Require Password"
	// needs the code on every session, so treating its presence as "pair again"
	// would re-pair on every run and throw away working credentials. When a
	// pairing PIN is what is actually wanted, -pair asks for one.
	needFullPair := *forcePair

	credStore, err := newCredentialStore(*credBackend, *credFile)
	if err != nil {
		log.Fatalf("failed to load credentials: %v", err)
	}

	var savedCreds *airplay.SavedCredentials
	if !needFullPair {
		savedCreds = credStore.Lookup(info.DeviceID)
	}

	reconnect := func() {
		_ = client.Close()
		client = newClient()
		client.SetPassword(credential)
		if err := client.Connect(ctx); err != nil {
			log.Fatalf("reconnect failed: %v", err)
		}
		var err error
		info, err = client.GetInfo()
		if err != nil {
			log.Fatalf("get info after reconnect failed: %v", err)
		}
	}

	savePairingCredentials := func() {
		if client.PairKeys == nil {
			return
		}
		if err := credStore.SavePairing(info.DeviceID, client.PairingID,
			client.PairKeys.Ed25519Public, client.PairKeys.Ed25519Private,
			client.PairingProtocol()); err != nil {
			log.Printf("warning: failed to save credentials: %v", err)
		} else {
			log.Printf("credentials saved (%s)", *credBackend)
		}
	}
	pairWithCredential := func(value string, expectPIN bool) {
		if value == "" {
			value = credentialOrPrompt(credential, client, expectPIN, info.Name, consoleReader)
		}
		// AirPlay uses the same user-entered value for pair-setup and, on
		// password-protected receivers, the later HTTP Digest challenge.
		// Keep it for reconnects as well as the current connection.
		credential = value
		if err := client.Pair(ctx, value); err != nil {
			log.Fatalf("pairing failed: %v", err)
		}
		savePairingCredentials()
	}
	if needFullPair {
		if forcePairUsesTransient(info) {
			// A modern receiver's fixed playback password belongs only to HTTP
			// Digest. Passing it to full SRP pair-setup is rejected as a bad PIN,
			// even when -pair was requested. Keep the one entered value for Digest
			// and establish this session with transient HAP pairing.
			if err := client.Pair(ctx, ""); err != nil {
				log.Fatalf("transient pairing failed for password-protected receiver: %v", err)
			}
		} else {
			// Full pair-setup with a supplied PIN/password, or request an onscreen PIN.
			pairWithCredential(credential, !info.RequiresPassword())
		}
	} else if savedCreds != nil && savedCreds.HasPairingCredentials() {
		// Use saved credentials — pair-verify
		log.Printf("using saved credentials (%s)", *credBackend)
		verifyErr := restoreSavedPairing(client, savedCreds)
		if verifyErr == nil {
			verifyErr = client.PairVerify(ctx)
		}
		if verifyErr != nil {
			log.Printf("pair-verify with saved creds failed: %v", verifyErr)
			reconnect()
			if passwordRequiresPairing(info) {
				pairWithCredential("", false)
			} else if info.RequiredPairingCredential() == airplay.PairingCredentialPIN {
				pairWithCredential("", true)
			} else if err := client.Pair(ctx, ""); err != nil {
				if info.RequiresPassword() {
					log.Fatalf("transient pairing failed for password-protected receiver: %v", err)
				}
				log.Printf("transient pairing fallback failed: %v, requesting pairing credentials", err)
				// A failed pairing exchange may leave receiver state on this socket.
				// Start and finish credential pairing together on a clean connection.
				reconnect()
				pairWithCredential("", false)
			}
		}
	} else {
		if passwordRequiresPairing(info) {
			pairWithCredential("", false)
		} else if info.RequiredPairingCredential() == airplay.PairingCredentialPIN {
			pairWithCredential("", true)
		} else if err := client.Pair(ctx, ""); err != nil {
			if info.RequiresPassword() {
				log.Fatalf("transient pairing failed for password-protected receiver: %v", err)
			}
			log.Printf("transient pairing failed: %v, requesting pairing credentials", err)
			reconnect()
			pairWithCredential("", false)
		}
	}
	log.Println("pairing complete")

	// FairPlay setup — establishes fp-setup state and ekey/eiv used for the
	// final encrypted mirror stream. Pair-verify and FairPlay are both needed
	// for compatibility with the normal modern flow.
	if client.FpEkey == nil {
		if err := client.FairPlaySetup(ctx); err != nil {
			if !errors.Is(err, airplay.ErrFairPlayUnsupported) {
				log.Fatalf("FairPlay setup failed: %v", err)
			}
			log.Printf("FairPlay SAP unsupported (%v); continuing with pair-verify DataStream setup", err)
		} else {
			log.Println("FairPlay setup complete")
		}
	}

	streamCfg := airplay.StreamConfig{
		FPS:          *fps,
		Bitrate:      *bitrate,
		VideoCodec:   airplay.VideoCodec(*videoCodec),
		SourceWidth:  sourceWidth,
		SourceHeight: sourceHeight,
		NoEncrypt:    *noEncrypt,
		DirectKey:    *directKey,
		NoAudio:      *noAudio,
		PortMin:      portMin,
		PortMax:      portMax,
	}
	// Complete the potentially interactive Wayland portal request before SETUP,
	// but delay encoder startup until control SETUP returns session-time display
	// information. Some current receivers omit displays before that session
	// exists; committing the pipeline earlier silently selects the 720p fallback.
	captureCfg := airplay.CaptureConfig{
		FPS:           *fps,
		Bitrate:       *bitrate,
		HWAccel:       *hwaccel,
		VideoCodec:    airplay.VideoCodec(*videoCodec),
		X11WindowID:   xid,
		X11WindowName: *x11WindowName,
		DisplayIndex:  selectedDisplay,
		ShowCursor:    !*noCursor,
	}
	if *testMode {
		if *noAudio {
			log.Println("using synthetic video (videotestsrc) for debugging")
		} else {
			log.Println("using synthetic video (videotestsrc) and audio test tone for debugging")
		}
	}
	prepareCapture := func(cfg airplay.CaptureConfig) (*airplay.CapturePreparation, error) {
		if *testMode {
			return airplay.PrepareTestCapture(ctx, cfg)
		}
		restoreToken := ""
		if creds := credStore.Lookup(info.DeviceID); creds != nil {
			restoreToken = creds.RestoreToken
		}
		cfg.RestoreToken = restoreToken
		cfg.SaveRestoreToken = func(token string) error {
			return credStore.SaveRestoreToken(info.DeviceID, token)
		}
		return airplay.PrepareCapture(ctx, cfg)
	}
	capturePreparation, err := prepareCapture(captureCfg)
	if err != nil {
		log.Fatalf("prepare screen capture: %v", err)
	}
	streamCfg.AutomaticHEVCAvailable = capturePreparation.AutomaticHEVCAvailable()
	streamCfg.MeasuredVideoLatency = capturePreparation.MeasuredVideoLatency()

	var capture *airplay.ScreenCapture
	var broadcast *airplay.BroadcastCapture
	var broadcastDone chan error
	startedWidth, startedHeight, startedFPS := -1, -1, -1
	startedCodec := airplay.VideoCodec("")
	var liveVideoLead time.Duration
	prepareH264Fallback := func(cause error) error {
		fallbackCfg := captureCfg
		fallbackCfg.VideoCodec = airplay.VideoCodecH264
		replacement, fallbackErr := prepareCapture(fallbackCfg)
		if fallbackErr != nil {
			return fmt.Errorf("%v; prepare H.264 fallback: %w", cause, fallbackErr)
		}
		capturePreparation.Close()
		capturePreparation = replacement
		liveVideoLead = 0
		return fmt.Errorf("%w: %v", airplay.ErrAutomaticVideoCodecUnavailable, cause)
	}
	prepareVideo := func(profile airplay.VideoProfile) (airplay.VideoPreparationResult, error) {
		width, height, fps, codec := profile.Width, profile.Height, profile.FPS, profile.Codec
		if capture != nil {
			if codec == startedCodec {
				if width != startedWidth || height != startedHeight || fps != startedFPS {
					log.Printf("receiver updated the %s profile to %dx%d@%dfps after capture started; keeping %dx%d@%dfps for this session", codec, width, height, fps, startedWidth, startedHeight, startedFPS)
				}
				return airplay.VideoPreparationResult{MinimumVideoLead: liveVideoLead}, nil
			}
			return airplay.VideoPreparationResult{}, fmt.Errorf("receiver changed video from %s %dx%d@%dfps to %s %dx%d@%dfps during setup", startedCodec, startedWidth, startedHeight, startedFPS, codec, width, height, fps)
		}
		startedCapture, startErr := capturePreparation.StartWithProfile(profile)
		if startErr != nil {
			if captureCfg.VideoCodec == airplay.VideoCodecAuto && codec == airplay.VideoCodecHEVC {
				return airplay.VideoPreparationResult{}, prepareH264Fallback(startErr)
			}
			return airplay.VideoPreparationResult{}, startErr
		}
		if codec == airplay.VideoCodecHEVC && !airplay.HasExplicitTargetLatency() {
			liveVideoLead, startErr = airplay.MeasureVideoCaptureLatency(ctx, startedCapture, fps)
			if startErr != nil {
				startedCapture.Stop()
				measurementErr := fmt.Errorf("measure production HEVC timing: %w", startErr)
				if captureCfg.VideoCodec == airplay.VideoCodecAuto {
					return airplay.VideoPreparationResult{}, prepareH264Fallback(measurementErr)
				}
				return airplay.VideoPreparationResult{}, measurementErr
			}
			log.Printf("[CAPTURE] production HEVC timing requires at least %v video lead", liveVideoLead)
		}
		activeBroadcast := airplay.NewBroadcastCaptureWithFrameRate(startedCapture, fps)
		done := make(chan error, 1)
		capture = startedCapture
		broadcast = activeBroadcast
		broadcastDone = done
		startedWidth, startedHeight, startedFPS = width, height, fps
		startedCodec = codec
		go func(active *airplay.BroadcastCapture, result chan<- error) {
			result <- active.Run()
		}(activeBroadcast, done)
		log.Printf("screen capture started at %dx%d@%dfps using %s", width, height, fps, codec)
		return airplay.VideoPreparationResult{MinimumVideoLead: liveVideoLead}, nil
	}
	defer func() {
		capturePreparation.Close()
		if capture != nil {
			capture.Stop()
			<-broadcastDone
		}
	}()

	session, err := client.SetupMirrorWithVideoProfilePreparation(ctx, streamCfg, prepareVideo)
	if errors.Is(err, airplay.ErrCredentialsRequired) {
		// Some legacy receivers do not advertise their configured password in
		// /info. They reveal it only by challenging the first media SETUP. Keep
		// the completed pairing/FairPlay state and running capture, configure the
		// cached Digest challenge, and retry this setup exactly once.
		credential = readCredential(consoleReader, fmt.Sprintf(
			"\nPAIRING CREDENTIAL REQUIRED for %s.\nEnter the code shown there, or its configured password: ",
			receiverPromptLabel(info.Name),
		))
		if credential == "" {
			log.Fatal("receiver code/password cannot be empty")
		}
		client.SetPassword(credential)
		if startedCodec != "" {
			// A late Digest challenge may expose richer authenticated display
			// metadata on the retry. The running encoder is single-use, so pin
			// the already safe concrete codec for the remainder of this session.
			streamCfg.VideoCodec = startedCodec
		}
		session, err = client.SetupMirrorWithVideoProfilePreparation(ctx, streamCfg, prepareVideo)
	}
	if err != nil {
		log.Fatalf("mirror setup failed: %v", err)
	}
	if capture == nil || broadcast == nil {
		log.Fatal("mirror setup completed without preparing video capture")
	}
	defer session.Close()
	log.Printf("mirror session ready (data port: %d)", session.DataPort)

	go func() {
		<-ctx.Done()
		capture.Stop()
		session.Close()
	}()

	// Start audio capture and streaming unless disabled.
	if !*noAudio && session.HasAudio() {
		audioCapture, err := airplay.StartAudioCapture(ctx, *testMode, session.AudioCodec())
		if err != nil {
			log.Printf("warning: audio capture failed: %v (continuing without audio)", err)
		} else {
			defer audioCapture.Stop()
			go func() {
				if err := session.StreamAudio(ctx, audioCapture, session.AudioStream()); err != nil && ctx.Err() == nil {
					log.Printf("audio streaming error: %v", err)
				}
			}()
			log.Println("audio capture started")
		}
	} else if !*noAudio {
		log.Println("audio disabled (receiver did not provide audio ports)")
	}

	// Keep the capture pipeline draining until the receiver is ready to consume
	// video. Holding its first access unit during setup can exhaust the portal
	// buffer pool. StreamFrames waits for the next parameter sets and keyframe.
	videoSink, err := broadcast.AddBackpressuredSink()
	if err != nil {
		log.Fatalf("attach video stream: %v", err)
	}
	defer videoSink.Close()
	if err := session.StreamFrames(ctx, videoSink.AsCapture(), 0*time.Second); err != nil && ctx.Err() == nil {
		log.Printf("streaming error: %v", err)
		return
	}
	log.Println("stream ended")
}

// credentialOrPrompt returns a supplied credential or makes one pairing-display
// request and prompts once. expectPIN is true only when the receiver advertised
// an on-screen PIN (or the user explicitly requested fresh pairing). A successful
// /pair-pin-start response alone cannot distinguish a PIN from a fixed password,
// so an unadvertised fallback keeps its prompt deliberately generic.
func credentialOrPrompt(
	value string,
	client *airplay.AirPlayClient,
	expectPIN bool,
	receiverName string,
	reader *bufio.Reader,
) string {
	if value != "" {
		return value
	}

	displayErr := client.StartPINDisplay()
	if displayErr != nil {
		log.Printf("warning: failed to trigger PIN display: %v", displayErr)
	}
	credential := readCredential(reader, pairingCredentialPrompt(receiverName, expectPIN, displayErr))
	if credential == "" {
		log.Fatal("pairing credential cannot be empty")
	}
	return credential
}

func receiverPromptLabel(name string) string {
	name = strings.TrimSpace(name)
	if name == "" {
		return "the selected receiver"
	}
	return strconv.Quote(name)
}

func pairingCredentialPrompt(receiverName string, expectPIN bool, displayErr error) string {
	label := receiverPromptLabel(receiverName)
	if expectPIN && displayErr == nil {
		return fmt.Sprintf("\nPAIRING REQUIRED: a PIN should now be visible on %s.\nEnter that PIN here: ", label)
	}
	return fmt.Sprintf("\nPAIRING CREDENTIAL REQUIRED for %s.\nEnter its configured password or displayed pairing PIN: ", label)
}

func passwordRequiresPairing(info *airplay.ReceiverInfo) bool {
	// A failed transient exchange can put a legacy password pairer into an error
	// state, so receivers advertising that pairing generation start with SRP.
	// Modern receivers retain the password only for Digest, even when their
	// status flags also advertise on-screen PIN pairing.
	return info.RequiredPairingCredential() == airplay.PairingCredentialPassword
}

func restoreSavedPairing(client *airplay.AirPlayClient, saved *airplay.SavedCredentials) error {
	return client.RestorePairingCredentials(saved)
}

func forcePairUsesTransient(info *airplay.ReceiverInfo) bool {
	return info != nil && info.RequiresPassword() &&
		info.RequiredPairingCredential() == airplay.PairingCredentialNone
}

func readCredential(reader *bufio.Reader, prompt string) string {
	fmt.Print(prompt)
	// Read the whole line rather than using fmt.Scanln, which stops at the
	// first space and would silently truncate a password containing one.
	line, err := reader.ReadString('\n')
	if err != nil && !errors.Is(err, io.EOF) {
		log.Fatalf("failed to read credential: %v", err)
	}
	line = strings.TrimRight(line, "\r\n")
	return line
}

func selectDevice(
	ctx context.Context,
	selector string,
	reader *bufio.Reader,
	writer io.Writer,
) (*airplay.AirPlayDevice, error) {
	fmt.Fprintln(writer, "searching for AirPlay receivers...")
	discoverCtx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()

	devices, err := airplay.DiscoverAirPlayDevices(discoverCtx)
	if err != nil {
		return nil, err
	}
	if len(devices) == 0 {
		return nil, fmt.Errorf("no AirPlay receivers found")
	}
	sortAirPlayDevices(devices)

	selector = strings.TrimSpace(selector)
	if selector == "" {
		fmt.Fprintln(writer, "\navailable receivers:")
		printReceivers(writer, devices)
		fmt.Fprint(writer, "\nSelect YOUR receiver by exact name or index (no default): ")
		line, readErr := reader.ReadString('\n')
		if readErr != nil && !errors.Is(readErr, io.EOF) {
			return nil, fmt.Errorf("read receiver selection: %w", readErr)
		}
		selector = strings.TrimSpace(line)
	}
	return selectReceiver(devices, selector)
}

func sortAirPlayDevices(devices []airplay.AirPlayDevice) {
	sort.SliceStable(devices, func(i, j int) bool {
		if comparison := compareIPs(devices[i].IP, devices[j].IP); comparison != 0 {
			return comparison < 0
		}
		if devices[i].Port != devices[j].Port {
			return devices[i].Port < devices[j].Port
		}
		return strings.ToLower(devices[i].Name) < strings.ToLower(devices[j].Name)
	})
}

func printReceivers(writer io.Writer, devices []airplay.AirPlayDevice) {
	for index, device := range devices {
		mirroring := "no"
		if device.SupportsScreen() {
			mirroring = "yes"
		}
		endpoint := net.JoinHostPort(device.IP, strconv.Itoa(device.Port))
		fmt.Fprintf(writer, "[%d] name=%s model=%s mirroring=%s endpoint=%s\n",
			index+1, strconv.Quote(device.Name), strconv.Quote(device.Model), mirroring, endpoint)
	}
}

func selectReceiver(devices []airplay.AirPlayDevice, selector string) (*airplay.AirPlayDevice, error) {
	selector = strings.TrimSpace(selector)
	if selector == "" {
		return nil, fmt.Errorf("receiver selection cannot be blank; choose your device by exact name or index")
	}

	// All-numeric selectors are always one-based list indices. This keeps a
	// copied discovery index deterministic even if a receiver is literally
	// named with digits.
	if index, err := strconv.Atoi(selector); err == nil {
		if index < 1 || index > len(devices) {
			return nil, fmt.Errorf("receiver index %d is outside the discovered range 1-%d", index, len(devices))
		}
		return requireMirroringReceiver(devices, index-1)
	}

	matches := make([]int, 0, 1)
	for index := range devices {
		if devices[index].Name == selector {
			matches = append(matches, index)
		}
	}
	if len(matches) > 1 {
		indices := make([]string, 0, len(matches))
		for _, index := range matches {
			indices = append(indices, strconv.Itoa(index+1))
		}
		return nil, fmt.Errorf("receiver name %q is ambiguous at indices %s; choose an index from the latest discovery list",
			selector, strings.Join(indices, ", "))
	}
	if len(matches) == 1 {
		return requireMirroringReceiver(devices, matches[0])
	}
	return nil, fmt.Errorf("receiver %q was not found; use an exact name or index from -list-receivers", selector)
}

func requireMirroringReceiver(devices []airplay.AirPlayDevice, index int) (*airplay.AirPlayDevice, error) {
	device := devices[index]
	if !device.SupportsScreen() {
		return nil, fmt.Errorf("receiver [%d] %q does not advertise screen-mirroring support", index+1, device.Name)
	}
	return &device, nil
}

func printDisplays(writer io.Writer, displays []airplay.Display) {
	for _, display := range displays {
		fmt.Fprintln(writer, display.String())
	}
}

func selectDisplay(displays []airplay.Display, requested int, reader *bufio.Reader, writer io.Writer) (int, error) {
	if len(displays) == 0 {
		return 0, fmt.Errorf("no active displays found")
	}
	find := func(index int) (int, bool) {
		for _, display := range displays {
			if display.Index == index {
				return display.Index, true
			}
		}
		return 0, false
	}
	if requested >= 0 {
		if index, ok := find(requested); ok {
			return index, nil
		}
		return 0, fmt.Errorf("display %d is not active", requested)
	}

	defaultIndex := displays[0].Index
	for _, display := range displays {
		if display.Primary {
			defaultIndex = display.Index
			break
		}
	}
	if len(displays) == 1 {
		fmt.Fprintf(writer, "using display %s\n", displays[0].String())
		return displays[0].Index, nil
	}

	fmt.Fprintln(writer, "available displays:")
	printDisplays(writer, displays)
	fmt.Fprintf(writer, "select display [%d]: ", defaultIndex)
	line, err := reader.ReadString('\n')
	if err != nil && !errors.Is(err, io.EOF) {
		return 0, fmt.Errorf("read display selection: %w", err)
	}
	line = strings.TrimSpace(line)
	if line == "" {
		return defaultIndex, nil
	}
	index, err := strconv.Atoi(line)
	if err != nil {
		return 0, fmt.Errorf("invalid display selection %q", line)
	}
	if index, ok := find(index); ok {
		return index, nil
	}
	return 0, fmt.Errorf("display %d is not active", index)
}

// compareIPs compares two IP address strings numerically.
func compareIPs(a, b string) int {
	ipA := net.ParseIP(a)
	ipB := net.ParseIP(b)
	if ipA == nil && ipB == nil {
		return strings.Compare(a, b)
	}
	if ipA == nil {
		return 1
	}
	if ipB == nil {
		return -1
	}
	aBytes := ipA.To16()
	bBytes := ipB.To16()
	for i := range aBytes {
		if aBytes[i] < bBytes[i] {
			return -1
		}
		if aBytes[i] > bBytes[i] {
			return 1
		}
	}
	return 0
}

func runDaemon(cfg daemon.Config) {
	d, err := daemon.New(cfg)
	if err != nil {
		log.Fatalf("[daemon] %v", err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sigCh
		log.Println("[daemon] shutting down...")
		cancel()
		d.Shutdown()
		<-sigCh
		log.Println("[daemon] forced exit")
		os.Exit(1)
	}()

	if err := d.Run(ctx); err != nil {
		log.Fatalf("[daemon] %v", err)
	}
}

func newCredentialStore(backend, filePath string) (*airplay.CredentialStore, error) {
	switch backend {
	case "keyring":
		kb, err := airplay.NewKeyringBackend()
		if err != nil {
			return nil, err
		}
		return airplay.NewCredentialStoreWithBackend(kb), nil
	case "file":
		return airplay.NewCredentialStore(filePath)
	default:
		return nil, fmt.Errorf("unknown credential backend %q (use \"file\" or \"keyring\")", backend)
	}
}
