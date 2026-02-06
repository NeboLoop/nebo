//go:build darwin || linux

package cli

import (
	"fmt"
	"os"
	"syscall"
)

// acquireLock creates a lock file to ensure only one nebo instance runs
func acquireLock(dataDir string) (*os.File, error) {
	lockPath := dataDir + "/nebo.lock"

	// Try to create/open the lock file
	file, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return nil, fmt.Errorf("cannot open lock file: %w", err)
	}

	// Try to get exclusive lock (non-blocking)
	err = syscall.Flock(int(file.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
	if err != nil {
		file.Close()
		return nil, fmt.Errorf("cannot acquire lock")
	}

	// Write our PID to the lock file
	file.Truncate(0)
	file.Seek(0, 0)
	fmt.Fprintf(file, "%d\n", os.Getpid())
	file.Sync()

	return file, nil
}

// releaseLock releases the lock file
func releaseLock(file *os.File) {
	if file != nil {
		syscall.Flock(int(file.Fd()), syscall.LOCK_UN)
		file.Close()
	}
}
