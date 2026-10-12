package daemon

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/parsakhaz/open-airplay/internal/airplay"
	"howett.net/plist"
)

// stdioHarness drives RunStdio through in-memory pipes, the way the desktop
// shell drives the bundled sender through the child's stdin and stdout.
type stdioHarness struct {
	t        *testing.T
	d        *Daemon
	in       *io.PipeWriter
	messages chan StdioMessage
	done     chan error
	// latest is the last state line read, even one skipped while waiting for
	// something else: a status that is already current gets no second line.
	latest *StdioMessage
}

func newStdioHarness(t *testing.T, d *Daemon) *stdioHarness {
	t.Helper()
	d.skipDiscovery = true
	inReader, inWriter := io.Pipe()
	outReader, outWriter := io.Pipe()
	h := &stdioHarness{
		t:        t,
		d:        d,
		in:       inWriter,
		messages: make(chan StdioMessage, 64),
		done:     make(chan error, 1),
	}
	go func() {
		err := d.RunStdio(context.Background(), inReader, outWriter)
		outWriter.Close()
		h.done <- err
	}()
	go func() {
		scanner := bufio.NewScanner(outReader)
		for scanner.Scan() {
			var msg StdioMessage
			if err := json.Unmarshal(scanner.Bytes(), &msg); err != nil {
				t.Errorf("stdout line is not JSON: %q (%v)", scanner.Text(), err)
				continue
			}
			h.messages <- msg
		}
		close(h.messages)
	}()
	t.Cleanup(func() {
		inWriter.Close()
		select {
		case <-h.done:
		case <-time.After(5 * time.Second):
			t.Error("RunStdio did not return after stdin closed")
		}
	})
	return h
}

func (h *stdioHarness) send(line string) {
	h.t.Helper()
	if _, err := io.WriteString(h.in, line+"\n"); err != nil {
		h.t.Fatalf("write request: %v", err)
	}
}

// next returns the first message that satisfies match, skipping others.
func (h *stdioHarness) next(what string, match func(StdioMessage) bool) StdioMessage {
	h.t.Helper()
	timeout := time.After(5 * time.Second)
	for {
		select {
		case msg, ok := <-h.messages:
			if !ok {
				h.t.Fatalf("stdout closed while waiting for %s", what)
			}
			if msg.Type == "state" {
				latest := msg
				h.latest = &latest
			}
			if match(msg) {
				return msg
			}
		case <-timeout:
			h.t.Fatalf("timed out waiting for %s", what)
		}
	}
}

func (h *stdioHarness) response(id uint64) StdioMessage {
	h.t.Helper()
	return h.next(fmt.Sprintf("response %d", id), func(m StdioMessage) bool {
		return m.Type == "response" && m.ID == id
	})
}

// state returns the current state line if it matches (it may have been read
// already, before a reply), else waits for the next state line that does.
func (h *stdioHarness) state(what string, match func(StdioMessage) bool) StdioMessage {
	h.t.Helper()
	if h.latest != nil && match(*h.latest) {
		return *h.latest
	}
	return h.next("state line: "+what, func(m StdioMessage) bool {
		return m.Type == "state" && match(m)
	})
}

func newStdioTestDaemon(t *testing.T) *Daemon {
	t.Helper()
	d, err := New(Config{
		CredBackend: "file",
		CredFile:    filepath.Join(t.TempDir(), "credentials.json"),
		NoAudio:     true,
	})
	if err != nil {
		t.Fatalf("create daemon: %v", err)
	}
	return d
}

func splitTestAddr(t *testing.T, addr net.Addr) (string, int) {
	t.Helper()
	host, portText, err := net.SplitHostPort(addr.String())
	if err != nil {
		t.Fatalf("split address: %v", err)
	}
	port, err := strconv.Atoi(portText)
	if err != nil {
		t.Fatalf("parse port: %v", err)
	}
	return host, port
}

func TestStdioStartsWithAnIdleStateLineAndAnswersRequestsByID(t *testing.T) {
	h := newStdioHarness(t, newStdioTestDaemon(t))

	hello := h.next("hello", func(StdioMessage) bool { return true })
	if hello.Type != "hello" || hello.Protocol != StdioProtocolVersion {
		t.Fatalf("first line = %+v, want hello with protocol %d", hello, StdioProtocolVersion)
	}
	h.state("initial idle", func(m StdioMessage) bool { return m.State == StateIdle })

	h.send(`{"id":7,"cmd":"status"}`)
	resp := h.response(7)
	if !resp.OK || resp.State != StateIdle {
		t.Fatalf("status response = %+v, want ok idle", resp)
	}

	h.send(`{"id":8,"cmd":"no-such-command"}`)
	resp = h.response(8)
	if resp.OK || !strings.Contains(resp.Error, "unknown command") {
		t.Fatalf("unknown command response = %+v", resp)
	}

	h.send(`not json`)
	resp = h.next("invalid request reply", func(m StdioMessage) bool { return m.Type == "response" })
	if resp.OK || !strings.Contains(resp.Error, "invalid request") {
		t.Fatalf("invalid request response = %+v", resp)
	}
}

func TestStdioPushesThePINPromptAndRelaysTheCode(t *testing.T) {
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	defer listener.Close()
	infoBody, err := plist.Marshal(map[string]any{
		"name":        "PIN TV",
		"model":       "AppleTV-Test",
		"deviceID":    "00:11:22:33:44:55",
		"features":    uint64(airplay.FeatureSystemPairing),
		"statusFlags": uint64(0x200),
	}, plist.BinaryFormat)
	if err != nil {
		t.Fatalf("encode /info response: %v", err)
	}
	requestSeen := make(chan pinFlowRequest, 3)
	serverDone := make(chan pinFlowServerResult, 1)
	go runPINFlowReceiver(listener, infoBody, requestSeen, serverDone)
	host, port := splitTestAddr(t, listener.Addr())

	h := newStdioHarness(t, newStdioTestDaemon(t))
	h.send(fmt.Sprintf(`{"id":1,"cmd":"connect","target":%q,"port":%d}`, host, port))
	if resp := h.response(1); !resp.OK {
		t.Fatalf("connect rejected: %+v", resp)
	}

	prompt := h.state("PIN prompt", func(m StdioMessage) bool { return m.State == StatePINRequired })
	if !prompt.NeedsPIN || prompt.Device != "PIN TV" || prompt.DeviceIP != host {
		t.Fatalf("PIN prompt line = %+v", prompt)
	}

	// The scripted receiver rejects pair-setup, as a TV does with a wrong code.
	h.send(fmt.Sprintf(`{"id":2,"cmd":"connect","target":%q,"pin":"1234"}`, host))
	if resp := h.response(2); !resp.OK {
		t.Fatalf("PIN submission rejected: %+v", resp)
	}
	failed := h.state("pairing failure", func(m StdioMessage) bool {
		return m.State == StateIdle && m.Error != ""
	})
	if !strings.Contains(failed.Error, "pin pairing") {
		t.Fatalf("failure line error = %q, want a pin pairing failure", failed.Error)
	}
	if len(failed.Errors) != 1 || failed.Errors[0].Target != host || failed.Errors[0].Kind != ErrorKindWrongCode || failed.Errors[0].Device != "PIN TV" {
		t.Fatalf("per-target errors = %+v, want one wrong_code for %s", failed.Errors, host)
	}
	if result := <-serverDone; result.err != nil {
		t.Fatalf("scripted receiver: %v", result.err)
	}
}

// silentReceiver accepts connections and never answers, so a connect stays in
// the connecting state until it is cancelled. It reports each accepted and
// each closed connection.
func silentReceiver(t *testing.T) (string, int, <-chan struct{}, <-chan struct{}) {
	t.Helper()
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	t.Cleanup(func() { listener.Close() })
	accepted := make(chan struct{}, 4)
	closed := make(chan struct{}, 4)
	go func() {
		for {
			conn, err := listener.Accept()
			if err != nil {
				return
			}
			accepted <- struct{}{}
			go func() {
				_, _ = io.Copy(io.Discard, conn)
				conn.Close()
				closed <- struct{}{}
			}()
		}
	}()
	host, port := splitTestAddr(t, listener.Addr())
	return host, port, accepted, closed
}

// waitAccepted waits for the daemon's TCP connection, which can take seconds
// on a Windows loopback. A cancel before then never opens a connection.
func waitAccepted(t *testing.T, accepted <-chan struct{}) {
	t.Helper()
	select {
	case <-accepted:
	case <-time.After(10 * time.Second):
		t.Fatal("daemon never connected to the receiver")
	}
}

func TestStdioDisconnectEndsAConnectingStream(t *testing.T) {
	host, port, accepted, closed := silentReceiver(t)
	h := newStdioHarness(t, newStdioTestDaemon(t))

	h.send(fmt.Sprintf(`{"id":1,"cmd":"connect","target":%q,"port":%d}`, host, port))
	if resp := h.response(1); !resp.OK {
		t.Fatalf("connect rejected: %+v", resp)
	}
	h.state("connecting", func(m StdioMessage) bool { return m.State == StateConnecting })
	waitAccepted(t, accepted)

	h.send(`{"id":2,"cmd":"disconnect"}`)
	if resp := h.response(2); !resp.OK || resp.State != StateIdle {
		t.Fatalf("disconnect response = %+v", resp)
	}
	h.state("idle after disconnect", func(m StdioMessage) bool { return m.State == StateIdle })
	select {
	case <-closed:
	case <-time.After(5 * time.Second):
		t.Fatal("receiver connection stayed open after disconnect")
	}
}

func TestStdioClosedInputTearsDownAndReturns(t *testing.T) {
	host, port, accepted, closed := silentReceiver(t)
	d := newStdioTestDaemon(t)
	d.skipDiscovery = true
	inReader, inWriter := io.Pipe()
	done := make(chan error, 1)
	go func() { done <- d.RunStdio(context.Background(), inReader, io.Discard) }()

	if _, err := fmt.Fprintf(inWriter, `{"cmd":"connect","target":%q,"port":%d}`+"\n", host, port); err != nil {
		t.Fatalf("write connect: %v", err)
	}
	pinFlowWaitForState(t, d, StateConnecting)
	waitAccepted(t, accepted)
	inWriter.Close()

	select {
	case err := <-done:
		if err != nil {
			t.Fatalf("RunStdio returned %v, want nil on closed input", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("RunStdio did not return after stdin closed")
	}
	select {
	case <-closed:
	case <-time.After(5 * time.Second):
		t.Fatal("receiver connection stayed open after stdin closed")
	}
}

func TestStdioListsDisplays(t *testing.T) {
	d := newStdioTestDaemon(t)
	d.listDisplays = func() ([]airplay.Display, error) {
		return []airplay.Display{
			{Index: 0, Name: `\\.\DISPLAY1`, Width: 2560, Height: 1440, Primary: true},
			{Index: 1, Name: `\\.\DISPLAY2`, X: 2560, Width: 1920, Height: 1080},
		}, nil
	}
	h := newStdioHarness(t, d)

	h.send(`{"id":3,"cmd":"displays"}`)
	resp := h.response(3)
	if !resp.OK || len(resp.Displays) != 2 {
		t.Fatalf("displays response = %+v", resp)
	}
	want := DisplayInfo{Index: 1, Name: `\\.\DISPLAY2`, X: 2560, Width: 1920, Height: 1080}
	if resp.Displays[1] != want || !resp.Displays[0].Primary {
		t.Fatalf("displays = %+v", resp.Displays)
	}

	d.listDisplays = func() ([]airplay.Display, error) { return nil, errors.New("no D3D11 device") }
	h.send(`{"id":4,"cmd":"displays"}`)
	if resp := h.response(4); resp.OK || !strings.Contains(resp.Error, "no D3D11 device") {
		t.Fatalf("failing displays response = %+v", resp)
	}
}

func TestStdioConnectRemembersTheChosenDisplay(t *testing.T) {
	host, port, _, _ := silentReceiver(t)
	d := newStdioTestDaemon(t)
	h := newStdioHarness(t, d)

	h.send(fmt.Sprintf(`{"id":1,"cmd":"connect","target":%q,"port":%d,"options":{"display":1}}`, host, port))
	if resp := h.response(1); !resp.OK {
		t.Fatalf("connect rejected: %+v", resp)
	}
	d.mu.Lock()
	entry := d.streams[host]
	d.mu.Unlock()
	if entry == nil || entry.settings.display != 1 {
		t.Fatalf("stream entry = %+v, want display 1", entry)
	}
}

func TestSourceSizeIsTheChosenDisplaysSize(t *testing.T) {
	d := newStdioTestDaemon(t)
	d.listDisplays = func() ([]airplay.Display, error) {
		return []airplay.Display{
			{Index: 0, Width: 1440, Height: 900, Primary: true},
			{Index: 1, Width: 2560, Height: 1440},
		}, nil
	}
	if w, h := d.sourceSize(1); w != 2560 || h != 1440 {
		t.Fatalf("sourceSize(1) = %dx%d, want 2560x1440", w, h)
	}
	if w, h := d.sourceSize(5); w != 0 || h != 0 {
		t.Fatalf("sourceSize(unknown) = %dx%d, want 0x0 (receiver's nominal canvas)", w, h)
	}
	d.listDisplays = func() ([]airplay.Display, error) { return nil, errors.New("not on Windows") }
	if w, h := d.sourceSize(0); w != 0 || h != 0 {
		t.Fatalf("sourceSize without displays = %dx%d, want 0x0", w, h)
	}
}

func TestStdioUnreachableTargetReportsItsOwnErrorKind(t *testing.T) {
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatalf("listen: %v", err)
	}
	host, port := splitTestAddr(t, listener.Addr())
	listener.Close() // nothing listens: the connect is refused

	h := newStdioHarness(t, newStdioTestDaemon(t))
	h.send(fmt.Sprintf(`{"id":1,"cmd":"connect","target":%q,"port":%d}`, host, port))
	if resp := h.response(1); !resp.OK {
		t.Fatalf("connect rejected: %+v", resp)
	}
	line := h.state("unreachable", func(m StdioMessage) bool { return len(m.Errors) > 0 })
	if line.Errors[0].Kind != ErrorKindUnreachable || line.Errors[0].Target != host || line.Errors[0].Message == "" {
		t.Fatalf("errors = %+v, want unreachable for %s", line.Errors, host)
	}

	// Retrying that target acknowledges its error.
	h.send(fmt.Sprintf(`{"id":2,"cmd":"connect","target":%q,"port":%d}`, host, port))
	resp := h.response(2)
	if !resp.OK || len(resp.Errors) != 0 {
		t.Fatalf("retry response = %+v, want ok with the old error cleared", resp)
	}
}

func TestStdioConnectAppliesTuningOptionsToThatTargetOnly(t *testing.T) {
	host, port, _, _ := silentReceiver(t)
	d := newStdioTestDaemon(t)
	h := newStdioHarness(t, d)

	h.send(fmt.Sprintf(`{"id":1,"cmd":"connect","target":%q,"port":%d,"options":{"display":1,"fps":30,"bitrate_kbps":8000,"latency_ms":250,"video_codec":"h264","hwaccel":"none","show_cursor":false,"audio":false}}`, host, port))
	if resp := h.response(1); !resp.OK {
		t.Fatalf("connect rejected: %+v", resp)
	}
	d.mu.Lock()
	entry := d.streams[host]
	d.mu.Unlock()
	want := mirrorSettings{
		display:    1,
		fps:        30,
		bitrate:    8000,
		latency:    250 * time.Millisecond,
		videoCodec: airplay.VideoCodecH264,
		hwaccel:    "none",
		showCursor: false,
		noAudio:    true,
	}
	if entry == nil || entry.settings != want {
		t.Fatalf("stream settings = %+v, want %+v", entry, want)
	}
}

func TestOmittedOptionsKeepTheSendersDefaults(t *testing.T) {
	d, err := New(Config{
		CredBackend: "file",
		CredFile:    filepath.Join(t.TempDir(), "credentials.json"),
		FPS:         60,
		Bitrate:     6000,
		HWAccel:     "nvenc",
		ShowCursor:  true,
		NoAudio:     true,
	})
	if err != nil {
		t.Fatalf("create daemon: %v", err)
	}
	want := mirrorSettings{fps: 60, bitrate: 6000, videoCodec: airplay.VideoCodecAuto, hwaccel: "nvenc", showCursor: true, noAudio: true}
	if got := d.mirrorSettingsFor(nil); got != want {
		t.Fatalf("settings without options = %+v, want %+v", got, want)
	}
	if got := d.mirrorSettingsFor(&MirrorOptions{VideoCodec: "auto", HWAccel: "auto"}); got.videoCodec != airplay.VideoCodecAuto || got.hwaccel != "auto" {
		t.Fatalf("explicit auto = %+v, want codec and encoder auto", got)
	}
}

func TestStdioRejectsOutOfRangeOptions(t *testing.T) {
	h := newStdioHarness(t, newStdioTestDaemon(t))
	for i, options := range []string{
		`{"fps":-1}`,
		`{"fps":500}`,
		`{"bitrate_kbps":-5}`,
		`{"latency_ms":2}`,
		`{"latency_ms":5000}`,
		`{"video_codec":"vp9"}`,
		`{"hwaccel":"quantum"}`,
	} {
		id := i + 1
		h.send(fmt.Sprintf(`{"id":%d,"cmd":"connect","target":"127.0.0.1","options":%s}`, id, options))
		if resp := h.response(uint64(id)); resp.OK || resp.ErrorKind != ErrorKindInvalidRequest {
			t.Errorf("options %s: response = %+v, want invalid_request", options, resp)
		}
	}
}

func TestStdioHelloAnnouncesWhetherSoundCanBeSent(t *testing.T) {
	h := newStdioHarness(t, newStdioTestDaemon(t))
	hello := h.next("hello", func(m StdioMessage) bool { return m.Type == "hello" })
	if hello.Capabilities == nil || hello.Capabilities.Audio != airplay.SystemAudioCaptureAvailable() {
		t.Fatalf("hello capabilities = %+v, want audio = %v", hello.Capabilities, airplay.SystemAudioCaptureAvailable())
	}
}

func TestStdioRefusesSoundWithoutAudioCapture(t *testing.T) {
	if airplay.SystemAudioCaptureAvailable() {
		t.Skip("this build can capture system audio")
	}
	h := newStdioHarness(t, newStdioTestDaemon(t))
	h.send(`{"id":1,"cmd":"connect","target":"127.0.0.1","options":{"audio":true}}`)
	if resp := h.response(1); resp.OK || resp.ErrorKind != ErrorKindUnsupportedOption || !strings.Contains(resp.Error, "audio") {
		t.Fatalf("response = %+v, want unsupported_option naming audio", resp)
	}
}

func TestStdioRejectsOptionsItCannotHonourYet(t *testing.T) {
	h := newStdioHarness(t, newStdioTestDaemon(t))
	h.send(`{"id":1,"cmd":"connect","target":"127.0.0.1","options":{"display":-3}}`)
	if resp := h.response(1); resp.OK || resp.ErrorKind != ErrorKindInvalidRequest {
		t.Fatalf("negative display response = %+v, want invalid_request", resp)
	}
	if runtime.GOOS != "windows" {
		return
	}
	// Windows Desktop Duplication encodes H.264 only, with nvenc or libx264.
	for i, options := range []string{`{"video_codec":"hevc"}`, `{"hwaccel":"vaapi"}`} {
		id := i + 2
		h.send(fmt.Sprintf(`{"id":%d,"cmd":"connect","target":"127.0.0.1","options":%s}`, id, options))
		if resp := h.response(uint64(id)); resp.OK || resp.ErrorKind != ErrorKindUnsupportedOption {
			t.Errorf("options %s: response = %+v, want unsupported_option", options, resp)
		}
	}
}

// "Require Password" on a modern receiver is HTTP Digest: a wrong password is
// refused with a second 401, which must read as a wrong code so the app asks again.
func TestStdioWrongDigestPasswordIsAWrongCode(t *testing.T) {
	server, err := airplay.NewReceiverServer(airplay.ReceiverConfig{
		ListenAddress: "127.0.0.1:0",
		Profile:       airplay.ReceiverProfileModern,
		Auth:          airplay.ReceiverAuthDigest,
		Code:          "hunter2",
		Logger:        log.New(io.Discard, "", 0),
	})
	if err != nil {
		t.Fatalf("create receiver: %v", err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	done := make(chan error, 1)
	go func() { done <- server.Serve(ctx) }()
	t.Cleanup(func() { cancel(); _ = server.Close(); <-done })
	host, port := splitTestAddr(t, server.Addr())

	h := newStdioHarness(t, newStdioTestDaemon(t))
	h.send(fmt.Sprintf(`{"id":1,"cmd":"connect","target":%q,"port":%d}`, host, port))
	if resp := h.response(1); !resp.OK {
		t.Fatalf("connect rejected: %+v", resp)
	}
	prompt := h.state("password prompt", func(m StdioMessage) bool { return m.State == StatePINRequired })
	if prompt.CredentialKind != CredentialKindPassword {
		t.Fatalf("prompt = %+v, want a password prompt", prompt)
	}
	h.send(fmt.Sprintf(`{"id":2,"cmd":"connect","target":%q,"pin":"wrong"}`, host))
	h.response(2)
	failed := h.state("failure", func(m StdioMessage) bool { return len(m.Errors) > 0 })
	if failed.Errors[0].Kind != ErrorKindWrongCode {
		t.Fatalf("errors = %+v, want wrong_code", failed.Errors)
	}
}
