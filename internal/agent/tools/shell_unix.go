//go:build darwin || linux

package tools

import "os"

// ShellCommand returns the shell command and arguments for Unix systems.
// Uses absolute path to prevent PATH-based binary substitution attacks.
func ShellCommand() (shell string, args []string) {
	// Prefer /bin/bash (standard location), fall back to relative for portability
	for _, path := range []string{"/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"} {
		if _, err := os.Stat(path); err == nil {
			return path, []string{"-c"}
		}
	}
	return "bash", []string{"-c"}
}

// ShellName returns a human-readable name for the shell
func ShellName() string {
	return "bash"
}
