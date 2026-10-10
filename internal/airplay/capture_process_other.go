//go:build !linux

package airplay

import "os/exec"

func superviseCaptureCommand(cmd *exec.Cmd, started chan<- error, waitResult chan<- error) {
	defer close(waitResult)

	err := cmd.Start()
	started <- err
	if err != nil {
		return
	}
	waitResult <- cmd.Wait()
}
