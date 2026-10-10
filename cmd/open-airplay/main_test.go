package main

import (
	"bufio"
	"crypto/ed25519"
	"crypto/rand"
	"errors"
	"runtime"
	"strings"
	"testing"

	"github.com/parsakhaz/open-airplay/internal/airplay"
)

func TestDefaultCredentialBackend(t *testing.T) {
	want := "file"
	if runtime.GOOS == "windows" {
		want = "keyring"
	}
	if got := defaultCredentialBackend(); got != want {
		t.Fatalf("defaultCredentialBackend() = %q, want %q", got, want)
	}
}

func TestPrintReceiversIncludesMirroringCapability(t *testing.T) {
	devices := []airplay.AirPlayDevice{
		{Name: "Living Room", Model: "AppleTV14,1", IP: "192.0.2.10", Port: 7000, Features: airplay.FeatureScreen},
		{Name: "Speaker", Model: "AudioAccessory6,1", IP: "192.0.2.11", Port: 7000, Features: airplay.FeatureAudio},
	}
	var output strings.Builder
	printReceivers(&output, devices)
	for _, want := range []string{
		`[1] name="Living Room" model="AppleTV14,1" mirroring=yes endpoint=192.0.2.10:7000`,
		`[2] name="Speaker" model="AudioAccessory6,1" mirroring=no endpoint=192.0.2.11:7000`,
	} {
		if !strings.Contains(output.String(), want) {
			t.Fatalf("receiver list %q does not contain %q", output.String(), want)
		}
	}
}

func TestSelectReceiverRequiresExplicitMirroringTarget(t *testing.T) {
	devices := []airplay.AirPlayDevice{
		{Name: "Living Room", Features: airplay.FeatureScreen},
		{Name: "Speaker", Features: airplay.FeatureAudio},
		{Name: "Duplicate", Features: airplay.FeatureScreen},
		{Name: "Duplicate", Features: airplay.FeatureScreen},
		{Name: "1", Features: airplay.FeatureScreen},
	}

	for _, test := range []struct {
		name       string
		selector   string
		wantName   string
		wantErrSub string
	}{
		{name: "exact name", selector: "Living Room", wantName: "Living Room"},
		{name: "name requires exact case", selector: "living room", wantErrSub: "was not found"},
		{name: "numeric selector is always index", selector: "1", wantName: "Living Room"},
		{name: "blank has no default", wantErrSub: "cannot be blank"},
		{name: "audio only rejected", selector: "2", wantErrSub: "does not advertise screen-mirroring"},
		{name: "ambiguous name rejected", selector: "Duplicate", wantErrSub: "ambiguous"},
		{name: "missing name", selector: "Bedroom", wantErrSub: "was not found"},
		{name: "index out of range", selector: "9", wantErrSub: "outside the discovered range"},
	} {
		t.Run(test.name, func(t *testing.T) {
			got, err := selectReceiver(devices, test.selector)
			if test.wantErrSub != "" {
				if err == nil || !strings.Contains(err.Error(), test.wantErrSub) {
					t.Fatalf("selectReceiver(%q) error = %v, want substring %q", test.selector, err, test.wantErrSub)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if got.Name != test.wantName {
				t.Fatalf("selectReceiver(%q) = %q, want %q", test.selector, got.Name, test.wantName)
			}
		})
	}
}

func TestValidateWindowsCaptureFlags(t *testing.T) {
	for _, valid := range []struct{ hwaccel, codec string }{
		{"", ""}, {"auto", "auto"}, {"none", "h264"}, {"nvenc", "h264"},
	} {
		if err := validateWindowsCaptureFlags(valid.hwaccel, valid.codec); err != nil {
			t.Errorf("validateWindowsCaptureFlags(%q, %q): %v", valid.hwaccel, valid.codec, err)
		}
	}
	for _, invalid := range []struct{ hwaccel, codec string }{
		{"vaapi", "h264"}, {"openh264", "h264"}, {"auto", "hevc"},
	} {
		if err := validateWindowsCaptureFlags(invalid.hwaccel, invalid.codec); err == nil {
			t.Errorf("validateWindowsCaptureFlags(%q, %q) accepted unsupported options", invalid.hwaccel, invalid.codec)
		}
	}
}

func TestParsePortRange(t *testing.T) {
	for _, test := range []struct {
		name       string
		value      string
		wantMin    int
		wantMax    int
		wantErrSub string
	}{
		{name: "ephemeral"},
		{name: "valid", value: "60000-60010", wantMin: 60000, wantMax: 60010},
		{name: "whitespace", value: " 60000 - 60002 ", wantMin: 60000, wantMax: 60002},
		{name: "malformed", value: "60000", wantErrSub: "expected MIN-MAX"},
		{name: "minimum invalid", value: "zero-60010", wantErrSub: "min:"},
		{name: "maximum invalid", value: "60000-many", wantErrSub: "max:"},
		{name: "zero", value: "0-2", wantErrSub: "out of bounds"},
		{name: "maximum too high", value: "65534-65536", wantErrSub: "out of bounds"},
		{name: "reversed", value: "60010-60000", wantErrSub: "out of bounds"},
		{name: "too small", value: "60000-60001", wantErrSub: "too small"},
	} {
		t.Run(test.name, func(t *testing.T) {
			gotMin, gotMax, err := parsePortRange(test.value)
			if test.wantErrSub != "" {
				if err == nil || !strings.Contains(err.Error(), test.wantErrSub) {
					t.Fatalf("parsePortRange(%q) error = %v, want substring %q", test.value, err, test.wantErrSub)
				}
				return
			}
			if err != nil {
				t.Fatalf("parsePortRange(%q): %v", test.value, err)
			}
			if gotMin != test.wantMin || gotMax != test.wantMax {
				t.Fatalf("parsePortRange(%q) = %d-%d, want %d-%d", test.value, gotMin, gotMax, test.wantMin, test.wantMax)
			}
		})
	}
}

func TestSelectDisplay(t *testing.T) {
	displays := []airplay.Display{
		{Index: 0, Name: `\\.\DISPLAY1`, Width: 1920, Height: 1080},
		{Index: 1, Name: `\\.\DISPLAY2`, X: 1920, Width: 2560, Height: 1440, Primary: true},
	}

	for _, test := range []struct {
		name       string
		requested  int
		input      string
		want       int
		wantErrSub string
		wantOutput string
	}{
		{name: "explicit", requested: 0, want: 0},
		{name: "default primary", requested: -1, input: "\n", want: 1, wantOutput: "available displays:"},
		{name: "EOF defaults primary", requested: -1, want: 1},
		{name: "interactive choice", requested: -1, input: "0\n", want: 0},
		{name: "invalid integer", requested: -1, input: "first\n", wantErrSub: "invalid display selection"},
		{name: "inactive explicit", requested: 3, wantErrSub: "not active"},
	} {
		t.Run(test.name, func(t *testing.T) {
			var output strings.Builder
			got, err := selectDisplay(
				displays,
				test.requested,
				bufio.NewReader(strings.NewReader(test.input)),
				&output,
			)
			if test.wantErrSub != "" {
				if err == nil || !strings.Contains(err.Error(), test.wantErrSub) {
					t.Fatalf("selectDisplay() error = %v, want substring %q", err, test.wantErrSub)
				}
				return
			}
			if err != nil {
				t.Fatal(err)
			}
			if got != test.want {
				t.Fatalf("selectDisplay() = %d, want %d", got, test.want)
			}
			if test.wantOutput != "" && !strings.Contains(output.String(), test.wantOutput) {
				t.Fatalf("output %q does not contain %q", output.String(), test.wantOutput)
			}
		})
	}
}

func TestSelectDisplayUsesOnlyDisplayWithoutPrompt(t *testing.T) {
	display := airplay.Display{Index: 4, Name: `\\.\DISPLAY5`, Width: 1280, Height: 720, Primary: true}
	var output strings.Builder
	got, err := selectDisplay(
		[]airplay.Display{display},
		-1,
		bufio.NewReader(strings.NewReader("")),
		&output,
	)
	if err != nil {
		t.Fatal(err)
	}
	if got != 4 {
		t.Fatalf("selectDisplay() = %d, want 4", got)
	}
	if !strings.Contains(output.String(), "using display [4]") {
		t.Fatalf("single-display output = %q", output.String())
	}
}

func TestReadCredentialPreservesPasswordAndEmptyInput(t *testing.T) {
	reader := bufio.NewReader(strings.NewReader("password with spaces\n\n"))
	if got := readCredential(reader, ""); got != "password with spaces" {
		t.Fatalf("password = %q, want %q", got, "password with spaces")
	}
	if got := readCredential(reader, ""); got != "" {
		t.Fatalf("blank password choice = %q, want empty", got)
	}
}

func TestPairingCredentialPrompt(t *testing.T) {
	for _, test := range []struct {
		name       string
		expectPIN  bool
		displayErr error
		want       string
	}{
		{name: "advertised PIN displayed", expectPIN: true, want: "\nPAIRING REQUIRED: a PIN should now be visible on \"Living Room\".\nEnter that PIN here: "},
		{name: "advertised PIN display failed", expectPIN: true, displayErr: errors.New("display failed"), want: "\nPAIRING CREDENTIAL REQUIRED for \"Living Room\".\nEnter its configured password or displayed pairing PIN: "},
		{name: "unknown mode accepted display request", want: "\nPAIRING CREDENTIAL REQUIRED for \"Living Room\".\nEnter its configured password or displayed pairing PIN: "},
		{name: "unknown mode rejected display request", displayErr: errors.New("not supported"), want: "\nPAIRING CREDENTIAL REQUIRED for \"Living Room\".\nEnter its configured password or displayed pairing PIN: "},
	} {
		t.Run(test.name, func(t *testing.T) {
			if got := pairingCredentialPrompt("Living Room", test.expectPIN, test.displayErr); got != test.want {
				t.Fatalf("prompt = %q, want %q", got, test.want)
			}
		})
	}
}

func TestPasswordRequiresPairing(t *testing.T) {
	const (
		passwordFlag = uint64(1 << 7)
		pinFlag      = uint64(1 << 9)
	)
	modernFeatures := airplay.FeatureSystemPairing | airplay.FeatureTransientPairing | uint64(1<<38) | uint64(1<<46)
	legacyFeatures := modernFeatures | uint64(1<<51)

	for _, test := range []struct {
		name string
		info *airplay.ReceiverInfo
		want bool
	}{
		{name: "missing info"},
		{name: "legacy configured password", info: &airplay.ReceiverInfo{Features: legacyFeatures, StatusFlags: passwordFlag}, want: true},
		{name: "modern configured password can pair transiently", info: &airplay.ReceiverInfo{Features: modernFeatures, StatusFlags: passwordFlag}},
		{name: "modern password suppresses PIN but remains Digest only", info: &airplay.ReceiverInfo{Features: modernFeatures, StatusFlags: passwordFlag | pinFlag}},
		{name: "legacy password wins over PIN", info: &airplay.ReceiverInfo{Features: legacyFeatures, StatusFlags: passwordFlag | pinFlag}, want: true},
		{name: "legacy receiver without password", info: &airplay.ReceiverInfo{Features: legacyFeatures}},
	} {
		t.Run(test.name, func(t *testing.T) {
			if got := passwordRequiresPairing(test.info); got != test.want {
				t.Fatalf("passwordRequiresPairing() = %t, want %t", got, test.want)
			}
		})
	}
}

func TestForcePairDoesNotUseModernDigestPasswordAsSRPPIN(t *testing.T) {
	const passwordFlag = uint64(1 << 7)
	modernFeatures := airplay.FeatureSystemPairing | airplay.FeatureTransientPairing | uint64(1<<38) | uint64(1<<46)
	legacyFeatures := modernFeatures | uint64(1<<51)

	if !forcePairUsesTransient(&airplay.ReceiverInfo{Features: modernFeatures, StatusFlags: passwordFlag}) {
		t.Fatal("modern fixed-password receiver would reuse its Digest password as an SRP PIN")
	}
	if forcePairUsesTransient(&airplay.ReceiverInfo{Features: legacyFeatures, StatusFlags: passwordFlag}) {
		t.Fatal("legacy fixed-password receiver lost its authenticated SRP pairing path")
	}
	if forcePairUsesTransient(&airplay.ReceiverInfo{Features: modernFeatures}) {
		t.Fatal("unprotected modern receiver unexpectedly bypassed explicit full pairing")
	}
}

func TestRestoreSavedPairingRestoresRawProtocol(t *testing.T) {
	pub, priv, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatalf("GenerateKey: %v", err)
	}
	client := airplay.NewAirPlayClient("127.0.0.1", 7000)
	err = restoreSavedPairing(client, &airplay.SavedCredentials{
		PairingID:       "pair-id",
		Ed25519Public:   pub,
		Ed25519Seed:     priv.Seed(),
		PairingProtocol: airplay.PairingProtocolRaw,
	})
	if err != nil {
		t.Fatalf("restoreSavedPairing: %v", err)
	}
	if got := client.PairingProtocol(); got != airplay.PairingProtocolRaw {
		t.Fatalf("restored protocol = %q, want raw", got)
	}
}
