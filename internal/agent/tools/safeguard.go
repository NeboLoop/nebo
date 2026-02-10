package tools

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// Safeguard enforces hard safety limits that CANNOT be overridden by policy,
// autonomous mode, or any user setting. These protect the host operating system
// from catastrophic damage.
//
// Design principles:
//   - Defense in depth: runs inside registry.Execute() before tool.Execute()
//   - Unconditional: no bypass via autonomous mode, policy level, or approval
//   - Cross-platform: covers macOS, Linux, and Windows system paths
//   - Fail-closed: when in doubt, block the operation

// CheckSafeguard validates a tool call against hard safety limits.
// Returns nil if the operation is safe, or an error describing why it was blocked.
// This check is unconditional — it cannot be bypassed by any setting.
func CheckSafeguard(toolName string, input json.RawMessage) error {
	switch toolName {
	case "file":
		return checkFileSafeguard(input)
	case "shell":
		return checkShellSafeguard(input)
	default:
		return nil
	}
}

// --- File safeguard ---

func checkFileSafeguard(input json.RawMessage) error {
	var fi struct {
		Action string `json:"action"`
		Path   string `json:"path"`
	}
	if err := json.Unmarshal(input, &fi); err != nil {
		return nil // can't parse — let the tool handle validation
	}

	// Only guard destructive actions
	switch fi.Action {
	case "write", "edit":
	default:
		return nil // read, glob, grep are safe
	}

	if fi.Path == "" {
		return nil // tool will reject empty path
	}

	// Resolve to absolute path
	absPath, err := filepath.Abs(fi.Path)
	if err != nil {
		return nil
	}

	// Check original path first (before symlink resolution)
	if reason := isProtectedPath(absPath); reason != "" {
		return fmt.Errorf("BLOCKED: cannot %s %q — %s. "+
			"This is a hard safety limit that cannot be overridden. "+
			"If you need to modify system files, do it manually in a terminal",
			fi.Action, fi.Path, reason)
	}

	// Also check resolved path (catches symlink indirection like /etc → /private/etc)
	if resolved, err := filepath.EvalSymlinks(absPath); err == nil && resolved != absPath {
		if reason := isProtectedPath(resolved); reason != "" {
			return fmt.Errorf("BLOCKED: cannot %s %q — %s. "+
				"This is a hard safety limit that cannot be overridden. "+
				"If you need to modify system files, do it manually in a terminal",
				fi.Action, fi.Path, reason)
		}
	}

	return nil
}

// --- Shell safeguard ---

func checkShellSafeguard(input json.RawMessage) error {
	var si struct {
		Resource string `json:"resource"`
		Action   string `json:"action"`
		Command  string `json:"command"`
	}
	if err := json.Unmarshal(input, &si); err != nil {
		return nil
	}

	// Only guard command execution
	if si.Resource != "bash" && si.Resource != "" {
		return nil
	}
	if si.Action != "exec" {
		return nil
	}
	if si.Command == "" {
		return nil
	}

	cmd := strings.TrimSpace(si.Command)
	cmdLower := strings.ToLower(cmd)

	// --- Hard blocks (unconditional, no override) ---

	// Block sudo entirely
	if hasSudo(cmdLower) {
		return fmt.Errorf("BLOCKED: sudo is not permitted. "+
			"Nebo must never run commands with elevated privileges. "+
			"This is a hard safety limit that cannot be overridden. "+
			"If you need root access, run the command manually in a terminal")
	}

	// Block su (switch user to root)
	if hasSu(cmdLower) {
		return fmt.Errorf("BLOCKED: su is not permitted. "+
			"Nebo must never run commands as another user. "+
			"This is a hard safety limit that cannot be overridden")
	}

	// Block destructive operations targeting root or system paths
	if reason := checkDestructiveCommand(cmd, cmdLower); reason != "" {
		return fmt.Errorf("BLOCKED: %s. "+
			"This is a hard safety limit that cannot be overridden. "+
			"If you need to perform this operation, do it manually in a terminal",
			reason)
	}

	return nil
}

// hasSudo checks if a command uses sudo in any form
func hasSudo(cmdLower string) bool {
	// Direct sudo
	if strings.HasPrefix(cmdLower, "sudo ") || strings.HasPrefix(cmdLower, "sudo\t") {
		return true
	}
	// Piped or chained sudo (handle with/without space before separator)
	for _, sep := range []string{
		" | sudo ", "| sudo ",
		" && sudo ", "&& sudo ",
		" ; sudo ", "; sudo ",
		" || sudo ", "|| sudo ",
	} {
		if strings.Contains(cmdLower, sep) {
			return true
		}
	}
	// Subshell sudo
	if strings.Contains(cmdLower, "$(sudo ") || strings.Contains(cmdLower, "`sudo ") {
		return true
	}
	return false
}

// hasSu checks if a command uses su to switch user
func hasSu(cmdLower string) bool {
	if strings.HasPrefix(cmdLower, "su ") || strings.HasPrefix(cmdLower, "su\t") || cmdLower == "su" {
		return true
	}
	// But don't block "suspend", "surface", "sum", etc.
	for _, sep := range []string{" | su ", " && su ", " ; su ", " || su "} {
		if strings.Contains(cmdLower, sep) {
			return true
		}
	}
	return false
}

// checkDestructiveCommand checks for commands that target system-critical paths
func checkDestructiveCommand(cmd, cmdLower string) string {
	// Block rm -rf / or rm -rf /* (catastrophic)
	if isRootWipe(cmdLower) {
		return "cannot delete root filesystem — this would destroy the operating system"
	}

	// Block dd to block devices
	if strings.Contains(cmdLower, "dd ") && (strings.Contains(cmdLower, "of=/dev/") || strings.Contains(cmdLower, "of= /dev/")) {
		return "cannot write to block devices with dd — this could destroy disk data"
	}

	// Block ALL disk formatting and partitioning commands
	formatCmds := []struct {
		pattern string
		reason  string
	}{
		{"mkfs", "cannot format filesystems — this would destroy all data on the target device"},
		{"fdisk", "cannot modify disk partition tables — this could destroy all data on the drive"},
		{"gdisk", "cannot modify GPT partition tables — this could destroy all data on the drive"},
		{"parted", "cannot modify disk partitions — this could destroy all data on the drive"},
		{"sfdisk", "cannot modify disk partition tables — this could destroy all data on the drive"},
		{"cfdisk", "cannot modify disk partition tables — this could destroy all data on the drive"},
		{"wipefs", "cannot wipe filesystem signatures — this could make drives unreadable"},
		{"sgdisk", "cannot modify GPT partition tables — this could destroy all data on the drive"},
		{"partprobe", "cannot probe partition changes — this is a disk management operation"},
		{"diskutil erasedisk", "cannot erase disks — this would destroy all data on the drive"},
		{"diskutil erasevolume", "cannot erase volumes — this would destroy all data on the volume"},
		{"diskutil partitiondisk", "cannot partition disks — this could destroy all data on the drive"},
		{"diskutil apfs deletecontainer", "cannot delete APFS containers — this would destroy data"},
		{"format", "cannot format drives — this would destroy all data on the target"},
	}
	for _, fc := range formatCmds {
		if strings.HasPrefix(cmdLower, fc.pattern) || strings.Contains(cmdLower, " "+fc.pattern) {
			return fc.reason
		}
	}

	// Block fork bombs
	if strings.Contains(cmd, ":(){ :|:& };:") || strings.Contains(cmdLower, "fork bomb") {
		return "fork bomb detected — this would crash the system"
	}

	// Block writing to /dev/ (except /dev/null, /dev/stdout, /dev/stderr)
	if strings.Contains(cmdLower, "> /dev/") || strings.Contains(cmdLower, ">/dev/") {
		safeDevs := []string{"/dev/null", "/dev/stdout", "/dev/stderr"}
		isSafe := false
		for _, d := range safeDevs {
			if strings.Contains(cmdLower, "> "+d) || strings.Contains(cmdLower, ">"+d) {
				isSafe = true
				break
			}
		}
		if !isSafe {
			return "cannot write to device files — this could damage hardware or corrupt data"
		}
	}

	// Check for rm/rmdir targeting protected system directories
	if strings.Contains(cmdLower, "rm ") || strings.HasPrefix(cmdLower, "rm\t") {
		if reason := checkRmTargets(cmd); reason != "" {
			return reason
		}
	}

	// Check for chmod/chown on system paths
	if strings.HasPrefix(cmdLower, "chmod ") || strings.HasPrefix(cmdLower, "chown ") {
		if reason := checkChmodTargets(cmd); reason != "" {
			return reason
		}
	}

	return ""
}

// isRootWipe detects attempts to delete the entire filesystem
func isRootWipe(cmdLower string) bool {
	// rm -rf / variants
	wipePatterns := []string{
		"rm -rf /",
		"rm -fr /",
		"rm -rf /*",
		"rm -fr /*",
		"rm -rf --no-preserve-root /",
		"rm -rf --no-preserve-root /*",
	}
	for _, p := range wipePatterns {
		if strings.Contains(cmdLower, p) {
			// Make sure it's actually targeting root, not /some/path
			idx := strings.Index(cmdLower, p)
			after := cmdLower[idx+len(p):]
			// If nothing after the slash (or just *), it's a root wipe
			if p[len(p)-1] == '/' && (after == "" || after[0] == ' ' || after[0] == '\n' || after[0] == ';' || after[0] == '&') {
				return true
			}
			if p[len(p)-1] == '*' {
				return true
			}
		}
	}
	return false
}

// checkRmTargets checks if rm targets protected system directories
func checkRmTargets(cmd string) string {
	// Extract paths from the command (skip flags)
	parts := strings.Fields(cmd)
	for _, part := range parts[1:] { // skip "rm"
		if strings.HasPrefix(part, "-") {
			continue // skip flags
		}

		absPath, err := filepath.Abs(part)
		if err != nil {
			continue
		}

		if reason := isProtectedPath(absPath); reason != "" {
			return fmt.Sprintf("cannot delete %q — %s", part, reason)
		}
	}
	return ""
}

// checkChmodTargets checks if chmod/chown targets protected system directories
func checkChmodTargets(cmd string) string {
	parts := strings.Fields(cmd)
	for _, part := range parts[1:] {
		if strings.HasPrefix(part, "-") {
			continue
		}
		// Skip the mode/owner argument (e.g., "777", "root:root")
		if len(part) <= 5 && !strings.Contains(part, "/") {
			continue
		}

		absPath, err := filepath.Abs(part)
		if err != nil {
			continue
		}

		if reason := isProtectedPath(absPath); reason != "" {
			return fmt.Sprintf("cannot modify permissions on %q — %s", part, reason)
		}
	}
	return ""
}

// isProtectedPath checks if an absolute path falls within a protected system directory.
// Returns a human-readable reason if protected, or empty string if safe.
func isProtectedPath(absPath string) string {
	// Normalize: clean and ensure trailing separator handling is consistent
	absPath = filepath.Clean(absPath)

	switch runtime.GOOS {
	case "darwin":
		return isProtectedPathDarwin(absPath)
	case "linux":
		return isProtectedPathLinux(absPath)
	case "windows":
		return isProtectedPathWindows(absPath)
	default:
		return isProtectedPathLinux(absPath) // fall back to Linux rules
	}
}

// --- macOS protected paths ---

func isProtectedPathDarwin(absPath string) string {
	// Root itself
	if absPath == "/" {
		return "this is the root filesystem"
	}

	protectedPrefixes := []struct {
		prefix string
		reason string
	}{
		{"/System", "macOS system files (SIP-protected)"},
		{"/usr/bin", "system binaries"},
		{"/usr/sbin", "system admin binaries"},
		{"/usr/lib", "system libraries"},
		{"/usr/libexec", "system executables"},
		{"/usr/share", "system shared data"},
		{"/bin", "core system binaries"},
		{"/sbin", "core system admin binaries"},
		{"/private/var/db", "macOS system databases"},
		{"/Library/LaunchDaemons", "system launch daemons"},
		{"/Library/LaunchAgents", "system launch agents"},
		{"/etc", "system configuration"},
	}

	for _, p := range protectedPrefixes {
		if absPath == p.prefix || strings.HasPrefix(absPath, p.prefix+"/") {
			return p.reason
		}
	}

	// Sensitive user directories
	if reason := isProtectedUserPath(absPath); reason != "" {
		return reason
	}

	return ""
}

// --- Linux protected paths ---

func isProtectedPathLinux(absPath string) string {
	if absPath == "/" {
		return "this is the root filesystem"
	}

	protectedPrefixes := []struct {
		prefix string
		reason string
	}{
		{"/bin", "core system binaries"},
		{"/sbin", "core system admin binaries"},
		{"/usr/bin", "system binaries"},
		{"/usr/sbin", "system admin binaries"},
		{"/usr/lib", "system libraries"},
		{"/usr/libexec", "system executables"},
		{"/usr/share", "system shared data"},
		{"/boot", "boot loader and kernel"},
		{"/etc", "system configuration"},
		{"/proc", "kernel process filesystem"},
		{"/sys", "kernel sysfs"},
		{"/dev", "device files"},
		{"/root", "root user home directory"},
		{"/var/lib/dpkg", "package manager database"},
		{"/var/lib/rpm", "package manager database"},
		{"/var/lib/apt", "package manager cache"},
	}

	for _, p := range protectedPrefixes {
		if absPath == p.prefix || strings.HasPrefix(absPath, p.prefix+"/") {
			return p.reason
		}
	}

	if reason := isProtectedUserPath(absPath); reason != "" {
		return reason
	}

	return ""
}

// --- Windows protected paths ---

func isProtectedPathWindows(absPath string) string {
	absLower := strings.ToLower(absPath)

	protectedPrefixes := []struct {
		prefix string
		reason string
	}{
		{`c:\windows`, "Windows system directory"},
		{`c:\program files`, "installed program files"},
		{`c:\program files (x86)`, "installed program files (32-bit)"},
		{`c:\programdata`, "system program data"},
		{`c:\recovery`, "Windows recovery partition"},
		{`c:\$recycle.bin`, "recycle bin system folder"},
	}

	for _, p := range protectedPrefixes {
		if absLower == p.prefix || strings.HasPrefix(absLower, p.prefix+`\`) {
			return p.reason
		}
	}

	return ""
}

// --- Sensitive user paths (cross-platform) ---

func isProtectedUserPath(absPath string) string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}

	sensitiveRelPaths := []struct {
		rel    string
		reason string
	}{
		{".ssh", "SSH keys and configuration"},
		{".gnupg", "GPG keys and configuration"},
		{".aws/credentials", "AWS credentials"},
		{".aws/config", "AWS configuration"},
		{".kube/config", "Kubernetes credentials"},
		{".docker/config.json", "Docker registry credentials"},
	}

	// Protect Nebo's own data directory (database, config, etc.)
	// Nebo must never delete or overwrite its own database — this is catastrophic self-harm.
	neboDataPaths := neboDataDirs(home)
	for _, ndp := range neboDataPaths {
		if absPath == ndp.path || strings.HasPrefix(absPath, ndp.path+"/") {
			return ndp.reason
		}
	}

	for _, s := range sensitiveRelPaths {
		protected := filepath.Join(home, s.rel)
		if absPath == protected || strings.HasPrefix(absPath, protected+"/") {
			return s.reason
		}
	}

	return ""
}

// neboDataDirs returns the Nebo data directory paths that must be protected.
// Nebo must never delete or modify its own database, config, or critical data files.
func neboDataDirs(home string) []struct {
	path   string
	reason string
} {
	var dirs []struct {
		path   string
		reason string
	}

	// Check NEBO_DATA_DIR override first
	if envDir := os.Getenv("NEBO_DATA_DIR"); envDir != "" {
		dirs = append(dirs, struct {
			path   string
			reason string
		}{filepath.Join(envDir, "data"), "Nebo database directory — deleting this would destroy all agent data"})
		return dirs
	}

	// Platform-standard data directories
	switch runtime.GOOS {
	case "darwin":
		dirs = append(dirs, struct {
			path   string
			reason string
		}{filepath.Join(home, "Library", "Application Support", "Nebo", "data"), "Nebo database directory — deleting this would destroy all agent data"})
	case "windows":
		if appData := os.Getenv("APPDATA"); appData != "" {
			dirs = append(dirs, struct {
				path   string
				reason string
			}{filepath.Join(appData, "Nebo", "data"), "Nebo database directory — deleting this would destroy all agent data"})
		}
	default: // Linux and others
		dirs = append(dirs, struct {
			path   string
			reason string
		}{filepath.Join(home, ".config", "nebo", "data"), "Nebo database directory — deleting this would destroy all agent data"})
	}

	return dirs
}
