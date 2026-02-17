//go:build !windows

package browser

import (
	"os/exec"
	"syscall"
)

// setChromeProcessGroup configures Chrome to run in its own process group.
// This ensures all child processes (renderers, GPU, etc.) share the same PGID
// so they can be killed together on shutdown.
func setChromeProcessGroup(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{
		Setpgid: true,
	}
}

// killChromeProcessGroup sends a signal to the entire Chrome process group.
// force=false sends SIGTERM (graceful), force=true sends SIGKILL.
func killChromeProcessGroup(cmd *exec.Cmd, force bool) {
	if cmd.Process == nil {
		return
	}
	sig := syscall.SIGTERM
	if force {
		sig = syscall.SIGKILL
	}
	// Negative PID targets the entire process group
	_ = syscall.Kill(-cmd.Process.Pid, sig)
}
