package updater

import (
	"fmt"
	"io"
	"os"
	"os/exec"
	"time"
)

// healthCheck runs "nebo --version" on the new binary with a timeout.
func healthCheck(binaryPath string) error {
	cmd := exec.Command(binaryPath, "--version")
	cmd.Env = os.Environ()
	done := make(chan error, 1)
	go func() { done <- cmd.Run() }()
	select {
	case err := <-done:
		if err != nil {
			return fmt.Errorf("updater: health check failed: %w", err)
		}
		return nil
	case <-time.After(5 * time.Second):
		_ = cmd.Process.Kill()
		return fmt.Errorf("updater: health check timed out")
	}
}

// copyFile copies src to dst, preserving permissions.
func copyFile(src, dst string) error {
	srcInfo, err := os.Stat(src)
	if err != nil {
		return err
	}

	in, err := os.Open(src)
	if err != nil {
		return err
	}
	defer in.Close()

	out, err := os.OpenFile(dst, os.O_CREATE|os.O_WRONLY|os.O_TRUNC, srcInfo.Mode())
	if err != nil {
		return err
	}
	defer out.Close()

	_, err = io.Copy(out, in)
	return err
}
