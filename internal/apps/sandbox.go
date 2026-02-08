package apps

import (
	"fmt"
	"io"
	"os"
	"path/filepath"
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
	"PATH":   true,
	"HOME":   true,
	"TMPDIR": true,
	"LANG":   true,
	"LC_ALL": true,
	"TZ":     true,
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
// and non-regular files (devices, pipes, etc.).
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

	return nil
}

// appLogWriter returns writers for an app's stdout and stderr.
// When LogToFile is true, output goes to {appDir}/logs/stdout.log and stderr.log
// instead of inheriting Nebo's stdout/stderr.
func appLogWriter(appDir string, cfg SandboxConfig) (stdout, stderr io.Writer, cleanup func(), err error) {
	if !cfg.LogToFile {
		return os.Stdout, os.Stderr, func() {}, nil
	}

	logDir := filepath.Join(appDir, "logs")
	if mkErr := os.MkdirAll(logDir, 0700); mkErr != nil {
		return nil, nil, nil, fmt.Errorf("create log dir: %w", mkErr)
	}

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

	cleanup = func() {
		outFile.Close()
		errFile.Close()
	}

	return outFile, errFile, cleanup, nil
}
