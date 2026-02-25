//go:build windows

package tools

import "os/exec"

// ShellCommand returns the shell command and arguments for Windows systems.
// Prefers PowerShell (pwsh or powershell.exe) over cmd.exe so that the LLM
// can use the same cmdlets that platform tools rely on (Get-Process, etc.).
func ShellCommand() (shell string, args []string) {
	// Prefer PowerShell 7+ (pwsh) if available
	if path, err := exec.LookPath("pwsh"); err == nil {
		return path, []string{"-NoProfile", "-Command"}
	}
	// Fall back to Windows PowerShell 5.x (always present on Win10+)
	if path, err := exec.LookPath("powershell.exe"); err == nil {
		return path, []string{"-NoProfile", "-Command"}
	}
	// Last resort: cmd.exe
	return "cmd.exe", []string{"/C"}
}

// ShellName returns a human-readable name for the shell
func ShellName() string {
	if _, err := exec.LookPath("pwsh"); err == nil {
		return "powershell"
	}
	if _, err := exec.LookPath("powershell.exe"); err == nil {
		return "powershell"
	}
	return "cmd"
}
