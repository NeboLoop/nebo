//go:build windows

package cli

import (
	"fmt"
	"os"

	"golang.org/x/sys/windows"
)

// acquireLock creates a lock file to ensure only one nebo instance runs
func acquireLock(dataDir string) (*os.File, error) {
	lockPath := dataDir + "/nebo.lock"

	// Try to create/open the lock file
	file, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return nil, fmt.Errorf("cannot open lock file: %w", err)
	}

	// Try to get exclusive lock using Windows LockFileEx
	handle := windows.Handle(file.Fd())
	overlapped := &windows.Overlapped{}

	// LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY
	err = windows.LockFileEx(handle, windows.LOCKFILE_EXCLUSIVE_LOCK|windows.LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, overlapped)
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
		handle := windows.Handle(file.Fd())
		overlapped := &windows.Overlapped{}
		windows.UnlockFileEx(handle, 0, 1, 0, overlapped)
		file.Close()
	}
}
