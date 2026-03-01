package apps

import (
	"context"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
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
	HookClient     pb.HookServiceClient

	cmd        *exec.Cmd
	conn       *grpc.ClientConn
	startedAt  time.Time
	logCleanup func() // closes per-app log files
	waitDone   chan struct{} // closed when cmd.Wait() returns (reaper goroutine)
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
	launchMu    sync.Map               // map[appID]*sync.Mutex — per-app launch serialization
	restarting  sync.Map               // map[appID]time.Time — suppresses watcher during managed restarts
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

// SuppressWatcher marks an app as "being restarted by supervisor/registry" so
// the file watcher won't fire a redundant restart. The suppression expires
// after the given duration as a safety net (prevents permanent suppression if
// the restart fails and nobody clears it).
func (rt *Runtime) SuppressWatcher(appID string, d time.Duration) {
	rt.restarting.Store(appID, time.Now().Add(d))
}

// ClearWatcherSuppression removes the watcher suppression for an app.
func (rt *Runtime) ClearWatcherSuppression(appID string) {
	rt.restarting.Delete(appID)
}

// IsWatcherSuppressed returns true if the watcher should skip this app
// because a managed restart is in progress.
func (rt *Runtime) IsWatcherSuppressed(appID string) bool {
	v, ok := rt.restarting.Load(appID)
	if !ok {
		return false
	}
	expiry := v.(time.Time)
	if time.Now().After(expiry) {
		// Expired — clean up and allow watcher
		rt.restarting.Delete(appID)
		return false
	}
	return true
}

// appLaunchMutex returns the per-app mutex for serializing launches.
// Guarantees at most ONE Launch() in flight per app ID — prevents the race
// where watcher, supervisor, and DiscoverAndLaunch all try to start the same
// app concurrently, resulting in duplicate processes.
func (rt *Runtime) appLaunchMutex(appID string) *sync.Mutex {
	v, _ := rt.launchMu.LoadOrStore(appID, &sync.Mutex{})
	return v.(*sync.Mutex)
}

// Launch starts an app binary, waits for its Unix socket, and connects via gRPC.
// Serialized per app ID — concurrent calls for the same app block until the
// first completes. Different apps launch in parallel.
func (rt *Runtime) Launch(appDir string) (*AppProcess, error) {
	manifest, err := LoadManifest(appDir)
	if err != nil {
		return nil, fmt.Errorf("load manifest: %w", err)
	}

	// Serialize launches for the same app. Without this, watcher + supervisor
	// can both call Launch() for the same app and spawn duplicate processes.
	mu := rt.appLaunchMutex(manifest.ID)
	mu.Lock()
	defer mu.Unlock()

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

	// Reaper goroutine: calls cmd.Wait() so the OS can reclaim the process
	// table entry immediately when the app exits. Without this, dead app
	// processes become zombies until someone else calls Wait().
	waitDone := make(chan struct{})
	go func() {
		defer close(waitDone)
		_ = cmd.Wait()
	}()

	// Wait for socket to appear (exponential backoff, configurable via manifest)
	startupTimeout := 10 * time.Second
	if manifest.StartupTimeout > 0 {
		startupTimeout = time.Duration(manifest.StartupTimeout) * time.Second
	}
	if err := waitForSocket(sockPath, startupTimeout); err != nil {
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
		waitDone:   waitDone,
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
		case cap == CapHooks:
			proc.HookClient = pb.NewHookServiceClient(conn)
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

	fmt.Printf("[apps] Launched %s v%s (PID %d, provides: %v)\n",
		manifest.Name, manifest.Version, cmd.Process.Pid, manifest.Provides)
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
		// Send SIGTERM to the entire process group
		killProcGroupTerm(p.cmd)

		// Wait for the reaper goroutine to confirm the process exited,
		// or force-kill after timeout. We NEVER call cmd.Wait() directly
		// here because the reaper goroutine already owns that call.
		if p.waitDone != nil {
			select {
			case <-p.waitDone:
				// Process exited cleanly after SIGTERM
			case <-time.After(2 * time.Second):
				// Still alive after SIGTERM — force kill the process group
				killProcGroup(p.cmd)
				<-p.waitDone // wait for reaper to finish
			}
		}
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

// cleanupStaleProcess kills orphaned app processes left over from a previous
// nebo run that died without cleaning up (SIGKILL, crash, air hot-reload).
//
// Strategy:
// 1. Read .pid file and kill that specific process if alive.
// 2. Scan the process table for any process running the app's binary path
//    that we didn't spawn — catches orphans whose .pid was overwritten.
//
// Safe to call even if no stale process exists.
func cleanupStaleProcess(appDir string) {
	pidFile := filepath.Join(appDir, ".pid")
	appID := filepath.Base(appDir)

	// Phase 1: Kill by PID file
	if data, err := os.ReadFile(pidFile); err == nil {
		os.Remove(pidFile)
		if pid, err := strconv.Atoi(strings.TrimSpace(string(data))); err == nil && pid > 0 {
			if isProcessAlive(pid) {
				killOrphan(pid, appID)
			}
		}
	}

	// Phase 2: Scan process table for any other instances of this binary
	// This catches orphans whose .pid was already overwritten by a restart
	binaryPath, err := FindBinary(appDir)
	if err != nil {
		return
	}
	killOrphansByBinary(binaryPath, appID)
}

// killOrphan gracefully kills a single orphaned process and its process group.
// Since we launch apps with Setpgid: true, the app PID == its PGID.
// Killing -PID sends the signal to the entire group, including children
// like sox/rec that the voice app spawns.
func killOrphan(pid int, appID string) {
	fmt.Printf("[apps] Killing orphaned process group %d from %s\n", pid, appID)
	killOrphanGroup(pid)
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
