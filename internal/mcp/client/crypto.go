package client

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"

	"github.com/neboloop/nebo/internal/keyring"
)

// GetEncryptionKey returns the encryption key, checking sources in priority order:
// 1. OS keychain (most secure — tied to user login session)
// 2. MCP_ENCRYPTION_KEY env var
// 3. JWT_SECRET env var
// 4. Persistent file at {dataDir}/.mcp-key
// 5. Generate new key
//
// When a key is found in env/file, it is automatically promoted to keychain
// (and the file deleted) so subsequent restarts use the keychain.
func GetEncryptionKey(dataDir string) ([]byte, error) {
	// 1. Try OS keychain first (most secure)
	if key, err := keyring.Get(); err == nil && len(key) == 32 {
		return key, nil
	}

	// 2. Try MCP_ENCRYPTION_KEY env var
	if envKey := os.Getenv("MCP_ENCRYPTION_KEY"); envKey != "" {
		decoded, err := hex.DecodeString(envKey)
		if err != nil {
			return nil, fmt.Errorf("invalid MCP_ENCRYPTION_KEY: must be hex encoded: %w", err)
		}
		if len(decoded) != 32 {
			return nil, fmt.Errorf("invalid MCP_ENCRYPTION_KEY: must be 32 bytes (256 bits)")
		}
		promoteToKeychain(decoded)
		return decoded, nil
	}

	// 3. Try JWT_SECRET as fallback (derive 32 bytes from it)
	if secret := os.Getenv("JWT_SECRET"); secret != "" {
		key := make([]byte, 32)
		copy(key, []byte(secret))
		promoteToKeychain(key)
		return key, nil
	}

	// 4. Load persistent key file
	keyFile := filepath.Join(dataDir, ".mcp-key")
	if data, err := os.ReadFile(keyFile); err == nil {
		decoded, err := hex.DecodeString(string(data))
		if err == nil && len(decoded) == 32 {
			// Migrate file-based key to keychain if possible
			if keyring.Available() {
				if err := keyring.Set(decoded); err == nil {
					_ = os.Remove(keyFile)
					slog.Info("Migrated encryption key from file to OS keychain")
				}
			}
			return decoded, nil
		}
	}

	// 5. Generate new key
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		return nil, fmt.Errorf("failed to generate encryption key: %w", err)
	}

	// Store in keychain if available, otherwise fall back to file
	if keyring.Available() {
		if err := keyring.Set(key); err == nil {
			slog.Info("Encryption key stored in OS keychain")
			return key, nil
		}
		slog.Warn("OS keychain available but store failed, falling back to file")
	}

	// File-based fallback (headless/CI/Docker)
	slog.Warn("No OS keychain available — encryption key stored in file (less secure)")
	if err := os.WriteFile(keyFile, []byte(hex.EncodeToString(key)), 0600); err != nil {
		return nil, fmt.Errorf("failed to persist encryption key: %w", err)
	}
	return key, nil
}

// promoteToKeychain migrates a key found in env vars to the OS keychain.
func promoteToKeychain(key []byte) {
	if keyring.Available() {
		if err := keyring.Set(key); err == nil {
			slog.Info("Promoted encryption key to OS keychain")
		}
	}
}

// EncryptString encrypts plaintext using AES-256-GCM
func EncryptString(plaintext string, key []byte) (string, error) {
	if len(plaintext) == 0 {
		return "", nil
	}

	block, err := aes.NewCipher(key)
	if err != nil {
		return "", fmt.Errorf("failed to create cipher: %w", err)
	}

	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", fmt.Errorf("failed to create GCM: %w", err)
	}

	nonce := make([]byte, gcm.NonceSize())
	if _, err := rand.Read(nonce); err != nil {
		return "", fmt.Errorf("failed to generate nonce: %w", err)
	}

	ciphertext := gcm.Seal(nonce, nonce, []byte(plaintext), nil)
	return hex.EncodeToString(ciphertext), nil
}

// DecryptString decrypts ciphertext using AES-256-GCM
func DecryptString(ciphertext string, key []byte) (string, error) {
	if len(ciphertext) == 0 {
		return "", nil
	}

	data, err := hex.DecodeString(ciphertext)
	if err != nil {
		return "", fmt.Errorf("failed to decode ciphertext: %w", err)
	}

	block, err := aes.NewCipher(key)
	if err != nil {
		return "", fmt.Errorf("failed to create cipher: %w", err)
	}

	gcm, err := cipher.NewGCM(block)
	if err != nil {
		return "", fmt.Errorf("failed to create GCM: %w", err)
	}

	nonceSize := gcm.NonceSize()
	if len(data) < nonceSize {
		return "", fmt.Errorf("ciphertext too short")
	}

	nonce, cipherdata := data[:nonceSize], data[nonceSize:]
	plaintext, err := gcm.Open(nil, nonce, cipherdata, nil)
	if err != nil {
		return "", fmt.Errorf("failed to decrypt: %w", err)
	}

	return string(plaintext), nil
}
