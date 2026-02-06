package client

import (
	"crypto/rand"
	"testing"
)

func TestEncryptDecryptRoundtrip(t *testing.T) {
	// Generate a random key
	key := make([]byte, 32)
	if _, err := rand.Read(key); err != nil {
		t.Fatalf("failed to generate key: %v", err)
	}

	tests := []struct {
		name      string
		plaintext string
	}{
		{"empty string", ""},
		{"short string", "hello"},
		{"longer string", "this is a longer test string with some special characters: !@#$%^&*()"},
		{"unicode", "Hello 世界 🌍"},
		{"json-like", `{"access_token": "sk-abc123", "refresh_token": "rt-xyz789"}`},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			encrypted, err := EncryptString(tt.plaintext, key)
			if err != nil {
				t.Fatalf("encryption failed: %v", err)
			}

			// Empty string should return empty
			if tt.plaintext == "" {
				if encrypted != "" {
					t.Errorf("expected empty encrypted string for empty plaintext")
				}
				return
			}

			// Encrypted should be different from plaintext
			if encrypted == tt.plaintext {
				t.Errorf("encrypted string should differ from plaintext")
			}

			decrypted, err := DecryptString(encrypted, key)
			if err != nil {
				t.Fatalf("decryption failed: %v", err)
			}

			if decrypted != tt.plaintext {
				t.Errorf("roundtrip failed: got %q, want %q", decrypted, tt.plaintext)
			}
		})
	}
}

func TestDecryptWithWrongKey(t *testing.T) {
	key1 := make([]byte, 32)
	key2 := make([]byte, 32)
	rand.Read(key1)
	rand.Read(key2)

	plaintext := "secret data"
	encrypted, err := EncryptString(plaintext, key1)
	if err != nil {
		t.Fatalf("encryption failed: %v", err)
	}

	_, err = DecryptString(encrypted, key2)
	if err == nil {
		t.Error("expected decryption with wrong key to fail")
	}
}

func TestGeneratePKCE(t *testing.T) {
	verifier, challenge, err := GeneratePKCE()
	if err != nil {
		t.Fatalf("PKCE generation failed: %v", err)
	}

	// Verifier should be non-empty and base64url encoded
	if len(verifier) == 0 {
		t.Error("verifier should not be empty")
	}

	// Challenge should be non-empty and base64url encoded
	if len(challenge) == 0 {
		t.Error("challenge should not be empty")
	}

	// Challenge should differ from verifier (it's a hash)
	if challenge == verifier {
		t.Error("challenge should differ from verifier")
	}
}

func TestGenerateState(t *testing.T) {
	state1, err := GenerateState()
	if err != nil {
		t.Fatalf("state generation failed: %v", err)
	}

	state2, err := GenerateState()
	if err != nil {
		t.Fatalf("state generation failed: %v", err)
	}

	// Each state should be unique
	if state1 == state2 {
		t.Error("states should be unique")
	}

	// State should have reasonable length
	if len(state1) < 10 {
		t.Error("state should be at least 10 characters")
	}
}
