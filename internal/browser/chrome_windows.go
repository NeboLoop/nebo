//go:build windows

package browser

import (
	"os"
	"os/exec"
)

// setChromeProcessGroup is a no-op on Windows.
// Windows doesn't use Unix process groups.
func setChromeProcessGroup(cmd *exec.Cmd) {
	// Windows: no process group setup needed
}

// killChromeProcessGroup kills the Chrome process on Windows.
// Windows doesn't have Unix-style process groups, so we kill the main process
// and rely on Chrome's own cleanup for child processes.
func killChromeProcessGroup(cmd *exec.Cmd, force bool) {
	if cmd.Process == nil {
		return
	}
	if force {
		_ = cmd.Process.Kill()
	} else {
		_ = cmd.Process.Signal(os.Interrupt)
	}
}
