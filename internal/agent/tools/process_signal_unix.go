//go:build darwin || linux

package tools

import (
	"os"
	"strings"
	"syscall"
)

// KillProcessWithSignal sends a signal to a process on Unix systems
func KillProcessWithSignal(process *os.Process, signal string) error {
	var sig syscall.Signal
	switch strings.ToUpper(signal) {
	case "SIGKILL", "9":
		sig = syscall.SIGKILL
	case "SIGINT", "2":
		sig = syscall.SIGINT
	case "SIGHUP", "1":
		sig = syscall.SIGHUP
	default:
		sig = syscall.SIGTERM
	}
	return process.Signal(sig)
}

// SignalSupported returns true if Unix signals are supported
func SignalSupported() bool {
	return true
}

// DefaultSignalName returns the default signal name for graceful termination
func DefaultSignalName() string {
	return "SIGTERM"
}
