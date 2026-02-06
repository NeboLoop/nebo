//go:build windows

package tools

import (
	"os"
)

// KillProcessWithSignal terminates a process on Windows
// Windows doesn't support Unix signals, so all signals are treated as Kill
func KillProcessWithSignal(process *os.Process, signal string) error {
	// Windows only supports Kill() - no graceful termination via signals
	return process.Kill()
}

// SignalSupported returns false on Windows since Unix signals aren't supported
func SignalSupported() bool {
	return false
}

// DefaultSignalName returns the default termination method name
func DefaultSignalName() string {
	return "KILL"
}
