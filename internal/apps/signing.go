package apps

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"
)

// SignaturesFile represents the signatures.json from a .napp package.
// Contains all cryptographic signatures needed to verify the app.
type SignaturesFile struct {
	KeyID             string `json:"key_id"`
	Algorithm         string `json:"algorithm"`
	BinarySHA256      string `json:"binary_sha256"`
	BinarySignature   string `json:"binary_signature"`
	ManifestSignature string `json:"manifest_signature"`
}

// SigningKey represents a NeboLoop ED25519 public signing key.
type SigningKey struct {
	Algorithm string `json:"algorithm"`
	KeyID     string `json:"keyId"`
	PublicKey string `json:"publicKey"` // base64-encoded ED25519 public key
}

// signingClient is a shared HTTP client with a short timeout for signing/revocation checks.
var signingClient = &http.Client{Timeout: 5 * time.Second}

// SigningKeyProvider fetches and caches the NeboLoop signing key.
// Thread-safe. Caches for 24 hours, force-refreshable on verification failure.
type SigningKeyProvider struct {
	neboloopURL string
	key         *SigningKey
	fetchedAt   time.Time
	ttl         time.Duration
	mu          sync.RWMutex
}

// NewSigningKeyProvider creates a provider that fetches the signing key from NeboLoop.
func NewSigningKeyProvider(neboloopURL string) *SigningKeyProvider {
	return &SigningKeyProvider{
		neboloopURL: neboloopURL,
		ttl:         24 * time.Hour,
	}
}

// GetKey returns the cached signing key, fetching if expired or missing.
func (p *SigningKeyProvider) GetKey() (*SigningKey, error) {
	p.mu.RLock()
	if p.key != nil && time.Since(p.fetchedAt) < p.ttl {
		key := p.key
		p.mu.RUnlock()
		return key, nil
	}
	p.mu.RUnlock()
	return p.Refresh()
}

// Refresh force-fetches the signing key from NeboLoop.
func (p *SigningKeyProvider) Refresh() (*SigningKey, error) {
	p.mu.Lock()
	defer p.mu.Unlock()

	url := p.neboloopURL + "/api/v1/apps/signing-key"
	resp, err := signingClient.Get(url)
	if err != nil {
		return nil, fmt.Errorf("fetch signing key: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("fetch signing key: HTTP %d", resp.StatusCode)
	}

	// Limit response size to prevent abuse
	body, err := io.ReadAll(io.LimitReader(resp.Body, 64*1024))
	if err != nil {
		return nil, fmt.Errorf("read signing key response: %w", err)
	}

	var key SigningKey
	if err := json.Unmarshal(body, &key); err != nil {
		return nil, fmt.Errorf("parse signing key: %w", err)
	}

	if key.Algorithm != "ed25519" {
		return nil, fmt.Errorf("unsupported signing algorithm: %s", key.Algorithm)
	}

	p.key = &key
	p.fetchedAt = time.Now()
	return &key, nil
}

// LoadSignatures reads signatures.json from an app directory.
func LoadSignatures(appDir string) (*SignaturesFile, error) {
	path := filepath.Join(appDir, "signatures.json")
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("read signatures.json: %w", err)
	}

	var sigs SignaturesFile
	if err := json.Unmarshal(data, &sigs); err != nil {
		return nil, fmt.Errorf("parse signatures.json: %w", err)
	}

	if sigs.Algorithm != "ed25519" {
		return nil, fmt.Errorf("unsupported signature algorithm: %s", sigs.Algorithm)
	}

	if sigs.BinarySHA256 == "" || sigs.BinarySignature == "" || sigs.ManifestSignature == "" {
		return nil, fmt.Errorf("signatures.json missing required fields")
	}

	return &sigs, nil
}

// VerifyAppSignatures verifies both the manifest and binary signatures
// using the NeboLoop signing key.
//
// Verification steps:
// 1. Load signatures.json from app directory
// 2. Verify key ID matches the server's current key
// 3. Verify manifest signature: ed25519.Verify(pubKey, manifestBytes, manifestSig)
// 4. Verify binary SHA256 integrity
// 5. Verify binary signature: ed25519.Verify(pubKey, binaryBytes, binarySig)
func VerifyAppSignatures(appDir, binaryPath string, key *SigningKey) error {
	sigs, err := LoadSignatures(appDir)
	if err != nil {
		return err
	}

	// Verify key ID matches
	if sigs.KeyID != key.KeyID {
		return fmt.Errorf("key ID mismatch: signatures.json has %s, server has %s (possible key rotation)", sigs.KeyID, key.KeyID)
	}

	// Decode the public key
	pubKeyBytes, err := base64.StdEncoding.DecodeString(key.PublicKey)
	if err != nil {
		return fmt.Errorf("decode public key: %w", err)
	}
	if len(pubKeyBytes) != ed25519.PublicKeySize {
		return fmt.Errorf("invalid public key size: got %d, want %d", len(pubKeyBytes), ed25519.PublicKeySize)
	}
	pubKey := ed25519.PublicKey(pubKeyBytes)

	// Verify manifest signature
	manifestPath := filepath.Join(appDir, "manifest.json")
	manifestBytes, err := os.ReadFile(manifestPath)
	if err != nil {
		return fmt.Errorf("read manifest for verification: %w", err)
	}

	manifestSig, err := base64.StdEncoding.DecodeString(sigs.ManifestSignature)
	if err != nil {
		return fmt.Errorf("decode manifest signature: %w", err)
	}

	if !ed25519.Verify(pubKey, manifestBytes, manifestSig) {
		return fmt.Errorf("manifest signature verification failed — file may have been tampered with")
	}

	// Read binary once — used for both SHA256 check and signature verification
	binaryBytes, err := os.ReadFile(binaryPath)
	if err != nil {
		return fmt.Errorf("read binary for verification: %w", err)
	}

	// Verify binary SHA256 integrity
	actualHash := sha256.Sum256(binaryBytes)
	actualHashHex := hex.EncodeToString(actualHash[:])
	if actualHashHex != sigs.BinarySHA256 {
		return fmt.Errorf("binary SHA256 mismatch: expected %s, got %s — file corrupted or tampered", sigs.BinarySHA256, actualHashHex)
	}

	// Verify binary signature (over raw binary bytes, per NeboLoop spec)
	binarySig, err := base64.StdEncoding.DecodeString(sigs.BinarySignature)
	if err != nil {
		return fmt.Errorf("decode binary signature: %w", err)
	}

	if !ed25519.Verify(pubKey, binaryBytes, binarySig) {
		return fmt.Errorf("binary signature verification failed — file may have been tampered with")
	}

	return nil
}

// RevocationList is the response from NeboLoop's revocation endpoint.
type RevocationList struct {
	Revocations []RevocationEntry `json:"revocations"`
}

// RevocationEntry represents a revoked app.
type RevocationEntry struct {
	ID        string `json:"id"`
	Name      string `json:"name"`
	Slug      string `json:"slug"`
	Version   string `json:"version"`
	RevokedAt string `json:"revoked_at"`
}

// RevocationChecker checks if apps have been revoked by NeboLoop.
// Thread-safe. Caches the revocation list for 1 hour.
type RevocationChecker struct {
	neboloopURL string
	revoked     map[string]bool
	fetchedAt   time.Time
	ttl         time.Duration
	mu          sync.RWMutex
}

// NewRevocationChecker creates a checker that queries NeboLoop's revocation list.
func NewRevocationChecker(neboloopURL string) *RevocationChecker {
	return &RevocationChecker{
		neboloopURL: neboloopURL,
		revoked:     make(map[string]bool),
		ttl:         1 * time.Hour,
	}
}

// IsRevoked returns true if the given app ID has been revoked.
func (rc *RevocationChecker) IsRevoked(appID string) (bool, error) {
	rc.mu.RLock()
	if time.Since(rc.fetchedAt) < rc.ttl {
		revoked := rc.revoked[appID]
		rc.mu.RUnlock()
		return revoked, nil
	}
	rc.mu.RUnlock()

	if err := rc.refresh(); err != nil {
		return false, err
	}

	rc.mu.RLock()
	defer rc.mu.RUnlock()
	return rc.revoked[appID], nil
}

func (rc *RevocationChecker) refresh() error {
	rc.mu.Lock()
	defer rc.mu.Unlock()

	// Double-check after acquiring write lock
	if time.Since(rc.fetchedAt) < rc.ttl {
		return nil
	}

	url := rc.neboloopURL + "/api/v1/apps/revocations"
	resp, err := signingClient.Get(url)
	if err != nil {
		return fmt.Errorf("fetch revocation list: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("fetch revocation list: HTTP %d", resp.StatusCode)
	}

	// Limit response size
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1*1024*1024))
	if err != nil {
		return fmt.Errorf("read revocation list: %w", err)
	}

	var list RevocationList
	if err := json.Unmarshal(body, &list); err != nil {
		return fmt.Errorf("parse revocation list: %w", err)
	}

	revoked := make(map[string]bool, len(list.Revocations))
	for _, entry := range list.Revocations {
		revoked[entry.ID] = true
	}

	rc.revoked = revoked
	rc.fetchedAt = time.Now()
	return nil
}
