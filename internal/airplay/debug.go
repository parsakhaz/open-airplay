package airplay

import (
	"fmt"
	"log"
	"strings"
	"sync/atomic"
)

var debugMode atomic.Bool

// SetDebugMode controls verbose AirPlay logging. It is safe to call while
// capture or receiver workers are active.
func SetDebugMode(enabled bool) {
	debugMode.Store(enabled)
}

// DebugMode reports whether verbose AirPlay logging is enabled.
func DebugMode() bool {
	return debugMode.Load()
}

// dbg logs a message only when debug mode is enabled.
func dbg(format string, args ...interface{}) {
	if DebugMode() {
		log.Printf(format, args...)
	}
}

// debugPlistValue returns a logging-only copy of a plist value. Binary data is
// summarized by length, and secret fields are redacted even when a receiver
// encodes them as strings or other scalar values.
func debugPlistValue(key string, value interface{}) interface{} {
	if secretPlistKey(key) {
		switch v := value.(type) {
		case []byte:
			return fmt.Sprintf("<redacted: %d bytes>", len(v))
		case plistData:
			return fmt.Sprintf("<redacted: %d bytes>", len(v))
		case string:
			return fmt.Sprintf("<redacted: %d bytes>", len(v))
		default:
			return "<redacted>"
		}
	}
	switch v := value.(type) {
	case []byte:
		return fmt.Sprintf("<%d bytes>", len(v))
	case plistData:
		return fmt.Sprintf("<%d bytes>", len(v))
	case map[string]interface{}:
		redacted := make(map[string]interface{}, len(v))
		for k, item := range v {
			redacted[k] = debugPlistValue(k, item)
		}
		return redacted
	case []interface{}:
		redacted := make([]interface{}, len(v))
		for i, item := range v {
			redacted[i] = debugPlistValue("", item)
		}
		return redacted
	case []map[string]interface{}:
		redacted := make([]interface{}, len(v))
		for i, item := range v {
			redacted[i] = debugPlistValue("", item)
		}
		return redacted
	default:
		return value
	}
}

func secretPlistKey(key string) bool {
	key = strings.ToLower(strings.NewReplacer("-", "", "_", "", " ", "").Replace(key))
	switch key {
	case "shk", "shiv", "ekey", "eiv", "key", "iv", "secret", "sharedsecret",
		"privatekey", "sessionkey", "pairingkey", "streamkey", "streamiv",
		"writekey", "readkey", "encwritekey", "encreadkey", "chachakey",
		"aeskey", "aesiv", "cipherkey", "cipheriv", "fpkey", "fpaeskey",
		"wrappingkey", "ikm", "inputkeymaterial", "password", "passcode", "pin",
		"code", "credential", "credentials", "ed25519seed":
		return true
	default:
		return false
	}
}
