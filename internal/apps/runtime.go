package apps

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"syscall"
	"time"

	pb "github.com/nebolabs/nebo/internal/apps/pb"
	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"
)

// AppProcess represents a running app instance.
type AppProcess struct {
	ID       string
	Dir      string
	Manifest *AppManifest
	SockPath string

	// Capability-specific gRPC clients (set based on manifest.provides)
	GatewayClient pb.GatewayServiceClient
	ToolClient    pb.ToolServiceClient
	ChannelClient pb.ChannelServiceClient
	CommClient    pb.CommServiceClient
	UIClient      pb.UIServiceClient

	cmd        *exec.Cmd
	conn       *grpc.ClientConn
	startedAt  time.Time
	logCleanup func() // closes per-app log files
	mu         sync.RWMutex
}

// Runtime manages app process lifecycles.
type Runtime struct {
	dataDir     string
	sandbox     SandboxConfig
	keyProvider *SigningKeyProvider  // nil = skip signature verification (dev mode)
	revChecker  *RevocationChecker  // nil = skip revocation check (dev mode)
	processes   map[string]*AppProcess
	mu          sync.RWMutex
}

// NewRuntime creates a new app runtime manager with the given sandbox config.
func NewRuntime(dataDir string, sandbox SandboxConfig) *Runtime {
	return &Runtime{
		dataDir:   dataDir,
		sandbox:   sandbox,
		processes: make(map[string]*AppProcess),
	}
}

// Launch starts an app binary, waits for its Unix socket, and connects via gRPC.
func (rt *Runtime) Launch(appDir string) (*AppProcess, error) {
	manifest, err := LoadManifest(appDir)
	if err != nil {
		return nil, fmt.Errorf("load manifest: %w", err)
	}

	binaryPath := filepath.Join(appDir, "binary")
	if _, err := os.Stat(binaryPath); err != nil {
		// Try platform-specific binary name
		binaryPath = filepath.Join(appDir, "app")
		if _, err := os.Stat(binaryPath); err != nil {
			return nil, fmt.Errorf("no binary found in %s", appDir)
		}
	}

	// Revocation check: refuse to launch apps revoked by NeboLoop
	if rt.revChecker != nil {
		revoked, err := rt.revChecker.IsRevoked(manifest.ID)
		if err != nil {
			fmt.Printf("[apps] Warning: revocation check failed for %s: %v\n", manifest.ID, err)
		} else if revoked {
			return nil, fmt.Errorf("app %s has been revoked by NeboLoop — refusing to launch", manifest.ID)
		}
	}

	// Signature verification: verify NeboLoop code-signed manifest + binary
	if rt.keyProvider != nil {
		key, err := rt.keyProvider.GetKey()
		if err != nil {
			// Try refresh in case of key rotation
			key, err = rt.keyProvider.Refresh()
			if err != nil {
				return nil, fmt.Errorf("cannot verify signatures — failed to fetch signing key: %w", err)
			}
		}
		if err := VerifyAppSignatures(appDir, binaryPath, key); err != nil {
			return nil, fmt.Errorf("signature verification failed: %w", err)
		}
	} else {
		fmt.Printf("[apps] Warning: no signing key provider — skipping signature verification for %s\n", manifest.ID)
	}

	// Pre-launch binary validation: reject symlinks, oversized binaries, non-executables
	if err := validateBinary(binaryPath, rt.sandbox); err != nil {
		return nil, fmt.Errorf("binary validation failed: %w", err)
	}

	sockPath := filepath.Join(appDir, "app.sock")

	// Clean up stale socket from previous run
	os.Remove(sockPath)

	// Create data directory for the app's sandboxed storage
	dataDir := filepath.Join(appDir, "data")
	os.MkdirAll(dataDir, 0700)

	// Set up per-app log files (prevents log injection into Nebo's stdout)
	stdout, stderr, logCleanup, err := appLogWriter(appDir, rt.sandbox)
	if err != nil {
		return nil, fmt.Errorf("setup app logging: %w", err)
	}

	// Start the binary with sandboxed environment
	cmd := exec.Command(binaryPath)
	cmd.Dir = appDir
	// Sanitized environment: only NEBO_APP_* and allowlisted system vars.
	// Prevents credential leakage (API keys, JWT secrets, tokens).
	cmd.Env = sanitizeEnv(manifest, appDir, sockPath)
	cmd.Stdout = stdout
	cmd.Stderr = stderr
	// Process group isolation: enables clean kill of app + all child processes
	cmd.SysProcAttr = &syscall.SysProcAttr{
		Setpgid: true,
	}

	if err := cmd.Start(); err != nil {
		logCleanup()
		return nil, fmt.Errorf("start binary: %w", err)
	}

	// Wait for socket to appear (exponential backoff, max 10s)
	if err := waitForSocket(sockPath, 10*time.Second); err != nil {
		_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
		logCleanup()
		return nil, fmt.Errorf("app did not create socket: %w", err)
	}

	// Set socket permissions
	os.Chmod(sockPath, 0600)

	// Connect via gRPC over Unix domain socket
	conn, err := grpc.NewClient(
		"unix://"+sockPath,
		grpc.WithTransportCredentials(insecure.NewCredentials()),
	)
	if err != nil {
		_ = syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
		logCleanup()
		return nil, fmt.Errorf("grpc dial: %w", err)
	}

	proc := &AppProcess{
		ID:         manifest.ID,
		Dir:        appDir,
		Manifest:   manifest,
		SockPath:   sockPath,
		cmd:        cmd,
		conn:       conn,
		startedAt:  time.Now(),
		logCleanup: logCleanup,
	}

	// Create capability-specific clients based on manifest.provides
	for _, cap := range manifest.Provides {
		switch {
		case cap == CapGateway:
			proc.GatewayClient = pb.NewGatewayServiceClient(conn)
		case cap == CapComm:
			proc.CommClient = pb.NewCommServiceClient(conn)
		case cap == CapVision || cap == CapBrowser || hasPrefix(cap, CapPrefixTool):
			proc.ToolClient = pb.NewToolServiceClient(conn)
		case hasPrefix(cap, CapPrefixChannel):
			proc.ChannelClient = pb.NewChannelServiceClient(conn)
		case cap == CapUI:
			proc.UIClient = pb.NewUIServiceClient(conn)
		}
	}

	// Health check
	if err := proc.HealthCheck(context.Background()); err != nil {
		proc.stop()
		return nil, fmt.Errorf("health check failed: %w", err)
	}

	rt.mu.Lock()
	rt.processes[manifest.ID] = proc
	rt.mu.Unlock()

	fmt.Printf("[apps] Launched %s v%s (provides: %v)\n", manifest.Name, manifest.Version, manifest.Provides)
	return proc, nil
}

// Stop gracefully stops an app by ID.
func (rt *Runtime) Stop(appID string) error {
	rt.mu.Lock()
	proc, ok := rt.processes[appID]
	if ok {
		delete(rt.processes, appID)
	}
	rt.mu.Unlock()

	if !ok {
		return fmt.Errorf("app not found: %s", appID)
	}

	return proc.stop()
}

// StopAll stops all running apps.
func (rt *Runtime) StopAll() error {
	rt.mu.Lock()
	procs := make([]*AppProcess, 0, len(rt.processes))
	for _, p := range rt.processes {
		procs = append(procs, p)
	}
	rt.processes = make(map[string]*AppProcess)
	rt.mu.Unlock()

	var lastErr error
	for _, p := range procs {
		if err := p.stop(); err != nil {
			lastErr = err
		}
	}
	return lastErr
}

// Get returns a running app process by ID.
func (rt *Runtime) Get(appID string) (*AppProcess, bool) {
	rt.mu.RLock()
	defer rt.mu.RUnlock()
	p, ok := rt.processes[appID]
	return p, ok
}

// List returns all running app IDs.
func (rt *Runtime) List() []string {
	rt.mu.RLock()
	defer rt.mu.RUnlock()
	ids := make([]string, 0, len(rt.processes))
	for id := range rt.processes {
		ids = append(ids, id)
	}
	return ids
}

// HealthCheck verifies an app is responsive via gRPC.
func (p *AppProcess) HealthCheck(ctx context.Context) error {
	p.mu.RLock()
	defer p.mu.RUnlock()

	// Try gateway health check first (most common for initial launch)
	if p.GatewayClient != nil {
		resp, err := p.GatewayClient.HealthCheck(ctx, &pb.HealthCheckRequest{})
		if err != nil {
			return fmt.Errorf("gateway health check: %w", err)
		}
		if !resp.Healthy {
			return fmt.Errorf("gateway reports unhealthy")
		}
		return nil
	}

	if p.ToolClient != nil {
		resp, err := p.ToolClient.HealthCheck(ctx, &pb.HealthCheckRequest{})
		if err != nil {
			return fmt.Errorf("tool health check: %w", err)
		}
		if !resp.Healthy {
			return fmt.Errorf("tool reports unhealthy")
		}
		return nil
	}

	if p.ChannelClient != nil {
		resp, err := p.ChannelClient.HealthCheck(ctx, &pb.HealthCheckRequest{})
		if err != nil {
			return fmt.Errorf("channel health check: %w", err)
		}
		if !resp.Healthy {
			return fmt.Errorf("channel reports unhealthy")
		}
		return nil
	}

	if p.CommClient != nil {
		resp, err := p.CommClient.HealthCheck(ctx, &pb.HealthCheckRequest{})
		if err != nil {
			return fmt.Errorf("comm health check: %w", err)
		}
		if !resp.Healthy {
			return fmt.Errorf("comm reports unhealthy")
		}
		return nil
	}

	if p.UIClient != nil {
		resp, err := p.UIClient.HealthCheck(ctx, &pb.HealthCheckRequest{})
		if err != nil {
			return fmt.Errorf("ui health check: %w", err)
		}
		if !resp.Healthy {
			return fmt.Errorf("ui reports unhealthy")
		}
		return nil
	}

	return fmt.Errorf("no capability client available for health check")
}

func (p *AppProcess) stop() error {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.conn != nil {
		p.conn.Close()
		p.conn = nil
	}

	if p.cmd != nil && p.cmd.Process != nil {
		// Graceful: SIGTERM the entire process group (kills child processes too)
		pid := p.cmd.Process.Pid
		_ = syscall.Kill(-pid, syscall.SIGTERM)

		// Wait up to 5 seconds
		done := make(chan error, 1)
		go func() { done <- p.cmd.Wait() }()

		select {
		case <-done:
		case <-time.After(5 * time.Second):
			// Force kill the entire process group
			_ = syscall.Kill(-pid, syscall.SIGKILL)
			<-done
		}
	}

	// Close per-app log files
	if p.logCleanup != nil {
		p.logCleanup()
	}

	// Clean up socket
	os.Remove(p.SockPath)

	return nil
}

func waitForSocket(path string, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	interval := 50 * time.Millisecond

	for time.Now().Before(deadline) {
		if _, err := os.Stat(path); err == nil {
			// Socket file exists — try connecting to verify it's ready
			conn, err := net.DialTimeout("unix", path, time.Second)
			if err == nil {
				conn.Close()
				return nil
			}
		}
		time.Sleep(interval)
		if interval < 500*time.Millisecond {
			interval = interval * 2
		}
	}
	return fmt.Errorf("timeout waiting for %s", path)
}

func hasPrefix(s, prefix string) bool {
	return len(s) > len(prefix) && s[:len(prefix)] == prefix
}
