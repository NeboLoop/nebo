package cli

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/spf13/cobra"
)

// doctorCmd creates the doctor command for health checks
func DoctorCmd() *cobra.Command {
	var fix bool

	cmd := &cobra.Command{
		Use:   "doctor",
		Short: "Check system health and diagnose issues",
		Long: `Run diagnostics on your GoBot installation.

Checks:
  - Configuration files
  - API key validity
  - Gateway connectivity
  - Database status
  - Channel connections
  - System resources

Examples:
  gobot doctor           # Run all diagnostics
  gobot doctor --fix     # Attempt to fix issues`,
		Run: func(cmd *cobra.Command, args []string) {
			runDoctor(fix)
		},
	}

	cmd.Flags().BoolVar(&fix, "fix", false, "Attempt to fix detected issues")

	return cmd
}

type checkResult struct {
	name    string
	status  string // "ok", "warn", "error"
	message string
}

func runDoctor(fix bool) {
	fmt.Println("\033[1m🔍 GoBot Doctor\033[0m")
	fmt.Println("================")
	fmt.Println()

	var results []checkResult

	// Check config
	results = append(results, checkConfig()...)

	// Check API keys
	results = append(results, checkAPIKeys()...)

	// Check Gateway
	results = append(results, checkGateway()...)

	// Check database
	results = append(results, checkDatabase()...)

	// Check system resources
	results = append(results, checkSystem()...)

	// Check channels
	results = append(results, checkChannels()...)

	// Print results
	fmt.Println()
	okCount := 0
	warnCount := 0
	errorCount := 0

	for _, r := range results {
		switch r.status {
		case "ok":
			fmt.Printf("\033[32m✓\033[0m %s: %s\n", r.name, r.message)
			okCount++
		case "warn":
			fmt.Printf("\033[33m⚠\033[0m %s: %s\n", r.name, r.message)
			warnCount++
		case "error":
			fmt.Printf("\033[31m✗\033[0m %s: %s\n", r.name, r.message)
			errorCount++
		}
	}

	// Summary
	fmt.Println()
	fmt.Println("Summary:")
	fmt.Printf("  \033[32m%d passed\033[0m", okCount)
	if warnCount > 0 {
		fmt.Printf("  \033[33m%d warnings\033[0m", warnCount)
	}
	if errorCount > 0 {
		fmt.Printf("  \033[31m%d errors\033[0m", errorCount)
	}
	fmt.Println()

	if errorCount > 0 && fix {
		fmt.Println()
		fmt.Println("Attempting fixes...")
		runFixes(results)
	}

	if errorCount > 0 {
		os.Exit(1)
	}
}

func checkConfig() []checkResult {
	var results []checkResult

	// Check ~/.nebo directory
	homeDir, _ := os.UserHomeDir()
	gobotDir := filepath.Join(homeDir, ".nebo")

	if _, err := os.Stat(gobotDir); os.IsNotExist(err) {
		results = append(results, checkResult{
			name:    "Config Directory",
			status:  "error",
			message: fmt.Sprintf("~/.nebo directory not found. Run 'gobot onboard' to create it."),
		})
	} else {
		results = append(results, checkResult{
			name:    "Config Directory",
			status:  "ok",
			message: gobotDir,
		})
	}

	// Check config.yaml
	configPath := filepath.Join(gobotDir, "config.yaml")
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		results = append(results, checkResult{
			name:    "Config File",
			status:  "error",
			message: "config.yaml not found",
		})
	} else {
		results = append(results, checkResult{
			name:    "Config File",
			status:  "ok",
			message: configPath,
		})
	}

	// Check server config
	if ServerConfig == nil {
		results = append(results, checkResult{
			name:    "Server Config",
			status:  "warn",
			message: "etc/gobot.yaml: not loaded",
		})
	} else {
		results = append(results, checkResult{
			name:    "Server Config",
			status:  "ok",
			message: "etc/gobot.yaml (embedded)",
		})
	}

	return results
}

func checkAPIKeys() []checkResult {
	var results []checkResult

	cfg := loadAgentConfig()
	if cfg == nil {
		results = append(results, checkResult{
			name:    "API Keys",
			status:  "warn",
			message: "Could not load config",
		})
		return results
	}

	if len(cfg.Providers) == 0 {
		results = append(results, checkResult{
			name:    "API Keys",
			status:  "error",
			message: "No providers configured",
		})
		return results
	}

	for _, p := range cfg.Providers {
		if p.APIKey == "" {
			results = append(results, checkResult{
				name:    fmt.Sprintf("Provider: %s", p.Name),
				status:  "error",
				message: "No API key configured",
			})
			continue
		}

		// Mask API key for display
		masked := maskKey(p.APIKey)
		results = append(results, checkResult{
			name:    fmt.Sprintf("Provider: %s", p.Name),
			status:  "ok",
			message: fmt.Sprintf("Key: %s, Model: %s", masked, p.Model),
		})
	}

	return results
}

func checkGateway() []checkResult {
	var results []checkResult

	gatewayURL := os.Getenv("GOBOT_GATEWAY_URL")
	if gatewayURL == "" {
		gatewayURL = "http://localhost:27895"
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	req, _ := http.NewRequestWithContext(ctx, "GET", gatewayURL+"/health", nil)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		results = append(results, checkResult{
			name:    "Gateway",
			status:  "warn",
			message: fmt.Sprintf("Not running at %s (start with 'gobot gateway')", gatewayURL),
		})
		return results
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusOK {
		results = append(results, checkResult{
			name:    "Gateway",
			status:  "ok",
			message: fmt.Sprintf("Running at %s", gatewayURL),
		})
	} else {
		results = append(results, checkResult{
			name:    "Gateway",
			status:  "warn",
			message: fmt.Sprintf("Unhealthy (status %d)", resp.StatusCode),
		})
	}

	return results
}

func checkDatabase() []checkResult {
	var results []checkResult

	homeDir, _ := os.UserHomeDir()
	dbPath := filepath.Join(homeDir, ".nebo", "gobot.db")

	if _, err := os.Stat(dbPath); os.IsNotExist(err) {
		results = append(results, checkResult{
			name:    "Database",
			status:  "warn",
			message: "Database not found (will be created on first run)",
		})
	} else {
		info, _ := os.Stat(dbPath)
		size := info.Size() / 1024 // KB
		results = append(results, checkResult{
			name:    "Database",
			status:  "ok",
			message: fmt.Sprintf("%s (%d KB)", dbPath, size),
		})
	}

	return results
}

func checkSystem() []checkResult {
	var results []checkResult

	// Check OS
	results = append(results, checkResult{
		name:    "Platform",
		status:  "ok",
		message: fmt.Sprintf("%s/%s", runtime.GOOS, runtime.GOARCH),
	})

	// Check for required tools
	tools := []string{"git", "curl"}
	for _, tool := range tools {
		if _, err := exec.LookPath(tool); err != nil {
			results = append(results, checkResult{
				name:    fmt.Sprintf("Tool: %s", tool),
				status:  "warn",
				message: "Not found in PATH",
			})
		} else {
			results = append(results, checkResult{
				name:    fmt.Sprintf("Tool: %s", tool),
				status:  "ok",
				message: "Found",
			})
		}
	}

	return results
}

func checkChannels() []checkResult {
	var results []checkResult

	gatewayURL := os.Getenv("GOBOT_GATEWAY_URL")
	if gatewayURL == "" {
		gatewayURL = "http://localhost:27895"
	}

	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()

	req, _ := http.NewRequestWithContext(ctx, "GET", gatewayURL+"/api/v1/channels", nil)
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		results = append(results, checkResult{
			name:    "Channels",
			status:  "warn",
			message: "Gateway not running (can't check channels)",
		})
		return results
	}
	defer resp.Body.Close()

	var result struct {
		Channels []struct {
			Type      string `json:"type"`
			ID        string `json:"id"`
			Name      string `json:"name"`
			Connected bool   `json:"connected"`
		} `json:"channels"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		results = append(results, checkResult{
			name:    "Channels",
			status:  "warn",
			message: "Could not parse response",
		})
		return results
	}

	if len(result.Channels) == 0 {
		results = append(results, checkResult{
			name:    "Channels",
			status:  "warn",
			message: "No channels configured (run 'gobot onboard')",
		})
		return results
	}

	connected := 0
	for _, ch := range result.Channels {
		if ch.Connected {
			connected++
		}
	}

	results = append(results, checkResult{
		name:    "Channels",
		status:  "ok",
		message: fmt.Sprintf("%d/%d connected", connected, len(result.Channels)),
	})

	return results
}

func maskKey(key string) string {
	if len(key) <= 8 {
		return "***"
	}
	return key[:4] + "..." + key[len(key)-4:]
}

func runFixes(results []checkResult) {
	for _, r := range results {
		if r.status != "error" {
			continue
		}

		switch {
		case strings.Contains(r.name, "Config Directory"):
			homeDir, _ := os.UserHomeDir()
			gobotDir := filepath.Join(homeDir, ".nebo")
			if err := os.MkdirAll(gobotDir, 0755); err != nil {
				fmt.Printf("  \033[31m✗\033[0m Could not create %s: %v\n", gobotDir, err)
			} else {
				fmt.Printf("  \033[32m✓\033[0m Created %s\n", gobotDir)
			}
		case strings.Contains(r.name, "Config File"):
			fmt.Println("  Run 'gobot onboard' to set up configuration")
		case strings.Contains(r.name, "API Keys"):
			fmt.Println("  Run 'gobot onboard' to configure API keys")
		}
	}
}
