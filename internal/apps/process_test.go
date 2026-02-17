//go:build !windows

package apps

import (
	"os"
	"os/exec"
	"testing"
	"time"
)

func TestIsProcessAlive_Self(t *testing.T) {
	// Our own PID should be alive
	if !isProcessAlive(os.Getpid()) {
		t.Error("current process should be alive")
	}
}

func TestIsProcessAlive_Dead(t *testing.T) {
	// PID 999999999 should not exist
	if isProcessAlive(999999999) {
		t.Error("PID 999999999 should not be alive")
	}
}

func TestSetProcGroup(t *testing.T) {
	cmd := exec.Command("sleep", "0")
	setProcGroup(cmd)

	if cmd.SysProcAttr == nil {
		t.Fatal("SysProcAttr should be set")
	}
	if !cmd.SysProcAttr.Setpgid {
		t.Error("Setpgid should be true")
	}
}

func TestKillProcGroup_NilProcess(t *testing.T) {
	// Should not panic with nil process
	cmd := &exec.Cmd{}
	killProcGroup(cmd)
	killProcGroupTerm(cmd)
}

func TestKillProcGroup_KillsEntireGroup(t *testing.T) {
	// Start a shell that spawns a child in the same process group.
	// bash -c "sleep 60" creates: bash (group leader) → sleep (child).
	cmd := exec.Command("bash", "-c", "sleep 60")
	setProcGroup(cmd)
	if err := cmd.Start(); err != nil {
		t.Fatalf("failed to start process: %v", err)
	}
	pid := cmd.Process.Pid

	// Give the process group a moment to fully establish
	time.Sleep(50 * time.Millisecond)

	if !isProcessAlive(pid) {
		t.Fatal("process should be alive after start")
	}

	// Kill the process group
	killProcGroup(cmd)

	// Wait for signal delivery + reap
	done := make(chan struct{})
	go func() {
		cmd.Wait()
		close(done)
	}()

	select {
	case <-done:
		// good — process exited
	case <-time.After(2 * time.Second):
		t.Error("process should be dead after killProcGroup")
		cmd.Process.Kill()
		<-done
	}
}

func TestKillProcGroupTerm_GracefulShutdown(t *testing.T) {
	// Start a process in its own group
	cmd := exec.Command("bash", "-c", "sleep 60")
	setProcGroup(cmd)
	if err := cmd.Start(); err != nil {
		t.Fatalf("failed to start process: %v", err)
	}

	// Give the process group a moment to fully establish
	time.Sleep(50 * time.Millisecond)

	// Send SIGTERM to the group
	killProcGroupTerm(cmd)

	// Wait for exit
	done := make(chan struct{})
	go func() {
		cmd.Wait()
		close(done)
	}()

	select {
	case <-done:
		// good — process exited from SIGTERM
	case <-time.After(2 * time.Second):
		t.Error("process should be dead after SIGTERM")
		cmd.Process.Kill()
		<-done
	}
}

func TestKillOrphanGroup_FullCleanup(t *testing.T) {
	// Start a process in its own group (simulates orphan)
	cmd := exec.Command("sleep", "60")
	setProcGroup(cmd)
	if err := cmd.Start(); err != nil {
		t.Fatalf("failed to start process: %v", err)
	}
	pid := cmd.Process.Pid

	// Kill it as an orphan
	killOrphanGroup(pid)

	// Should be dead
	if isProcessAlive(pid) {
		t.Error("orphan should be dead after killOrphanGroup")
		cmd.Process.Kill()
	}

	cmd.Wait()
}
