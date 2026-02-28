//go:build windows

package cli

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"time"

	"golang.org/x/sys/windows"
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

	handle := windows.Handle(file.Fd())
	overlapped := &windows.Overlapped{}

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
	handle, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION, false, uint32(pid))
	if err != nil {
		return false
	}
	defer windows.CloseHandle(handle)

	var exitCode uint32
	err = windows.GetExitCodeProcess(handle, &exitCode)
	if err != nil {
		return false
	}
	// STILL_ACTIVE (259) means the process is still running
	return exitCode == 259
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
