//go:build !windows

package apps

import (
	"fmt"
	"os"
	"os/exec"
	"strconv"
	"strings"
)

// killOrphansByBinary scans the process table for any processes running the
// given binary path. Kills any that aren't our own PID (current nebo process).
// This catches orphans that survived a previous nebo crash and whose .pid file
// was already overwritten.
func killOrphansByBinary(binaryPath, appID string) {
	// Use pgrep -f to find processes matching the binary path
	out, err := exec.Command("pgrep", "-f", binaryPath).Output()
	if err != nil {
		return // No matches or pgrep not available
	}

	myPID := os.Getpid()
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		pid, err := strconv.Atoi(strings.TrimSpace(line))
		if err != nil || pid <= 0 || pid == myPID {
			continue
		}

		// Verify this process is actually running our binary (pgrep -f can be fuzzy)
		cmdline, err := exec.Command("ps", "-p", strconv.Itoa(pid), "-o", "command=").Output()
		if err != nil {
			continue
		}
		if !strings.Contains(string(cmdline), binaryPath) {
			continue
		}

		fmt.Printf("[apps] Found orphaned %s process (PID %d), killing...\n", appID, pid)
		killOrphan(pid, appID)
	}
}
