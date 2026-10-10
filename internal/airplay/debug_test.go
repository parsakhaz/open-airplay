package airplay

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"log"
	"net"
	"reflect"
	"strings"
	"testing"
	"time"

	"golang.org/x/crypto/chacha20poly1305"
)

func captureDebugLog(t *testing.T) *bytes.Buffer {
	t.Helper()
	var output bytes.Buffer
	wasEnabled, writer, flags, prefix := DebugMode(), log.Writer(), log.Flags(), log.Prefix()
	log.SetOutput(&output)
	log.SetFlags(0)
	log.SetPrefix("")
	SetDebugMode(true)
	t.Cleanup(func() {
		SetDebugMode(wasEnabled)
		log.SetOutput(writer)
		log.SetFlags(flags)
		log.SetPrefix(prefix)
	})
	return &output
}

func TestDebugDumpPlistRedactsSecretsAndPreservesMetadata(t *testing.T) {
	output := captureDebugLog(t)
	secret := []byte("secret-stream-key")
	stream := map[string]interface{}{
		"type":               int64(110),
		"streamConnectionID": int64(42),
		"shk":                secret,
		"shiv":               "secret-stream-iv",
		"nested":             map[string]interface{}{"Shared_Secret": "secret-pairing-material", "mode": "AES-CTR"},
	}
	request := map[string]interface{}{
		"ekey":    []byte("secret-wrapped-key"),
		"eiv":     plistData("secret-fairplay-iv"),
		"streams": []interface{}{stream},
		"info":    map[string]interface{}{"width": int64(1920), "data": []byte("opaque-binary-material")},
	}
	debugDumpPlist("test request", request)
	got := output.String()
	for _, sensitive := range []string{
		"secret-stream-key", "secret-stream-iv", "secret-pairing-material",
		"secret-wrapped-key", "secret-fairplay-iv", "opaque-binary-material",
		hex.EncodeToString(secret[:8]), fmt.Sprint(secret),
	} {
		if strings.Contains(got, sensitive) {
			t.Fatalf("debug log contains secret material %q: %s", sensitive, got)
		}
	}
	for _, metadata := range []string{"type:110", "streamConnectionID:42", "mode:AES-CTR", "width:1920", "<redacted: 17 bytes>", "<22 bytes>"} {
		if !strings.Contains(got, metadata) {
			t.Errorf("debug log lost metadata %q: %s", metadata, got)
		}
	}
	if !bytes.Equal(stream["shk"].([]byte), secret) || stream["shiv"] != "secret-stream-iv" {
		t.Fatal("redaction modified the protocol request")
	}
	redacted := debugPlistValue("", request).(map[string]interface{})
	redacted["streams"].([]interface{})[0].(map[string]interface{})["type"] = int64(96)
	if stream["type"] != int64(110) {
		t.Fatal("logging copy shares a nested map with the protocol request")
	}
}

func TestDebugPlistValueRedactsScalarSecrets(t *testing.T) {
	for _, field := range []string{"shk", "shiv", "ekey", "eiv", "Shared-Secret", "AES_Key", "cipherIV", "encReadKey", "privateKey", "IKM", "password", "PIN", "credential", "ed25519_seed"} {
		for _, value := range []interface{}{[]byte("secret"), plistData("secret"), "secret", uint64(1234), nil} {
			got := fmt.Sprint(debugPlistValue(field, value))
			if !strings.HasPrefix(got, "<redacted") {
				t.Errorf("field %s with value type %T was not redacted: %s", field, value, got)
			}
		}
	}
	value := []map[string]interface{}{{"shk": "secret", "dataPort": int64(7000)}}
	want := []interface{}{map[string]interface{}{"shk": "<redacted: 6 bytes>", "dataPort": int64(7000)}}
	if got := debugPlistValue("streams", value); !reflect.DeepEqual(got, want) {
		t.Errorf("typed stream list = %v, want %v", got, want)
	}
}

func TestControlErrorLogsRedactKeysAndBodies(t *testing.T) {
	for _, encrypted := range []bool{false, true} {
		t.Run(fmt.Sprintf("encrypted=%t", encrypted), func(t *testing.T) {
			output := captureDebugLog(t)
			clientConn, receiverConn := net.Pipe()
			defer clientConn.Close()
			defer receiverConn.Close()
			body := []byte("secret-key-bearing-error-body")
			key := bytes.Repeat([]byte{0xab}, chacha20poly1305.KeySize)
			headerSecret := "secret-receiver-header"
			wire := append([]byte(fmt.Sprintf("RTSP/1.0 500 Error\r\nX-Receiver-Secret: %s\r\nContent-Length: %d\r\n\r\n", headerSecret, len(body))), body...)
			if encrypted {
				aead, err := chacha20poly1305.New(key)
				if err != nil {
					t.Fatal(err)
				}
				length := make([]byte, 2)
				binary.LittleEndian.PutUint16(length, uint16(len(wire)))
				wire = append(length, aead.Seal(nil, make([]byte, 12), wire, length)...)
			}
			go func() { _, _ = receiverConn.Write(wire) }()
			client := &AirPlayClient{conn: clientConn, encrypted: encrypted, encReadKey: key}
			_, _, err := client.readHTTPResponseWithTimeout(time.Second)
			var statusErr *HTTPStatusError
			if !errors.As(err, &statusErr) || statusErr.StatusCode != 500 {
				t.Fatalf("error = %v, want HTTP 500", err)
			}
			if !bytes.Equal(statusErr.Body, body) {
				t.Fatal("redaction modified the programmatic error body")
			}
			log.Printf("request failed: %v", err)
			got := output.String()
			for _, sensitive := range []string{string(body), headerSecret, hex.EncodeToString(body), hex.EncodeToString(key[:8])} {
				if strings.Contains(got, sensitive) {
					t.Errorf("control log contains sensitive material %q: %s", sensitive, got)
				}
			}
			if !strings.Contains(got, fmt.Sprintf("HTTP 500 (body: %d bytes)", len(body))) {
				t.Errorf("control log lost status/length metadata: %s", got)
			}
		})
	}
}
