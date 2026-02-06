//go:build windows

package tools

// ShellCommand returns the shell command and arguments for Windows systems
func ShellCommand() (shell string, args []string) {
	return "cmd.exe", []string{"/C"}
}

// ShellName returns a human-readable name for the shell
func ShellName() string {
	return "cmd"
}
