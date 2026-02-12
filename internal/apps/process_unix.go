//go:build !windows

package apps

import (
	"os/exec"
	"syscall"
	"time"
)

// setProcGroup configures the command to run in its own process group.
// This enables killing the app and all its child processes together.
func setProcGroup(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{
		Setpgid: true,
	}
}

// killProcGroup force-kills the process and its entire process group.
func killProcGroup(cmd *exec.Cmd) {
	if cmd.Process != nil {
		_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
	}
}

// gracefulStopProc sends SIGTERM to the process group, waits up to timeout,
// then force-kills if still running.
func gracefulStopProc(cmd *exec.Cmd, timeout time.Duration) {
	pid := cmd.Process.Pid
	_ = syscall.Kill(-pid, syscall.SIGTERM)

	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()

	select {
	case <-done:
	case <-time.After(timeout):
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		<-done
	}
}
