package daemon

import (
	"bufio"
	"context"
	"encoding/json"
	"io"
	"log"
	"reflect"
	"sync"
	"time"

	"github.com/parsakhaz/open-airplay/internal/airplay"
)

// StdioProtocolVersion is announced in the first line. It changes only when
// a field or command changes meaning; additions keep the version.
const StdioProtocolVersion = 1

// StdioMessage is one line written by the stdio control mode. "hello" comes
// first and carries Protocol; "response" answers the request with the same ID;
// "state" is pushed whenever the daemon's status changes, so a parent process
// never has to poll. See docs/control-protocol.md.
type StdioMessage struct {
	Type         string        `json:"type"`
	ID           uint64        `json:"id,omitempty"`
	Protocol     int           `json:"protocol,omitempty"`
	Capabilities *Capabilities `json:"capabilities,omitempty"` // hello only
	Response
}

// Capabilities, announced in hello, say which optional features this build
// supports, so a client can hide controls before trying them.
type Capabilities struct {
	// Audio is true when connect honours options.audio: true (this build can
	// capture the computer's own sound).
	Audio bool `json:"audio"`
}

// stdioStatePoll bounds how stale a pushed state line can be. Stream state is
// changed from many worker goroutines, so the stdio loop samples the status
// snapshot rather than threading a notifier through every transition.
const stdioStatePoll = 100 * time.Millisecond

// RunStdio serves the control protocol over in and out instead of a Unix
// socket: one JSON Request per input line, one StdioMessage per output line.
// It exists for Windows, where the socket control plane is unavailable, and
// for parent processes that run the sender as a child. It returns after in
// reaches EOF or ctx is cancelled, once every stream has been torn down.
func (d *Daemon) RunStdio(ctx context.Context, in io.Reader, out io.Writer) error {
	airplay.SetDebugMode(d.cfg.Debug)

	ctx, cancel := context.WithCancel(ctx)
	defer cancel()

	w := &stdioWriter{enc: json.NewEncoder(out)}
	w.write(StdioMessage{
		Type:         "hello",
		Protocol:     StdioProtocolVersion,
		Capabilities: &Capabilities{Audio: airplay.SystemAudioCaptureAvailable()},
	})

	if !d.skipDiscovery {
		d.mu.Lock()
		discoverCtx, discoverCancel := context.WithCancel(ctx)
		d.discoverCancel = discoverCancel
		d.mu.Unlock()
		go d.backgroundDiscover(discoverCtx)
	}

	kick := make(chan struct{}, 1)
	pushDone := make(chan struct{})
	go func() {
		defer close(pushDone)
		d.pushStateLines(ctx, kick, w)
	}()

	lines := make(chan []byte)
	go func() {
		defer close(lines)
		scanner := bufio.NewScanner(in)
		scanner.Buffer(make([]byte, 0, 64*1024), 1024*1024)
		for scanner.Scan() {
			line := append([]byte(nil), scanner.Bytes()...)
			select {
			case lines <- line:
			case <-ctx.Done():
				return
			}
		}
		if err := scanner.Err(); err != nil {
			log.Printf("[daemon] read control input: %v", err)
		}
	}()

	log.Printf("[daemon] serving control requests on stdin")
	for running := true; running; {
		select {
		case <-ctx.Done():
			running = false
		case line, ok := <-lines:
			if !ok {
				running = false
				break
			}
			if len(line) == 0 {
				continue
			}
			var req Request
			if err := json.Unmarshal(line, &req); err != nil {
				w.write(StdioMessage{Type: "response", Response: Response{OK: false, Error: "invalid request: " + err.Error(), ErrorKind: ErrorKindInvalidRequest}})
				continue
			}
			w.write(StdioMessage{Type: "response", ID: req.ID, Response: d.handleRequest(req)})
			select {
			case kick <- struct{}{}:
			default:
			}
		}
	}

	log.Printf("[daemon] control input closed; stopping")
	d.Shutdown()
	cancel()
	<-pushDone
	return nil
}

// stdioWriter serializes output lines.
type stdioWriter struct {
	mu  sync.Mutex
	enc *json.Encoder
}

func (w *stdioWriter) write(msg StdioMessage) {
	w.mu.Lock()
	defer w.mu.Unlock()
	w.encodeLocked(msg)
}

func (w *stdioWriter) encodeLocked(msg StdioMessage) {
	if err := w.enc.Encode(msg); err != nil {
		log.Printf("[daemon] write control line: %v", err)
	}
}

// pushStateLines writes a "state" line at start and after every change of the
// status snapshot, until ctx is cancelled. The snapshot is taken while holding
// the output lock, so a state line written after a request's response always
// reflects that request: a caller never sees a stale "idle" after "connect".
func (d *Daemon) pushStateLines(ctx context.Context, kick <-chan struct{}, w *stdioWriter) {
	ticker := time.NewTicker(stdioStatePoll)
	defer ticker.Stop()
	var last *Response
	for {
		w.mu.Lock()
		status := d.handleStatus()
		if last == nil || !reflect.DeepEqual(*last, status) {
			last = &status
			w.encodeLocked(StdioMessage{Type: "state", Response: status})
		}
		w.mu.Unlock()
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		case <-kick:
		}
	}
}
