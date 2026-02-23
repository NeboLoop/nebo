//go:build windows

package updater

import (
	"fmt"
	"os"
	"os/exec"
)

// Apply replaces the current binary with newBinaryPath and restarts the process.
// On Windows, a running .exe can be renamed but not overwritten, so we rename
// the current binary to .old, copy the new one into place, and spawn a new process.
func Apply(newBinaryPath string) error {
	currentExe, err := os.Executable()
	if err != nil {
		return fmt.Errorf("updater: resolve executable: %w", err)
	}

	// Health check: run "nebo --version" on the new binary with 5s timeout
	if err := healthCheck(newBinaryPath); err != nil {
		return err
	}

	// Rename current exe to .old (Windows allows renaming a running exe)
	backupPath := currentExe + ".old"
	os.Remove(backupPath) // remove stale backup if exists
	if err := os.Rename(currentExe, backupPath); err != nil {
		return fmt.Errorf("updater: rename current exe: %w", err)
	}

	// Copy new binary into place (not rename — temp dir may be on different filesystem)
	if err := copyFile(newBinaryPath, currentExe); err != nil {
		// Rollback: restore backup
		_ = os.Rename(backupPath, currentExe)
		return fmt.Errorf("updater: copy new binary: %w", err)
	}
	os.Remove(newBinaryPath)

	// Release resources (lock files, connections) before spawning
	runPreApply()

	// Spawn new process and exit
	newCmd := exec.Command(currentExe, os.Args[1:]...)
	newCmd.Stdout = os.Stdout
	newCmd.Stderr = os.Stderr
	if err := newCmd.Start(); err != nil {
		return fmt.Errorf("updater: start new process: %w", err)
	}

	os.Exit(0)
	return nil // unreachable
}
