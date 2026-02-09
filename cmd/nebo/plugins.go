package cli

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"

	agentcfg "github.com/nebolabs/nebo/internal/agent/config"
	"github.com/nebolabs/nebo/internal/agent/tools"
	"github.com/nebolabs/nebo/internal/apps"
)

// AppsCmd creates the apps management command
func AppsCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "apps",
		Short: "Manage installed apps",
		Long:  `Apps extend Nebo with new tools, channels, gateways, and UI panels.`,
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "list",
		Short: "List installed apps",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			listApps(cfg)
		},
	})

	cmd.AddCommand(&cobra.Command{
		Use:   "uninstall [app-id]",
		Short: "Uninstall an app",
		Long:  `Stops a running app, removes its directory, and unregisters its capabilities.`,
		Args:  cobra.ExactArgs(1),
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			uninstallApp(cfg, args[0])
		},
	})

	return cmd
}

// listApps lists all installed apps by scanning the apps directory
func listApps(cfg *agentcfg.Config) {
	appsDir := filepath.Join(cfg.DataDir, "apps")
	entries, err := os.ReadDir(appsDir)
	if err != nil {
		if os.IsNotExist(err) {
			fmt.Println("No apps installed.")
			fmt.Printf("\nApps directory: %s\n", appsDir)
			return
		}
		fmt.Fprintf(os.Stderr, "Error reading apps directory: %v\n", err)
		return
	}

	var found int
	for _, entry := range entries {
		if !entry.IsDir() {
			continue
		}
		appDir := filepath.Join(appsDir, entry.Name())
		manifest, err := apps.LoadManifest(appDir)
		if err != nil {
			continue
		}
		found++
		fmt.Printf("  %s v%s\n", manifest.Name, manifest.Version)
		if manifest.Description != "" {
			fmt.Printf("    %s\n", manifest.Description)
		}
		fmt.Printf("    Provides: %s\n", strings.Join(manifest.Provides, ", "))
		if len(manifest.Permissions) > 0 {
			fmt.Printf("    Permissions: %s\n", strings.Join(manifest.Permissions, ", "))
		}
		fmt.Println()
	}

	if found == 0 {
		fmt.Println("No apps installed.")
		fmt.Printf("\nApps directory: %s\n", appsDir)
	} else {
		fmt.Printf("%d app(s) installed\n", found)
	}
}

// uninstallApp removes an installed app by its app ID (directory name).
func uninstallApp(cfg *agentcfg.Config, appID string) {
	appsDir := filepath.Join(cfg.DataDir, "apps")
	appDir := filepath.Join(appsDir, appID)

	if _, err := os.Stat(appDir); os.IsNotExist(err) {
		fmt.Fprintf(os.Stderr, "App not found: %s\n", appID)
		fmt.Fprintf(os.Stderr, "Use 'nebo apps list' to see installed apps.\n")
		os.Exit(1)
	}

	// Show what we're removing
	manifest, err := apps.LoadManifest(appDir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Warning: could not read manifest: %v\n", err)
	} else {
		fmt.Printf("Uninstalling %s v%s (%s)...\n", manifest.Name, manifest.Version, appID)
	}

	if err := os.RemoveAll(appDir); err != nil {
		fmt.Fprintf(os.Stderr, "Failed to remove app directory: %v\n", err)
		os.Exit(1)
	}

	// Also clean up any pending update artifacts
	os.RemoveAll(appDir + ".pending")
	os.RemoveAll(appDir + ".updating")

	fmt.Printf("Uninstalled %s\n", appID)
	fmt.Println("Note: if Nebo is running, the app will be stopped on the next health check cycle.")
}

// CapabilitiesCmd lists platform-specific built-in capabilities
func CapabilitiesCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "capabilities",
		Short: "List built-in capabilities for this platform",
		Run: func(cmd *cobra.Command, args []string) {
			listCapabilities()
		},
	}
}

func listCapabilities() {
	platform := tools.CurrentPlatform()
	caps := tools.ListCapabilities()

	fmt.Printf("Platform: %s\n", platform)
	fmt.Printf("Available capabilities: %d\n\n", len(caps))

	if len(caps) == 0 {
		fmt.Println("No platform-specific capabilities registered.")
		fmt.Println("Core tools (bash, read, write, etc.) are always available.")
		return
	}

	// Group by category
	byCategory := make(map[string][]*tools.Capability)
	for _, cap := range caps {
		cat := cap.Category
		if cat == "" {
			cat = "other"
		}
		byCategory[cat] = append(byCategory[cat], cap)
	}

	for category, caps := range byCategory {
		fmt.Printf("[%s]\n", category)
		for _, cap := range caps {
			setup := ""
			if cap.RequiresSetup {
				setup = " (requires setup)"
			}
			fmt.Printf("  • %s: %s%s\n", cap.Tool.Name(), cap.Tool.Description(), setup)
		}
		fmt.Println()
	}
}
