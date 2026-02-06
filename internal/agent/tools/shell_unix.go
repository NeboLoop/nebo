//go:build darwin || linux

package tools

// ShellCommand returns the shell command and arguments for Unix systems
func ShellCommand() (shell string, args []string) {
	return "bash", []string{"-c"}
}

// ShellName returns a human-readable name for the shell
func ShellName() string {
	return "bash"
}
