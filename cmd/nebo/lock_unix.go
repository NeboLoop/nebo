//go:build darwin || linux

package cli

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"syscall"
	"time"
)

// acquireLock creates a lock file to ensure only one nebo instance runs.
// If the lock is held by a dead process, it removes the stale lock and retries.
func acquireLock(dataDir string) (*os.File, error) {
	lockPath := dataDir + "/nebo.lock"

	file, err := tryLock(lockPath)
	if err == nil {
		return file, nil
	}

	// Lock failed — check if the holder is still alive
	pid := readLockPID(lockPath)
	if pid > 0 && !isProcessAlive(pid) {
		fmt.Printf("Removing stale lock from dead process (PID %d)\n", pid)
		os.Remove(lockPath)
		// Brief pause to let the OS fully release the file handle
		time.Sleep(100 * time.Millisecond)
		return tryLock(lockPath)
	}

	if pid > 0 {
		return nil, fmt.Errorf("cannot acquire lock (held by PID %d)", pid)
	}
	return nil, fmt.Errorf("cannot acquire lock")
}

// tryLock attempts to open and exclusively lock the lock file.
func tryLock(lockPath string) (*os.File, error) {
	file, err := os.OpenFile(lockPath, os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return nil, fmt.Errorf("cannot open lock file: %w", err)
	}

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

// readLockPID reads the PID from an existing lock file.
func readLockPID(lockPath string) int {
	data, err := os.ReadFile(lockPath)
	if err != nil {
		return 0
	}
	pid, err := strconv.Atoi(strings.TrimSpace(string(data)))
	if err != nil {
		return 0
	}
	return pid
}

// isProcessAlive checks whether a process with the given PID is still running.
func isProcessAlive(pid int) bool {
	err := syscall.Kill(pid, 0)
	return err == nil
}

// releaseLock releases the lock file
func releaseLock(file *os.File) {
	if file != nil {
		syscall.Flock(int(file.Fd()), syscall.LOCK_UN)
		file.Close()
	}
}
