package credential

import (
	"strings"
	"sync"

	mcpclient "github.com/neboloop/nebo/internal/mcp/client"
)

const encPrefix = "enc:"

var (
	encKey []byte
	mu     sync.RWMutex
)

// Init sets the master encryption key. Called once from ServiceContext at startup.
func Init(key []byte) {
	mu.Lock()
	defer mu.Unlock()
	encKey = key
}

// Encrypt encrypts a plaintext string and prepends the "enc:" prefix.
// Returns empty string for empty input.
func Encrypt(plaintext string) (string, error) {
	if plaintext == "" {
		return "", nil
	}
	mu.RLock()
	k := encKey
	mu.RUnlock()

	ct, err := mcpclient.EncryptString(plaintext, k)
	if err != nil {
		return "", err
	}
	return encPrefix + ct, nil
}

// Decrypt decrypts a ciphertext string. Handles both "enc:"-prefixed values
// and legacy non-prefixed ciphertext (from app_oauth_grants migration window).
// Returns empty string for empty input.
func Decrypt(ciphertext string) (string, error) {
	if ciphertext == "" {
		return "", nil
	}
	mu.RLock()
	k := encKey
	mu.RUnlock()

	raw := strings.TrimPrefix(ciphertext, encPrefix)
	return mcpclient.DecryptString(raw, k)
}

// IsEncrypted returns true if the value has the "enc:" prefix.
func IsEncrypted(s string) bool {
	return strings.HasPrefix(s, encPrefix)
}
