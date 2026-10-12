package daemon

import (
	"fmt"
	"io"
	"net"
	"testing"
	"time"
)

// silentReceiverAt is silentReceiver on a chosen loopback address, so two
// receivers get two different targets (127.0.0.1 and 127.0.0.2).
func silentReceiverAt(t *testing.T, address string) (string, int, <-chan struct{}, <-chan struct{}) {
	t.Helper()
	listener, err := net.Listen("tcp4", address)
	if err != nil {
		t.Skipf("listen on %s: %v", address, err)
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

func streamTargets(m StdioMessage) []string {
	targets := make([]string, 0, len(m.Streams))
	for _, s := range m.Streams {
		targets = append(targets, s.DeviceIP)
	}
	return targets
}

func sameTargets(got []string, want ...string) bool {
	if len(got) != len(want) {
		return false
	}
	for i := range got {
		if got[i] != want[i] {
			return false
		}
	}
	return true
}

func expectClosed(t *testing.T, closed <-chan struct{}, what string) {
	t.Helper()
	select {
	case <-closed:
	case <-time.After(5 * time.Second):
		t.Fatalf("%s: receiver connection stayed open", what)
	}
}

func expectOpen(t *testing.T, closed <-chan struct{}, what string) {
	t.Helper()
	select {
	case <-closed:
		t.Fatalf("%s: receiver connection was closed", what)
	case <-time.After(300 * time.Millisecond):
	}
}

// The app adds a second TV to a running mirror, removes one, then stops all.
func TestStdioAddsAndRemovesTargetsWhileOthersRun(t *testing.T) {
	hostA, portA, acceptedA, closedA := silentReceiverAt(t, "127.0.0.1:0")
	hostB, portB, acceptedB, closedB := silentReceiverAt(t, "127.0.0.2:0")
	h := newStdioHarness(t, newStdioTestDaemon(t))

	h.send(fmt.Sprintf(`{"id":1,"cmd":"connect","target":%q,"port":%d}`, hostA, portA))
	if resp := h.response(1); !resp.OK {
		t.Fatalf("connect A rejected: %+v", resp)
	}
	waitAccepted(t, acceptedA)

	// Adding a second target while the first runs is not "busy".
	h.send(fmt.Sprintf(`{"id":2,"cmd":"connect","target":%q,"port":%d}`, hostB, portB))
	if resp := h.response(2); !resp.OK {
		t.Fatalf("connect B while A runs rejected: %+v", resp)
	}
	both := h.state("two streams", func(m StdioMessage) bool { return len(m.Streams) == 2 })
	if !sameTargets(streamTargets(both), hostA, hostB) {
		t.Fatalf("streams = %v, want [%s %s] keyed by target", streamTargets(both), hostA, hostB)
	}
	waitAccepted(t, acceptedB)

	// The same target again is busy and leaves the other alone.
	h.send(fmt.Sprintf(`{"id":3,"cmd":"connect","target":%q,"port":%d}`, hostA, portA))
	if resp := h.response(3); resp.OK || resp.ErrorKind != ErrorKindBusy {
		t.Fatalf("second connect to A = %+v, want busy", resp)
	}

	// Removing one device ends only that one.
	h.send(fmt.Sprintf(`{"id":4,"cmd":"disconnect","target":%q}`, hostA))
	if resp := h.response(4); !resp.OK || resp.State != StateConnecting {
		t.Fatalf("disconnect A = %+v, want ok with B still connecting", resp)
	}
	left := h.state("only B", func(m StdioMessage) bool { return len(m.Streams) == 1 })
	if !sameTargets(streamTargets(left), hostB) || len(left.Errors) != 0 {
		t.Fatalf("after removing A: streams %v errors %+v, want only %s and no error", streamTargets(left), left.Errors, hostB)
	}
	expectClosed(t, closedA, "removed A")
	expectOpen(t, closedB, "B after removing A")

	// Stop all.
	h.send(`{"id":5,"cmd":"disconnect"}`)
	if resp := h.response(5); !resp.OK || resp.State != StateIdle {
		t.Fatalf("disconnect all = %+v", resp)
	}
	h.state("idle", func(m StdioMessage) bool { return m.State == StateIdle && len(m.Streams) == 0 })
	expectClosed(t, closedB, "stop all")
}
