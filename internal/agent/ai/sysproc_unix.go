//go:build !windows

package ai

import "syscall"

// cliSysProcAttr returns SysProcAttr for CLI subprocesses.
// On Unix, Setpgid forces Go to use fork+exec instead of posix_spawn on macOS,
// which avoids EINVAL errors that occur when Nebo's process state triggers a
// posix_spawn edge case.
func cliSysProcAttr() *syscall.SysProcAttr {
	return &syscall.SysProcAttr{Setpgid: true}
}
