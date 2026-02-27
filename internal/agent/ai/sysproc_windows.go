//go:build windows

package ai

import "syscall"

// cliSysProcAttr returns SysProcAttr for CLI subprocesses.
// On Windows, Setpgid is not available so we return a default struct.
func cliSysProcAttr() *syscall.SysProcAttr {
	return &syscall.SysProcAttr{}
}
