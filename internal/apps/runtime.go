package apps

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"sync"
	"time"

	pb "github.com/neboloop/nebo/internal/apps/pb"
	"github.com/neboloop/nebo/internal/apps/inspector"
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
	GatewayClient  pb.GatewayServiceClient
	ToolClient     pb.ToolServiceClient
	ChannelClient  pb.ChannelServiceClient
	CommClient     pb.CommServiceClient
	UIClient       pb.UIServiceClient
	ScheduleClient pb.ScheduleServiceClient

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
	keyProvider *SigningKeyProvider     // nil = skip signature verification (dev mode)
	revChecker  *RevocationChecker     // nil = skip revocation check (dev mode)
	inspector   *inspector.Inspector   // nil = no gRPC inspection
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

// findBinary locates the app executable in appDir.
// Search order: binary, app, then any executable in tmp/.
func FindBinary(appDir string) (string, error) {
	for _, name := range []string{"binary", "app"} {
		p := filepath.Join(appDir, name)
		if info, err := os.Stat(p); err == nil && !info.IsDir() {
			return p, nil
		}
	}

	tmpDir := filepath.Join(appDir, "tmp")
	entries, err := os.ReadDir(tmpDir)
	if err == nil {
		for _, e := range entries {
			if e.IsDir() {
				continue
			}
			p := filepath.Join(tmpDir, e.Name())
			info, err := os.Stat(p)
			if err != nil {
				continue
			}
			if info.Mode()&0111 != 0 {
				return p, nil
			}
		}
	}

	return "", fmt.Errorf("no binary found (expected 'binary', 'app', or executable in tmp/)")
}

// Launch starts an app binary, waits for its Unix socket, and connects via gRPC.
func (rt *Runtime) Launch(appDir string) (*AppProcess, error) {
	manifest, err := LoadManifest(appDir)
	if err != nil {
		return nil, fmt.Errorf("load manifest: %w", err)
	}

	binaryPath, err := FindBinary(appDir)
	if err != nil {
		return nil, err
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

	// Signature verification: skip for sideloaded dev apps (symlinks), verify for store apps
	if rt.keyProvider != nil {
		if info, lerr := os.Lstat(appDir); lerr == nil && info.Mode()&os.ModeSymlink != 0 {
			fmt.Printf("[apps] Skipping signature verification for sideloaded app: %s\n", manifest.ID)
		} else {
			key, err := rt.keyProvider.GetKey()
			if err != nil {
				key, err = rt.keyProvider.Refresh()
			}
			if err != nil {
				fmt.Printf("[apps] Warning: signature verification skipped for %s (signing key unavailable: %v)\n", manifest.ID, err)
			} else if err := VerifyAppSignatures(appDir, binaryPath, key); err != nil {
				return nil, fmt.Errorf("signature verification failed: %w", err)
			}
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
	setProcGroup(cmd)

	if err := cmd.Start(); err != nil {
		logCleanup()
		return nil, fmt.Errorf("start binary: %w", err)
	}

	// Write PID file so we can kill orphans on next startup if nebo dies hard
	_ = os.WriteFile(filepath.Join(appDir, ".pid"), []byte(strconv.Itoa(cmd.Process.Pid)), 0600)

	// Wait for socket to appear (exponential backoff, max 10s)
	if err := waitForSocket(sockPath, 10*time.Second); err != nil {
		killProcGroup(cmd)
		logCleanup()
		return nil, fmt.Errorf("app did not create socket: %w", err)
	}

	// Set socket permissions
	os.Chmod(sockPath, 0600)

	// Connect via gRPC over Unix domain socket
	dialOpts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
	}
	if rt.inspector != nil {
		dialOpts = append(dialOpts,
			grpc.WithUnaryInterceptor(inspector.UnaryInterceptor(rt.inspector, manifest.ID)),
			grpc.WithStreamInterceptor(inspector.StreamInterceptor(rt.inspector, manifest.ID)),
		)
	}
	conn, err := grpc.NewClient("unix://"+sockPath, dialOpts...)
	if err != nil {
		killProcGroup(cmd)
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
		case cap == CapSchedule:
			proc.ScheduleClient = pb.NewScheduleServiceClient(conn)
		}
	}

	// Health check
	if err := proc.HealthCheck(context.Background()); err != nil {
		proc.stop()
		return nil, fmt.Errorf("health check failed: %w", err)
	}

	rt.mu.Lock()
	if old, ok := rt.processes[manifest.ID]; ok {
		// Stop the old process to prevent zombie leaks (e.g., watcher re-launching on binary change)
		go old.stop()
	}
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

// IsRunning returns true if an app process exists for the given ID.
func (rt *Runtime) IsRunning(appID string) bool {
	rt.mu.RLock()
	defer rt.mu.RUnlock()
	_, ok := rt.processes[appID]
	return ok
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
		// Graceful shutdown with timeout, then force kill
		gracefulStopProc(p.cmd, 2*time.Second)
	}

	// Close per-app log files
	if p.logCleanup != nil {
		p.logCleanup()
	}

	// Clean up socket and PID file
	os.Remove(p.SockPath)
	os.Remove(filepath.Join(p.Dir, ".pid"))

	return nil
}

// cleanupStaleProcess kills an orphaned app process left over from a previous
// nebo run that died without cleaning up (SIGKILL, crash, air hot-reload).
// Reads the .pid file, checks if the process is still alive, kills it, and
// removes the .pid file. Safe to call even if no stale process exists.
func cleanupStaleProcess(appDir string) {
	pidFile := filepath.Join(appDir, ".pid")
	data, err := os.ReadFile(pidFile)
	if err != nil {
		return // No PID file — nothing to clean up
	}
	defer os.Remove(pidFile)

	pid, err := strconv.Atoi(string(data))
	if err != nil || pid <= 0 {
		return // Corrupt PID file
	}

	proc, err := os.FindProcess(pid)
	if err != nil {
		return
	}

	// Check if process is alive (signal 0 probe). If already dead, nothing to do.
	if !isProcessAlive(pid) {
		return
	}

	fmt.Printf("[apps] Killing orphaned process %d from %s\n", pid, filepath.Base(appDir))
	_ = proc.Signal(os.Interrupt)

	// Give it a moment to exit gracefully, then force kill
	done := make(chan struct{})
	go func() {
		proc.Wait()
		close(done)
	}()
	select {
	case <-done:
	case <-time.After(2 * time.Second):
		_ = proc.Kill()
	}
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
