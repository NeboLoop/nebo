package client

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/hex"
	"fmt"
	"os"
	"path/filepath"
)

// GetEncryptionKey returns the encryption key from environment or loads/generates a
// persistent key in the data directory so encrypted tokens survive restarts.
func GetEncryptionKey(dataDir string) ([]byte, error) {
	// Try environment variable first
	if key := os.Getenv("MCP_ENCRYPTION_KEY"); key != "" {
		decoded, err := hex.DecodeString(key)
		if err != nil {
			return nil, fmt.Errorf("invalid MCP_ENCRYPTION_KEY: must be hex encoded: %w", err)
		}
		if len(decoded) != 32 {
			return nil, fmt.Errorf("invalid MCP_ENCRYPTION_KEY: must be 32 bytes (256 bits)")
		}
		return decoded, nil
	}

	// Try JWT_SECRET as fallback (derive 32 bytes from it)
	if secret := os.Getenv("JWT_SECRET"); secret != "" {
		key := make([]byte, 32)
		copy(key, []byte(secret))
		return key, nil
	}

	// Load or generate a persistent key in the data directory
	keyFile := filepath.Join(dataDir, ".mcp-key")
	if data, err := os.ReadFile(keyFile); err == nil {
		decoded, err := hex.DecodeString(string(data))
		if err == nil && len(decoded) == 32 {
			return decoded, nil
		}
	}

	// Generate and persist a new key
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		return nil, fmt.Errorf("failed to generate encryption key: %w", err)
	}
	if err := os.WriteFile(keyFile, []byte(hex.EncodeToString(key)), 0600); err != nil {
		return nil, fmt.Errorf("failed to persist encryption key: %w", err)
	}
	return key, nil
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
