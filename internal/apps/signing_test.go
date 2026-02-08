package apps

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestLoadSignatures_Valid(t *testing.T) {
	dir := t.TempDir()
	sigs := SignaturesFile{
		KeyID:             "abc123",
		Algorithm:         "ed25519",
		BinarySHA256:      "deadbeef",
		BinarySignature:   "c2lnbmF0dXJl",
		ManifestSignature: "bWFuaWZlc3Q=",
	}
	data, _ := json.Marshal(sigs)
	os.WriteFile(filepath.Join(dir, "signatures.json"), data, 0644)

	loaded, err := LoadSignatures(dir)
	if err != nil {
		t.Fatalf("LoadSignatures() error = %v", err)
	}
	if loaded.KeyID != "abc123" {
		t.Errorf("KeyID = %q, want 'abc123'", loaded.KeyID)
	}
	if loaded.Algorithm != "ed25519" {
		t.Errorf("Algorithm = %q, want 'ed25519'", loaded.Algorithm)
	}
}

func TestLoadSignatures_MissingFile(t *testing.T) {
	dir := t.TempDir()
	_, err := LoadSignatures(dir)
	if err == nil {
		t.Fatal("expected error for missing signatures.json")
	}
}

func TestLoadSignatures_InvalidJSON(t *testing.T) {
	dir := t.TempDir()
	os.WriteFile(filepath.Join(dir, "signatures.json"), []byte("not json"), 0644)
	_, err := LoadSignatures(dir)
	if err == nil {
		t.Fatal("expected error for invalid JSON")
	}
}

func TestLoadSignatures_WrongAlgorithm(t *testing.T) {
	dir := t.TempDir()
	sigs := SignaturesFile{
		KeyID:             "abc",
		Algorithm:         "rsa",
		BinarySHA256:      "x",
		BinarySignature:   "y",
		ManifestSignature: "z",
	}
	data, _ := json.Marshal(sigs)
	os.WriteFile(filepath.Join(dir, "signatures.json"), data, 0644)

	_, err := LoadSignatures(dir)
	if err == nil {
		t.Fatal("expected error for unsupported algorithm")
	}
}

func TestLoadSignatures_MissingFields(t *testing.T) {
	dir := t.TempDir()
	sigs := SignaturesFile{
		KeyID:     "abc",
		Algorithm: "ed25519",
		// Missing BinarySHA256, BinarySignature, ManifestSignature
	}
	data, _ := json.Marshal(sigs)
	os.WriteFile(filepath.Join(dir, "signatures.json"), data, 0644)

	_, err := LoadSignatures(dir)
	if err == nil {
		t.Fatal("expected error for missing required fields")
	}
}

func TestVerifyAppSignatures_Valid(t *testing.T) {
	// Generate a real ED25519 keypair
	pub, priv, err := ed25519.GenerateKey(nil)
	if err != nil {
		t.Fatal(err)
	}

	appDir := t.TempDir()

	// Write manifest
	manifestContent := []byte(`{"id":"com.test.app","name":"Test","version":"1.0.0","provides":["gateway"]}`)
	os.WriteFile(filepath.Join(appDir, "manifest.json"), manifestContent, 0644)

	// Write binary
	binaryContent := []byte("#!/bin/sh\necho hello")
	binaryPath := filepath.Join(appDir, "binary")
	os.WriteFile(binaryPath, binaryContent, 0755)

	// Generate signatures
	manifestSig := ed25519.Sign(priv, manifestContent)
	binarySig := ed25519.Sign(priv, binaryContent)
	binaryHash := sha256.Sum256(binaryContent)

	pubKeyB64 := base64.StdEncoding.EncodeToString(pub)
	keyID := keyIDFromPub(pub)

	// Write signatures.json
	sigs := SignaturesFile{
		KeyID:             keyID,
		Algorithm:         "ed25519",
		BinarySHA256:      hex.EncodeToString(binaryHash[:]),
		BinarySignature:   base64.StdEncoding.EncodeToString(binarySig),
		ManifestSignature: base64.StdEncoding.EncodeToString(manifestSig),
	}
	sigsData, _ := json.Marshal(sigs)
	os.WriteFile(filepath.Join(appDir, "signatures.json"), sigsData, 0644)

	key := &SigningKey{
		Algorithm: "ed25519",
		KeyID:     keyID,
		PublicKey: pubKeyB64,
	}

	if err := VerifyAppSignatures(appDir, binaryPath, key); err != nil {
		t.Fatalf("VerifyAppSignatures() error = %v", err)
	}
}

func TestVerifyAppSignatures_TamperedManifest(t *testing.T) {
	pub, priv, _ := ed25519.GenerateKey(nil)
	appDir := t.TempDir()

	originalManifest := []byte(`{"id":"com.test.app","name":"Test","version":"1.0.0","provides":["gateway"]}`)
	manifestSig := ed25519.Sign(priv, originalManifest)

	// Write a DIFFERENT manifest (tampered)
	tamperedManifest := []byte(`{"id":"com.evil.app","name":"Evil","version":"1.0.0","provides":["gateway"]}`)
	os.WriteFile(filepath.Join(appDir, "manifest.json"), tamperedManifest, 0644)

	binaryContent := []byte("binary")
	binaryPath := filepath.Join(appDir, "binary")
	os.WriteFile(binaryPath, binaryContent, 0755)
	binarySig := ed25519.Sign(priv, binaryContent)
	binaryHash := sha256.Sum256(binaryContent)

	pubKeyB64 := base64.StdEncoding.EncodeToString(pub)
	keyID := keyIDFromPub(pub)

	sigs := SignaturesFile{
		KeyID:             keyID,
		Algorithm:         "ed25519",
		BinarySHA256:      hex.EncodeToString(binaryHash[:]),
		BinarySignature:   base64.StdEncoding.EncodeToString(binarySig),
		ManifestSignature: base64.StdEncoding.EncodeToString(manifestSig),
	}
	sigsData, _ := json.Marshal(sigs)
	os.WriteFile(filepath.Join(appDir, "signatures.json"), sigsData, 0644)

	key := &SigningKey{Algorithm: "ed25519", KeyID: keyID, PublicKey: pubKeyB64}

	err := VerifyAppSignatures(appDir, binaryPath, key)
	if err == nil {
		t.Fatal("expected error for tampered manifest")
	}
	if !contains(err.Error(), "manifest signature verification failed") {
		t.Errorf("error = %q, want containing 'manifest signature verification failed'", err.Error())
	}
}

func TestVerifyAppSignatures_TamperedBinary(t *testing.T) {
	pub, priv, _ := ed25519.GenerateKey(nil)
	appDir := t.TempDir()

	manifestContent := []byte(`{"id":"test"}`)
	os.WriteFile(filepath.Join(appDir, "manifest.json"), manifestContent, 0644)
	manifestSig := ed25519.Sign(priv, manifestContent)

	originalBinary := []byte("original binary")
	binarySig := ed25519.Sign(priv, originalBinary)
	binaryHash := sha256.Sum256(originalBinary)

	// Write a DIFFERENT binary (tampered)
	tamperedBinary := []byte("tampered binary!!")
	binaryPath := filepath.Join(appDir, "binary")
	os.WriteFile(binaryPath, tamperedBinary, 0755)

	pubKeyB64 := base64.StdEncoding.EncodeToString(pub)
	keyID := keyIDFromPub(pub)

	sigs := SignaturesFile{
		KeyID:             keyID,
		Algorithm:         "ed25519",
		BinarySHA256:      hex.EncodeToString(binaryHash[:]),
		BinarySignature:   base64.StdEncoding.EncodeToString(binarySig),
		ManifestSignature: base64.StdEncoding.EncodeToString(manifestSig),
	}
	sigsData, _ := json.Marshal(sigs)
	os.WriteFile(filepath.Join(appDir, "signatures.json"), sigsData, 0644)

	key := &SigningKey{Algorithm: "ed25519", KeyID: keyID, PublicKey: pubKeyB64}

	err := VerifyAppSignatures(appDir, binaryPath, key)
	if err == nil {
		t.Fatal("expected error for tampered binary")
	}
	if !contains(err.Error(), "SHA256 mismatch") {
		t.Errorf("error = %q, want containing 'SHA256 mismatch'", err.Error())
	}
}

func TestVerifyAppSignatures_KeyIDMismatch(t *testing.T) {
	pub, priv, _ := ed25519.GenerateKey(nil)
	appDir := t.TempDir()

	manifestContent := []byte(`{"id":"test"}`)
	os.WriteFile(filepath.Join(appDir, "manifest.json"), manifestContent, 0644)

	binaryContent := []byte("binary")
	binaryPath := filepath.Join(appDir, "binary")
	os.WriteFile(binaryPath, binaryContent, 0755)

	binaryHash := sha256.Sum256(binaryContent)
	manifestSig := ed25519.Sign(priv, manifestContent)
	binarySig := ed25519.Sign(priv, binaryContent)

	pubKeyB64 := base64.StdEncoding.EncodeToString(pub)

	sigs := SignaturesFile{
		KeyID:             "wrong-key-id",
		Algorithm:         "ed25519",
		BinarySHA256:      hex.EncodeToString(binaryHash[:]),
		BinarySignature:   base64.StdEncoding.EncodeToString(binarySig),
		ManifestSignature: base64.StdEncoding.EncodeToString(manifestSig),
	}
	sigsData, _ := json.Marshal(sigs)
	os.WriteFile(filepath.Join(appDir, "signatures.json"), sigsData, 0644)

	key := &SigningKey{Algorithm: "ed25519", KeyID: "server-key-id", PublicKey: pubKeyB64}

	err := VerifyAppSignatures(appDir, binaryPath, key)
	if err == nil {
		t.Fatal("expected error for key ID mismatch")
	}
	if !contains(err.Error(), "key ID mismatch") {
		t.Errorf("error = %q, want containing 'key ID mismatch'", err.Error())
	}
}

func keyIDFromPub(pub ed25519.PublicKey) string {
	h := sha256.Sum256(pub)
	return hex.EncodeToString(h[:4])
}
