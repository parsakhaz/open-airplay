//go:build linux

package airplay

import (
	"os/exec"
	"syscall"
	"testing"
)

func assertCaptureProcessAttributes(t *testing.T, cmd *exec.Cmd) {
	t.Helper()
	if cmd.SysProcAttr == nil || cmd.SysProcAttr.Pdeathsig != syscall.SIGKILL {
		t.Fatalf("Pdeathsig = %v, want SIGKILL", cmd.SysProcAttr)
	}
}
