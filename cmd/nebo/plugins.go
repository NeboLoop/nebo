package cli

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/cobra"

	agentcfg "github.com/nebolabs/nebo/internal/agent/config"
	"github.com/nebolabs/nebo/internal/agent/plugins"
	"github.com/nebolabs/nebo/internal/agent/tools"
)

// pluginsCmd creates the plugins management command
func PluginsCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "plugins",
		Short: "Manage external plugins",
		Long: `Plugins are external binaries that extend the agent with new tools and channels.
Plugins are loaded from the Nebo data directory's plugins/ folder or the extensions/plugins/ directory.`,
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "list",
		Short: "List all loaded plugins",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			listPlugins(cfg)
		},
	})

	return cmd
}

// listPlugins lists all loaded plugins
func listPlugins(cfg *agentcfg.Config) {
	loader := createPluginLoader(cfg)
	if err := loader.LoadAll(); err != nil {
		fmt.Fprintf(os.Stderr, "Error loading plugins: %v\n", err)
	}
	defer loader.Stop()

	toolPlugins := loader.ListTools()
	channels := loader.ListChannels()
	comms := loader.ListComms()

	if len(toolPlugins) == 0 && len(channels) == 0 && len(comms) == 0 {
		fmt.Println("No plugins loaded.")
		fmt.Printf("\nPlugins directory: %s\n", pluginsDir(cfg))
		fmt.Println("Place compiled plugin binaries in tools/, channels/, or comm/ subdirectories.")
		return
	}

	if len(toolPlugins) > 0 {
		fmt.Println("Tool plugins:")
		for _, name := range toolPlugins {
			tool, _ := loader.GetTool(name)
			fmt.Printf("  - %s: %s\n", tool.Name(), tool.Description())
		}
	}

	if len(channels) > 0 {
		fmt.Println("Channel plugins:")
		for _, id := range channels {
			fmt.Printf("  - %s\n", id)
		}
	}

	if len(comms) > 0 {
		fmt.Println("Comm plugins:")
		for _, name := range comms {
			cp, _ := loader.GetComm(name)
			fmt.Printf("  - %s (v%s)\n", cp.Name(), cp.Version())
		}
	}
}

func pluginsDir(cfg *agentcfg.Config) string {
	userDir := filepath.Join(cfg.DataDir, "plugins")
	if _, err := os.Stat(userDir); err == nil {
		return userDir
	}
	return "extensions/plugins"
}

func createPluginLoader(cfg *agentcfg.Config) *plugins.Loader {
	return plugins.NewLoader(pluginsDir(cfg))
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
