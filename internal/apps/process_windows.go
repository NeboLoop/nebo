//go:build windows

package apps

import (
	"os"
	"os/exec"
	"strconv"
	"syscall"
)

// isProcessAlive checks if a process with the given PID is still running.
func isProcessAlive(pid int) bool {
	p, err := os.FindProcess(pid)
	if err != nil {
		return false
	}
	// On Windows, FindProcess always succeeds. Signal(0) returns an error
	// if the process doesn't exist or access is denied.
	return p.Signal(syscall.Signal(0)) == nil
}

// setProcGroup configures the command to run in its own process group.
// On Windows, CREATE_NEW_PROCESS_GROUP enables sending CTRL_BREAK_EVENT to the group.
func setProcGroup(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{
		CreationFlags: syscall.CREATE_NEW_PROCESS_GROUP,
	}
}

// killProcGroup force-kills the process AND all its children on Windows.
// Uses taskkill.exe /t (tree kill) to recursively terminate the entire process
// tree. Without /t, only the leader dies and children become orphans.
func killProcGroup(cmd *exec.Cmd) {
	if cmd.Process != nil {
		_ = exec.Command("taskkill.exe", "/t", "/f", "/pid", strconv.Itoa(cmd.Process.Pid)).Run()
	}
}

// killProcGroupTerm sends a termination signal to the process tree on Windows.
// Windows doesn't have SIGTERM for process groups — taskkill without /f sends
// WM_CLOSE to GUI apps (graceful) but console apps need /f. We try graceful
// first; the caller should follow up with killProcGroup after a timeout.
func killProcGroupTerm(cmd *exec.Cmd) {
	if cmd.Process != nil {
		// Try graceful tree kill (WM_CLOSE for GUI apps, no effect on console apps)
		_ = exec.Command("taskkill.exe", "/t", "/pid", strconv.Itoa(cmd.Process.Pid)).Run()
	}
}

// killOrphanGroup kills an orphaned process tree on Windows.
func killOrphanGroup(pid int) {
	// Force kill the entire process tree
	_ = exec.Command("taskkill.exe", "/t", "/f", "/pid", strconv.Itoa(pid)).Run()
	// Reap the process handle
	if proc, err := os.FindProcess(pid); err == nil {
		proc.Wait()
	}
}
