// Package defaults provides embedded default configuration files.
// These are copied to the platform data directory on first run or when reset is requested.
//
// Platform paths:
//
//	macOS:   ~/Library/Application Support/Nebo/
//	Windows: %AppData%\Nebo\
//	Linux:   ~/.config/nebo/
//
// Override with NEBO_DATA_DIR environment variable.
package defaults

import (
	"embed"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

//go:embed dotnebo/*
var defaultFiles embed.FS

// DataDir returns the platform-appropriate data directory.
//
//	macOS:   ~/Library/Application Support/Nebo/
//	Windows: %AppData%\Nebo\
//	Linux:   ~/.config/nebo/
//
// Set NEBO_DATA_DIR to override.
func DataDir() (string, error) {
	if dir := os.Getenv("NEBO_DATA_DIR"); dir != "" {
		return dir, nil
	}

	configDir, err := os.UserConfigDir()
	if err != nil {
		return "", fmt.Errorf("cannot determine config directory: %w", err)
	}

	// Linux: lowercase per XDG convention
	// macOS/Windows: title case per platform convention
	if runtime.GOOS == "linux" {
		return filepath.Join(configDir, "nebo"), nil
	}
	return filepath.Join(configDir, "Nebo"), nil
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
	return fs.WalkDir(defaultFiles, "dotnebo", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}

		// Skip the root directory
		if path == "dotnebo" {
			return nil
		}

		// Get relative path (strip "dotnebo/" prefix).
		// Use TrimPrefix instead of filepath.Rel because embed.FS always
		// uses forward slashes, but filepath.Rel produces backslashes on Windows.
		relPath := strings.TrimPrefix(path, "dotnebo/")
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
	return defaultFiles.ReadFile("dotnebo/" + name)
}

// ListDefaults returns the names of all default files.
func ListDefaults() ([]string, error) {
	var files []string
	err := fs.WalkDir(defaultFiles, "dotnebo", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if !d.IsDir() && path != "dotnebo" {
			// Use TrimPrefix to keep forward slashes (embed.FS convention).
			relPath := strings.TrimPrefix(path, "dotnebo/")
			files = append(files, relPath)
		}
		return nil
	})
	return files, err
}

// BotIDFile is the name of the file that persists the bot_id.
// This file is the source of truth for bot identity, surviving DB deletion.
const BotIDFile = "bot_id"

// ReadBotID reads the bot_id from <data_dir>/bot_id.
// Returns empty string on any failure or if the value is not a valid 36-char UUID.
func ReadBotID() string {
	dir, err := DataDir()
	if err != nil {
		return ""
	}
	data, err := os.ReadFile(filepath.Join(dir, BotIDFile))
	if err != nil {
		return ""
	}
	id := strings.TrimSpace(string(data))
	if len(id) != 36 {
		return ""
	}
	return id
}

// WriteBotID persists the bot_id to <data_dir>/bot_id with read-only permissions (0400).
// Removes any existing file first since 0400 prevents in-place overwrite.
func WriteBotID(id string) error {
	dir, err := DataDir()
	if err != nil {
		return err
	}
	path := filepath.Join(dir, BotIDFile)
	_ = os.Remove(path) // ignore error if file doesn't exist
	return os.WriteFile(path, []byte(id), 0400)
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
