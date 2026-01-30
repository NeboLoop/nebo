// Package defaults provides embedded default configuration files.
// These are copied to ~/.nebo on first run or when reset is requested.
package defaults

import (
	"embed"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"time"
)

//go:embed dotgobot/*
var defaultFiles embed.FS

// DataDir returns the platform-appropriate data directory.
// Unix: ~/.nebo
// Windows: %APPDATA%\gobot or %USERPROFILE%\.nebo
func DataDir() (string, error) {
	if runtime.GOOS == "windows" {
		// Try APPDATA first, fall back to USERPROFILE
		if appData := os.Getenv("APPDATA"); appData != "" {
			return filepath.Join(appData, "gobot"), nil
		}
		if userProfile := os.Getenv("USERPROFILE"); userProfile != "" {
			return filepath.Join(userProfile, ".nebo"), nil
		}
		return "", fmt.Errorf("cannot determine data directory on Windows")
	}

	// Unix-like systems
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	return filepath.Join(home, ".nebo"), nil
}

// EnsureDataDir creates the data directory if it doesn't exist
// and copies default files if they're missing.
func EnsureDataDir() (string, error) {
	dir, err := DataDir()
	if err != nil {
		return "", err
	}

	// Create directory if needed
	if err := os.MkdirAll(dir, 0755); err != nil {
		return "", fmt.Errorf("failed to create data directory: %w", err)
	}

	// Copy default files if they don't exist
	if err := copyDefaults(dir, false); err != nil {
		return "", err
	}

	return dir, nil
}

// Reset removes existing config files and replaces them with defaults.
// Database and settings.json are preserved.
func Reset(dir string) error {
	return copyDefaults(dir, true)
}

// copyDefaults copies embedded default files to the data directory.
// If overwrite is true, existing files are replaced.
func copyDefaults(dir string, overwrite bool) error {
	return fs.WalkDir(defaultFiles, "dotgobot", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}

		// Skip the root directory
		if path == "dotgobot" {
			return nil
		}

		// Get relative path (strip "dotgobot/" prefix)
		relPath, _ := filepath.Rel("dotgobot", path)
		destPath := filepath.Join(dir, relPath)

		if d.IsDir() {
			return os.MkdirAll(destPath, 0755)
		}

		// Skip if file exists and we're not overwriting
		if !overwrite {
			if _, err := os.Stat(destPath); err == nil {
				return nil
			}
		}

		// Read embedded file
		data, err := defaultFiles.ReadFile(path)
		if err != nil {
			return fmt.Errorf("failed to read embedded %s: %w", path, err)
		}

		// Write to destination
		if err := os.WriteFile(destPath, data, 0644); err != nil {
			return fmt.Errorf("failed to write %s: %w", destPath, err)
		}

		return nil
	})
}

// GetDefault returns the content of a default file by name.
// Example: GetDefault("config.yaml")
func GetDefault(name string) ([]byte, error) {
	return defaultFiles.ReadFile(filepath.Join("dotgobot", name))
}

// ListDefaults returns the names of all default files.
func ListDefaults() ([]string, error) {
	var files []string
	err := fs.WalkDir(defaultFiles, "dotgobot", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if !d.IsDir() && path != "dotgobot" {
			relPath, _ := filepath.Rel("dotgobot", path)
			files = append(files, relPath)
		}
		return nil
	})
	return files, err
}

// SetupCompleteFile is the name of the file that marks setup as complete.
const SetupCompleteFile = ".setup-complete"

// IsSetupComplete checks if the setup has been marked as complete.
func IsSetupComplete() (bool, error) {
	dir, err := DataDir()
	if err != nil {
		return false, err
	}
	filePath := filepath.Join(dir, SetupCompleteFile)
	_, err = os.Stat(filePath)
	if os.IsNotExist(err) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return true, nil
}

// MarkSetupComplete creates the .setup-complete file with the current timestamp.
func MarkSetupComplete() error {
	dir, err := DataDir()
	if err != nil {
		return err
	}

	// Ensure directory exists
	if err := os.MkdirAll(dir, 0755); err != nil {
		return fmt.Errorf("failed to create data directory: %w", err)
	}

	filePath := filepath.Join(dir, SetupCompleteFile)
	timestamp := fmt.Sprintf("%d", time.Now().Unix())
	if err := os.WriteFile(filePath, []byte(timestamp), 0644); err != nil {
		return fmt.Errorf("failed to write setup complete file: %w", err)
	}
	return nil
}
