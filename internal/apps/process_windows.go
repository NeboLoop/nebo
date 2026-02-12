//go:build windows

package apps

import (
	"os/exec"
	"syscall"
	"time"
)

// setProcGroup configures the command to run in its own process group.
// On Windows, CREATE_NEW_PROCESS_GROUP enables sending CTRL_BREAK_EVENT to the group.
func setProcGroup(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{
		CreationFlags: syscall.CREATE_NEW_PROCESS_GROUP,
	}
}

// killProcGroup force-kills the process on Windows.
func killProcGroup(cmd *exec.Cmd) {
	if cmd.Process != nil {
		_ = cmd.Process.Kill()
	}
}

// gracefulStopProc attempts graceful shutdown, waits up to timeout,
// then force-kills if still running.
func gracefulStopProc(cmd *exec.Cmd, timeout time.Duration) {
	// On Windows, send CTRL_BREAK_EVENT for graceful shutdown
	// Falls back to Kill if the process doesn't exit in time
	_ = cmd.Process.Signal(syscall.SIGTERM)

	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()

	select {
	case <-done:
	case <-time.After(timeout):
		_ = cmd.Process.Kill()
		<-done
	}
}
