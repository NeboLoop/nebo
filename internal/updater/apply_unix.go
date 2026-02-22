//go:build !windows

package updater

import (
	"fmt"
	"os"
	"path/filepath"
	"syscall"
)

// Apply replaces the current binary with newBinaryPath and restarts the process.
// On unix, the running binary can be replaced while the process is active.
func Apply(newBinaryPath string) error {
	currentExe, err := os.Executable()
	if err != nil {
		return fmt.Errorf("updater: resolve executable: %w", err)
	}
	realPath, err := filepath.EvalSymlinks(currentExe)
	if err != nil {
		return fmt.Errorf("updater: resolve symlinks: %w", err)
	}

	// Health check: run "nebo --version" on the new binary with 5s timeout
	if err := healthCheck(newBinaryPath); err != nil {
		return err
	}

	// Backup current binary
	backupPath := realPath + ".old"
	if err := copyFile(realPath, backupPath); err != nil {
		return fmt.Errorf("updater: backup current binary: %w", err)
	}

	// Replace current binary with new one
	if err := copyFile(newBinaryPath, realPath); err != nil {
		// Rollback
		_ = copyFile(backupPath, realPath)
		return fmt.Errorf("updater: replace binary: %w", err)
	}

	// Clean up temp file
	os.Remove(newBinaryPath)

	// Exec into the new binary — replaces this process in-place
	return syscall.Exec(realPath, os.Args, os.Environ())
}
