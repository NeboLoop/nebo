//go:build windows

package apps

// killOrphansByBinary is a no-op on Windows.
// Windows doesn't reparent orphaned processes to PID 1 the way Unix does,
// and process lifecycle is handled differently.
func killOrphansByBinary(binaryPath, appID string) {
	// TODO: Implement using tasklist/taskkill if needed
}
