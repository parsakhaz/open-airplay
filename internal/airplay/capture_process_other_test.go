//go:build !linux

package airplay

import (
	"os/exec"
	"testing"
)

func assertCaptureProcessAttributes(t *testing.T, cmd *exec.Cmd) {
	t.Helper()
	if cmd.SysProcAttr != nil {
		t.Fatalf("unexpected non-Linux process attributes: %v", cmd.SysProcAttr)
	}
}
