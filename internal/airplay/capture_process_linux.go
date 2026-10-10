//go:build linux

package airplay

import (
	"os/exec"
	"runtime"
	"syscall"
)

// superviseCaptureCommand keeps the launching OS thread alive because Linux
// applies Pdeathsig to the thread that created the child, not just the process.
func superviseCaptureCommand(cmd *exec.Cmd, started chan<- error, waitResult chan<- error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	defer close(waitResult)

	cmd.SysProcAttr = &syscall.SysProcAttr{Pdeathsig: syscall.SIGKILL}
	err := cmd.Start()
	started <- err
	if err != nil {
		return
	}
	waitResult <- cmd.Wait()
}
