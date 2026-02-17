//go:build !windows

package apps

import (
	"os"
	"os/exec"
	"syscall"
	"time"
)

// isProcessAlive checks if a process with the given PID is still running.
func isProcessAlive(pid int) bool {
	return syscall.Kill(pid, 0) == nil
}

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

// killProcGroupTerm sends SIGTERM to the entire process group.
// Use this for graceful shutdown — pair with a timeout + killProcGroup fallback.
func killProcGroupTerm(cmd *exec.Cmd) {
	if cmd.Process != nil {
		_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGTERM)
	}
}

// killOrphanGroup kills an orphaned process group from a previous Nebo run.
// Since apps are launched with Setpgid: true, the app PID == PGID.
// We send SIGTERM to -PID (the whole group), wait briefly, then SIGKILL.
func killOrphanGroup(pid int) {
	// Try SIGTERM first for graceful shutdown
	_ = syscall.Kill(-pid, syscall.SIGTERM)

	// Brief wait for graceful exit
	time.Sleep(500 * time.Millisecond)

	// Force kill if the leader or any children survived
	if syscall.Kill(pid, 0) == nil {
		_ = syscall.Kill(-pid, syscall.SIGKILL)
		// Reap the zombie
		if proc, err := os.FindProcess(pid); err == nil {
			proc.Wait()
		}
	}
}
