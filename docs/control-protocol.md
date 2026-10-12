# Sender control protocol (stdio, version 1)

The desktop app runs the Go mirror sender as a child process and talks to it over stdin and stdout:

```
open-airplay-sender -stdio [-cred-backend keyring] [-debug]
```

Every line in either direction is one JSON object. Logs go to stderr, never stdout. Closing stdin ends every stream cleanly, and then the process exits with code 0.

The same requests also work on the Unix-socket daemon (`-daemonize`, Linux and macOS). The socket takes one request per connection and ignores `id`. The socket has no `hello` or `state` lines.

Source of truth: `internal/daemon/daemon.go` (types) and `internal/daemon/daemon_stdio.go` (framing). Tests: `internal/daemon/daemon_stdio_test.go`.

## Lines from the sender

| `type` | When | Carries |
|---|---|---|
| `hello` | First line, once | `protocol`: `1`, `capabilities` (below) |
| `state` | Right after `hello`, then whenever the status changes (checked at most every 100 ms and after every request) | A status (below) |
| `response` | One per request, in request order | `id` of the request, then a status or a command result |

`capabilities` says which optional features this build has, so a client can hide a control instead of trying it:

| Field | Meaning |
|---|---|
| `audio` | `true` when `connect` honours `options.audio: true`, that is, this build can capture the computer's own sound. While it is `false`, hide the Sound control; `audio: true` is refused with `unsupported_option`. |

Example: `{"type":"hello","protocol":1,"capabilities":{"audio":false}}`. Clients must ignore capability fields they don't know. Go: `airplay.SystemAudioCaptureAvailable()`.

A `state` line written after a `response` always reflects that request. For example, after the reply to `connect`, no stale `idle` line for that target follows. A `state` line written before the reply may or may not include the request's effect.

## Status fields (in `state` lines and most responses)

```json
{
  "type": "state",
  "ok": true,
  "state": "streaming",
  "streams": [
    { "device": "Living Room", "device_ip": "192.168.1.20", "state": "streaming", "has_audio": false, "audio_muted": false },
    { "device": "Bedroom",     "device_ip": "192.168.1.21", "state": "pin_required", "credential_kind": "pin" }
  ],
  "errors": [
    { "target": "192.168.1.30", "device": "Office", "kind": "unreachable", "message": "connect to 192.168.1.30:7000 failed: ..." }
  ]
}
```

- **`streams`**: one entry per target that is connecting, waiting for a code, or streaming, sorted by `device_ip`. A target's `device_ip` is exactly the `target` string the client sent. Use it as the key.
  - `state`: `connecting`, `pin_required` or `streaming`.
  - `credential_kind` (only while `pin_required`): `pin` means the TV shows a one-time code. `password` means the TV has "Require Password" set. Both are answered the same way (see `connect` with `pin`).
  - `device` is the TV's own name. It is empty until the TV has answered.
- **`errors`**: each target's latest failure, sorted by `target`. An entry stays until that target is connected again, so a client that missed a line can still see it. A target the user disconnected gets no entry.
  - `kind`: see the table below. Choose the words for the user from `kind`. `message` is technical detail for logs; never show it as is.
- **`state`**: the aggregate. It is `pin_required` if any target waits, else `streaming` if any streams, else `connecting` if any connects, else `idle`.
- Legacy single-target fields (`device`, `device_ip`, `needs_pin`, `needs_credential`, `credential_kind`, `error`) are kept for older clients. New clients should read `streams` and `errors`.

## Requests

Every request may carry `"id"` (an unsigned integer). Its `response` echoes it.

| `cmd` | Fields | Does | Response |
|---|---|---|---|
| `status` | | Nothing | Status |
| `connect` | `target`, optional `port` (default: the discovered port, else 7000), optional `options` | Starts mirroring to `target` (IP address or hostname). Returns at once; progress comes in `state` lines. Several targets can run at once, each with its own entry in `streams`. | Status, `ok: false` with `error_kind: busy` if `target` is already connecting or streaming |
| `connect` with `pin` | `target`, `pin` | Answers a waiting PIN or password prompt for `target` (`target` may be omitted when only one target waits) | `ok: true` once handed over. The result arrives as a `state` line: the stream goes on, or it leaves `streams` with an `errors` entry of kind `wrong_code`. |
| `disconnect` | optional `target` | Stops that target, or every target without `target`. Returns after teardown. | Status |
| `displays` | | Lists this PC's displays (Windows) | `displays`: `[{index, name, x, y, width, height, primary}]` |
| `devices` / `discover` | | Lists AirPlay receivers found over mDNS (refreshed every 5 s) | `devices`: `[{name, model, ip, port, device_id}]` |
| `mute` / `unmute` | optional `target` | Mutes a stream's audio | Status |

After `wrong_code`, the attempt is over: the TV drops it. Send `connect` again (without `pin`) so the TV shows a new code. Pairing keys are saved after a successful PIN pairing (Windows Credential Manager on Windows, the same store and keys as the `open-airplay` CLI). The next `connect` to that TV then pair-verifies without asking.

### `options` (per target, all optional)

Each option applies to that target only. Targets with different encoder choices (display, bitrate, encoder, cursor, latency) get their own encoder; targets with the same ones share one.

| Field | Meaning | Values |
|---|---|---|
| `display` | Display index from `displays` | `0` or more. The video is fitted to that display's size. |
| `fps` | Frames per second | `0` (automatic: the TV's recommended rate) to `120`. A rate above the TV's maximum fails the connect with `rejected`. |
| `bitrate_kbps` | Video bitrate | `0` (automatic: tuned for resolution and frame rate) to `100000` |
| `latency_ms` | Joint audio/video playout latency | `0` (automatic, by network) or `5` to `2000` |
| `video_codec` | Screen codec | `auto`, `h264`, `hevc`. Windows: `auto` or `h264` only; `hevc` is `unsupported_option`. |
| `hwaccel` | Encoder | `auto`, `nvenc`, `vaapi`, `openh264`, `none` (software). Windows: `auto`, `nvenc` or `none`; others are `unsupported_option`. |
| `show_cursor` | Draw the pointer | `true` or `false` |
| `audio` | Send this PC's sound | `true` or `false`. `true` is `unsupported_option` unless `hello.capabilities.audio` is `true`. |

An omitted field (or `0` or `auto`) means automatic, or the sender's command-line default. A value out of range is `invalid_request`. A value this build cannot honour is refused with `unsupported_option` rather than ignored, so a client never shows a setting that didn't take effect.

## Error kinds

`error_kind` on a failed response, and `kind` in `errors`:

| Kind | Meaning | Suggested words |
|---|---|---|
| `invalid_request` | Malformed JSON, unknown command, bad field | (a bug: log it) |
| `unsupported_option` | An `options` field this build can't honour | (hide that option) |
| `busy` | That target is already connecting or streaming | "Already mirroring to <TV>." |
| `not_waiting` | A code was sent, but nothing is waiting for one | (refresh from status) |
| `unreachable` | Could not open or keep the connection | "<TV> didn't answer. Check it's on and on the same network." |
| `wrong_code` | The TV rejected the PIN or password | "That code didn't work." Then `connect` again for a new code. |
| `rejected` | The TV refused pairing or the mirror setup | "<TV> didn't accept this PC." |
| `capture_failed` | Screen capture or the encoder failed | "Couldn't capture this PC's screen." |
| `ended` | The TV ended a running mirror (or the stream failed mid-way) | "Mirroring to <TV> ended." |
| `failed` | Anything else | "Couldn't mirror to <TV>." |

## Example

```
> {"id":1,"cmd":"connect","target":"192.168.1.20","options":{"display":0}}
< {"type":"response","id":1,"ok":true,"state":"connecting",...}
< {"type":"state","ok":true,"state":"pin_required","streams":[{"device":"Living Room","device_ip":"192.168.1.20","state":"pin_required","credential_kind":"pin",...}]}
> {"id":2,"cmd":"connect","target":"192.168.1.20","pin":"4821"}
< {"type":"response","id":2,"ok":true,"state":"connecting",...}
< {"type":"state","ok":true,"state":"streaming","streams":[{"device":"Living Room","device_ip":"192.168.1.20","state":"streaming",...}]}
> {"id":3,"cmd":"disconnect","target":"192.168.1.20"}
< {"type":"response","id":3,"ok":true,"state":"idle"}
< {"type":"state","ok":true,"state":"idle"}
```
