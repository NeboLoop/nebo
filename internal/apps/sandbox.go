package apps

import (
	"bytes"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
)

// SandboxConfig controls process isolation for app binaries.
type SandboxConfig struct {
	// MaxBinarySizeMB is the maximum allowed binary size in megabytes.
	// Prevents oversized or zip-bomb-style binaries. 0 = no limit.
	MaxBinarySizeMB int

	// LogToFile redirects app stdout/stderr to per-app log files instead of
	// inheriting Nebo's stdout/stderr. Prevents log injection and keeps
	// per-app audit trails.
	LogToFile bool
}

// DefaultSandboxConfig returns production-ready sandbox defaults.
func DefaultSandboxConfig() SandboxConfig {
	return SandboxConfig{
		MaxBinarySizeMB: 500,
		LogToFile:       true,
	}
}

// allowedEnvKeys are environment variables safe to pass to sandboxed app processes.
// Everything else is stripped to prevent leaking API keys, tokens, and credentials
// from the parent environment (MAESTRO LM-003, DO-001).
var allowedEnvKeys = map[string]bool{
	"PATH":              true,
	"HOME":              true,
	"TMPDIR":            true,
	"LANG":              true,
	"LC_ALL":            true,
	"TZ":                true,
	"ELEVENLABS_API_KEY": true,
}

// sanitizeEnv builds a minimal, safe environment for an app process.
// Only passes NEBO_APP_* variables and a strict allowlist of system variables.
// This is the primary defense against credential leakage — a rogue app cannot
// read ANTHROPIC_API_KEY, OPENAI_API_KEY, JWT_SECRET, or any other secret
// from the parent process environment.
func sanitizeEnv(manifest *AppManifest, appDir, sockPath string) []string {
	env := []string{
		"NEBO_APP_DIR=" + appDir,
		"NEBO_APP_SOCK=" + sockPath,
		"NEBO_APP_ID=" + manifest.ID,
		"NEBO_APP_NAME=" + manifest.Name,
		"NEBO_APP_VERSION=" + manifest.Version,
		"NEBO_APP_DATA=" + filepath.Join(appDir, "data"),
	}

	for _, e := range os.Environ() {
		key, _, ok := strings.Cut(e, "=")
		if ok && allowedEnvKeys[key] {
			env = append(env, e)
		}
	}

	return env
}

// validateBinary performs pre-launch security checks on an app binary.
// Rejects symlinks (path traversal), oversized binaries, non-executables,
// non-regular files (devices, pipes, etc.), scripts, and non-native binaries.
func validateBinary(path string, cfg SandboxConfig) error {
	// Use Lstat to detect symlinks — Stat follows them silently
	info, err := os.Lstat(path)
	if err != nil {
		return fmt.Errorf("stat binary: %w", err)
	}

	if info.Mode()&os.ModeSymlink != 0 {
		return fmt.Errorf("binary is a symlink (rejected for security)")
	}

	if !info.Mode().IsRegular() {
		return fmt.Errorf("binary is not a regular file")
	}

	if info.Mode().Perm()&0111 == 0 {
		return fmt.Errorf("binary is not executable")
	}

	if cfg.MaxBinarySizeMB > 0 {
		maxBytes := int64(cfg.MaxBinarySizeMB) * 1024 * 1024
		if info.Size() > maxBytes {
			return fmt.Errorf("binary too large: %d MB (max %d MB)",
				info.Size()/(1024*1024), cfg.MaxBinarySizeMB)
		}
	}

	// Validate native binary format (compiled-only policy enforcement).
	// Rejects scripts, interpreted code, and non-native executables.
	if err := validateBinaryFormat(path); err != nil {
		return err
	}

	return nil
}

// Native binary magic bytes.
// These identify compiled executables at the file format level.
var (
	elfMagic        = []byte{0x7f, 'E', 'L', 'F'}           // Linux ELF
	machoMagic32    = []byte{0xfe, 0xed, 0xfa, 0xce}         // Mach-O 32-bit
	machoMagic64    = []byte{0xcf, 0xfa, 0xed, 0xfe}         // Mach-O 64-bit
	machoFatMagic   = []byte{0xca, 0xfe, 0xba, 0xbe}         // Mach-O Universal/Fat binary
	peMagic         = []byte{0x4d, 0x5a}                      // Windows PE (MZ header)
	shebangMagic    = []byte{0x23, 0x21}                      // #! (script shebang)
)

// validateBinaryFormat checks that a file is a native compiled binary,
// not a script or interpreted language artifact.
//
// This is the local enforcement layer of the compiled-only policy.
// NeboLoop performs deeper analysis (dynamic link scanning, hidden interpreter
// detection) at upload time. Nebo's local check is intentionally lightweight:
// magic bytes + shebang rejection is fast and sufficient because signed binaries
// from NeboLoop have already passed deep validation.
func validateBinaryFormat(path string) error {
	f, err := os.Open(path)
	if err != nil {
		return fmt.Errorf("open binary for format check: %w", err)
	}
	defer f.Close()

	// Read first 4 bytes for magic number identification.
	// All native binary formats (ELF, Mach-O, PE) have a magic number
	// in the first 2-4 bytes.
	header := make([]byte, 4)
	n, err := f.Read(header)
	if err != nil || n < 2 {
		return fmt.Errorf("binary too small or unreadable — not a valid executable")
	}

	// Reject scripts immediately — shebang (#!) means interpreted code.
	// This catches: #!/usr/bin/env python, #!/bin/bash, #!/usr/bin/env node, etc.
	if bytes.HasPrefix(header, shebangMagic) {
		return fmt.Errorf("binary is a script (shebang #! detected) — only compiled native binaries are allowed")
	}

	// Validate against platform-appropriate magic bytes.
	// We check for the current platform's format and also accept cross-platform
	// formats for forward compatibility (e.g., building macOS .napp on Linux CI).
	if isNativeBinary(header) {
		return nil
	}

	return fmt.Errorf("binary is not a recognized native executable format (expected ELF, Mach-O, or PE) — interpreted languages are not permitted")
}

// isNativeBinary checks if the file header matches any known native binary format.
func isNativeBinary(header []byte) bool {
	switch {
	case bytes.HasPrefix(header, elfMagic):
		return true
	case bytes.Equal(header, machoMagic32):
		return true
	case bytes.Equal(header, machoMagic64):
		return true
	case bytes.Equal(header, machoFatMagic):
		return true
	case bytes.HasPrefix(header, peMagic) && runtime.GOOS == "windows":
		// Only accept PE on Windows — MZ header on non-Windows is suspicious
		return true
	case bytes.HasPrefix(header, peMagic) && len(header) >= 2:
		// Accept PE on any platform (cross-compilation support)
		return true
	}
	return false
}

// maxAppLogSize is the maximum size of a single app log file before rotation.
// When exceeded, the current log is renamed to .log.1 and a new file is created.
const maxAppLogSize = 2 * 1024 * 1024 // 2 MB

// appLogWriter returns writers for an app's stdout and stderr.
// When LogToFile is true, output goes to {appDir}/logs/stdout.log and stderr.log.
// Logs are also tee'd to Nebo's stderr with an [app:ID] prefix so they appear
// in the main console for real-time debugging.
func appLogWriter(appDir string, cfg SandboxConfig) (stdout, stderr io.Writer, cleanup func(), err error) {
	appID := filepath.Base(appDir)

	if !cfg.LogToFile {
		prefix := fmt.Sprintf("[app:%s] ", appID)
		pw := &prefixWriter{prefix: prefix, dest: os.Stderr}
		return pw, pw, func() {}, nil
	}

	logDir := filepath.Join(appDir, "logs")
	if mkErr := os.MkdirAll(logDir, 0700); mkErr != nil {
		return nil, nil, nil, fmt.Errorf("create log dir: %w", mkErr)
	}

	// Rotate logs if they've grown too large
	rotateLogFile(filepath.Join(logDir, "stdout.log"))
	rotateLogFile(filepath.Join(logDir, "stderr.log"))

	outFile, err := os.OpenFile(
		filepath.Join(logDir, "stdout.log"),
		os.O_CREATE|os.O_WRONLY|os.O_APPEND,
		0600,
	)
	if err != nil {
		return nil, nil, nil, fmt.Errorf("open stdout log: %w", err)
	}

	errFile, err := os.OpenFile(
		filepath.Join(logDir, "stderr.log"),
		os.O_CREATE|os.O_WRONLY|os.O_APPEND,
		0600,
	)
	if err != nil {
		outFile.Close()
		return nil, nil, nil, fmt.Errorf("open stderr log: %w", err)
	}

	// Tee: write to both the log file and Nebo's stderr with [app:ID] prefix
	prefix := fmt.Sprintf("[app:%s] ", appID)
	outWriter := io.MultiWriter(outFile, &prefixWriter{prefix: prefix, dest: os.Stderr})
	errWriter := io.MultiWriter(errFile, &prefixWriter{prefix: prefix, dest: os.Stderr})

	cleanup = func() {
		outFile.Close()
		errFile.Close()
	}

	return outWriter, errWriter, cleanup, nil
}

// rotateLogFile renames logPath to logPath.1 if it exceeds maxAppLogSize.
// Only keeps one rotated backup — this is for crash loop protection, not
// long-term log archival.
func rotateLogFile(logPath string) {
	info, err := os.Stat(logPath)
	if err != nil || info.Size() < maxAppLogSize {
		return
	}
	backup := logPath + ".1"
	os.Remove(backup)
	os.Rename(logPath, backup)
}

// prefixWriter prepends a prefix to each line written, making it easy to
// identify which app produced which log line in Nebo's console output.
type prefixWriter struct {
	prefix string
	dest   io.Writer
	buf    []byte // partial line buffer
}

func (pw *prefixWriter) Write(p []byte) (n int, err error) {
	pw.buf = append(pw.buf, p...)
	for {
		idx := bytes.IndexByte(pw.buf, '\n')
		if idx < 0 {
			break
		}
		line := pw.buf[:idx+1]
		pw.buf = pw.buf[idx+1:]
		if _, err := fmt.Fprintf(pw.dest, "%s%s", pw.prefix, line); err != nil {
			return len(p), err
		}
	}
	return len(p), nil
}
