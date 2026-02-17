package keyring

import (
	"encoding/hex"
	"fmt"
	"os"

	zkr "github.com/zalando/go-keyring"
)

const (
	serviceName = "nebo"
	accountName = "master-encryption-key"
)

// Get retrieves the master encryption key from the OS keychain.
func Get() ([]byte, error) {
	hexKey, err := zkr.Get(serviceName, accountName)
	if err != nil {
		return nil, fmt.Errorf("keychain get: %w", err)
	}
	return hex.DecodeString(hexKey)
}

// Set stores the master encryption key in the OS keychain.
func Set(key []byte) error {
	return zkr.Set(serviceName, accountName, hex.EncodeToString(key))
}

// Delete removes the master encryption key from the OS keychain.
func Delete() error {
	return zkr.Delete(serviceName, accountName)
}

// Available returns true if the OS keychain is functional.
// Returns false if NEBO_KEYRING_DISABLED=1 is set (opt-in for headless/CI/Docker).
// Otherwise probes the keychain with a test write/read/delete cycle.
func Available() bool {
	if os.Getenv("NEBO_KEYRING_DISABLED") == "1" {
		return false
	}
	testService := "nebo-keyring-probe"
	testAccount := "probe"
	if err := zkr.Set(testService, testAccount, "ok"); err != nil {
		return false
	}
	_ = zkr.Delete(testService, testAccount)
	return true
}
