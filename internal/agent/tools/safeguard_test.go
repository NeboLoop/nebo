package tools

import (
	"encoding/json"
	"os"
	"runtime"
	"testing"
)

func TestCheckSafeguard_BlocksSudo(t *testing.T) {
	cases := []struct {
		cmd     string
		blocked bool
	}{
		{"sudo rm -rf /tmp/test", true},
		{"echo hello | sudo tee /tmp/test", true},
		{"ls && sudo apt install foo", true},
		{"ls; sudo reboot", true},
		{"$(sudo cat /etc/shadow)", true},
		{"`sudo whoami`", true},
		{"ls -la", false},
		{"git status", false},
		{"echo sudo is a word", false}, // "sudo" as argument, not command
	}

	for _, tc := range cases {
		input, _ := json.Marshal(map[string]string{
			"resource": "bash",
			"action":   "exec",
			"command":  tc.cmd,
		})
		err := CheckSafeguard("shell", input)
		if tc.blocked && err == nil {
			t.Errorf("expected command to be blocked: %s", tc.cmd)
		}
		if !tc.blocked && err != nil {
			t.Errorf("expected command to be allowed: %s, got: %v", tc.cmd, err)
		}
	}
}

func TestCheckSafeguard_BlocksSu(t *testing.T) {
	cases := []struct {
		cmd     string
		blocked bool
	}{
		{"su", true},
		{"su root", true},
		{"su -", true},
		{"echo | su root", true},
		{"summary", false},     // "su" is prefix but not the command
		{"suspend", false},     // same
		{"git submodule", false},
	}

	for _, tc := range cases {
		input, _ := json.Marshal(map[string]string{
			"resource": "bash",
			"action":   "exec",
			"command":  tc.cmd,
		})
		err := CheckSafeguard("shell", input)
		if tc.blocked && err == nil {
			t.Errorf("expected command to be blocked: %s", tc.cmd)
		}
		if !tc.blocked && err != nil {
			t.Errorf("expected command to be allowed: %s, got: %v", tc.cmd, err)
		}
	}
}

func TestCheckSafeguard_BlocksRootWipe(t *testing.T) {
	cases := []struct {
		cmd     string
		blocked bool
	}{
		{"rm -rf /", true},
		{"rm -fr /", true},
		{"rm -rf /*", true},
		{"rm -rf --no-preserve-root /", true},
		{"rm -rf /tmp/test", false}, // specific path, not root
	}

	for _, tc := range cases {
		input, _ := json.Marshal(map[string]string{
			"resource": "bash",
			"action":   "exec",
			"command":  tc.cmd,
		})
		err := CheckSafeguard("shell", input)
		if tc.blocked && err == nil {
			t.Errorf("expected command to be blocked: %s", tc.cmd)
		}
		if !tc.blocked && err != nil {
			t.Errorf("expected command to be allowed: %s, got: %v", tc.cmd, err)
		}
	}
}

func TestCheckSafeguard_BlocksSystemPathWrites(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("system path tests are platform-specific")
	}

	blockedPaths := []string{
		"/bin/sh",
		"/usr/bin/python",
		"/etc/passwd",
		"/sbin/init",
	}

	for _, path := range blockedPaths {
		input, _ := json.Marshal(map[string]string{
			"action":  "write",
			"path":    path,
			"content": "malicious",
		})
		err := CheckSafeguard("file", input)
		if err == nil {
			t.Errorf("expected file write to be blocked: %s", path)
		}
	}

	// Safe paths should be allowed
	safePaths := []string{
		"/tmp/test.txt",
		"/Users/test/project/main.go",
		"/home/user/code/app.py",
	}

	for _, path := range safePaths {
		input, _ := json.Marshal(map[string]string{
			"action":  "write",
			"path":    path,
			"content": "safe content",
		})
		err := CheckSafeguard("file", input)
		if err != nil {
			t.Errorf("expected file write to be allowed: %s, got: %v", path, err)
		}
	}
}

func TestCheckSafeguard_AllowsReads(t *testing.T) {
	// Reading system files should always be allowed
	input, _ := json.Marshal(map[string]string{
		"action": "read",
		"path":   "/etc/passwd",
	})
	err := CheckSafeguard("file", input)
	if err != nil {
		t.Errorf("expected file read to be allowed, got: %v", err)
	}
}

func TestCheckSafeguard_BlocksDd(t *testing.T) {
	input, _ := json.Marshal(map[string]string{
		"resource": "bash",
		"action":   "exec",
		"command":  "dd if=/dev/zero of=/dev/sda bs=1M",
	})
	err := CheckSafeguard("shell", input)
	if err == nil {
		t.Error("expected dd to block device to be blocked")
	}
}

func TestCheckSafeguard_BlocksDiskFormatting(t *testing.T) {
	blocked := []string{
		"mkfs.ext4 /dev/sda1",
		"mkfs -t xfs /dev/nvme0n1p1",
		"fdisk /dev/sda",
		"gdisk /dev/sda",
		"parted /dev/sda mklabel gpt",
		"sfdisk /dev/sda < layout.txt",
		"cfdisk /dev/sda",
		"wipefs -a /dev/sda",
		"sgdisk -Z /dev/sda",
		"diskutil eraseDisk JHFS+ Untitled /dev/disk2",
		"diskutil eraseVolume APFS Untitled /dev/disk2s1",
		"diskutil partitionDisk /dev/disk2 GPT JHFS+ Untitled 100%",
	}

	for _, cmd := range blocked {
		input, _ := json.Marshal(map[string]string{
			"resource": "bash",
			"action":   "exec",
			"command":  cmd,
		})
		err := CheckSafeguard("shell", input)
		if err == nil {
			t.Errorf("expected disk formatting command to be blocked: %s", cmd)
		}
	}
}

func TestCheckSafeguard_BlocksForkBomb(t *testing.T) {
	input, _ := json.Marshal(map[string]string{
		"resource": "bash",
		"action":   "exec",
		"command":  ":(){ :|:& };:",
	})
	err := CheckSafeguard("shell", input)
	if err == nil {
		t.Error("expected fork bomb to be blocked")
	}
}

func TestCheckSafeguard_BlocksSensitiveUserPaths(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("user path tests use Unix home dirs")
	}
	// Use absolute home path directly
	home, err := os.UserHomeDir()
	if err != nil {
		t.Skip("cannot determine home dir")
	}

	blocked := []string{
		home + "/.ssh/id_rsa",
		home + "/.ssh/authorized_keys",
		home + "/.gnupg/secring.gpg",
		home + "/.aws/credentials",
	}

	for _, path := range blocked {
		input, _ := json.Marshal(map[string]string{
			"action":  "write",
			"path":    path,
			"content": "malicious",
		})
		err := CheckSafeguard("file", input)
		if err == nil {
			t.Errorf("expected write to be blocked: %s", path)
		}
	}
}

func TestCheckSafeguard_BlocksNeboDatabase(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("test uses Unix paths")
	}
	home, err := os.UserHomeDir()
	if err != nil {
		t.Skip("cannot determine home dir")
	}

	// Determine expected Nebo data path
	var dbDir string
	if envDir := os.Getenv("NEBO_DATA_DIR"); envDir != "" {
		dbDir = envDir + "/data"
	} else if runtime.GOOS == "darwin" {
		dbDir = home + "/Library/Application Support/Nebo/data"
	} else {
		dbDir = home + "/.config/nebo/data"
	}

	blocked := []string{
		dbDir + "/nebo.db",
		dbDir + "/nebo.db-wal",
		dbDir + "/nebo.db-shm",
	}

	for _, path := range blocked {
		input, _ := json.Marshal(map[string]string{
			"action":  "write",
			"path":    path,
			"content": "malicious",
		})
		err := CheckSafeguard("file", input)
		if err == nil {
			t.Errorf("expected write to be blocked: %s", path)
		}
	}
}

func TestCheckSafeguard_AllowsNormalCommands(t *testing.T) {
	safe := []string{
		"ls -la",
		"git status",
		"go build ./...",
		"npm install",
		"cat /etc/hosts",
		"echo hello > /tmp/test.txt",
		"make build",
		"python script.py",
	}

	for _, cmd := range safe {
		input, _ := json.Marshal(map[string]string{
			"resource": "bash",
			"action":   "exec",
			"command":  cmd,
		})
		err := CheckSafeguard("shell", input)
		if err != nil {
			t.Errorf("expected safe command to be allowed: %s, got: %v", cmd, err)
		}
	}
}
