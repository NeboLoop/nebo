package cli

import (
	"fmt"
	"os"
	"path/filepath"

	"github.com/spf13/cobra"

	agentcfg "nebo/agent/config"
	"nebo/agent/plugins"
)

// pluginsCmd creates the plugins management command
func PluginsCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "plugins",
		Short: "Manage external plugins",
		Long: `Plugins are external binaries that extend the agent with new tools and channels.
Plugins are loaded from ~/.nebo/plugins/ or the extensions/plugins/ directory.`,
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

	tools := loader.ListTools()
	channels := loader.ListChannels()

	if len(tools) == 0 && len(channels) == 0 {
		fmt.Println("No plugins loaded.")
		fmt.Printf("\nPlugins directory: %s\n", pluginsDir(cfg))
		fmt.Println("Place compiled plugin binaries in tools/ or channels/ subdirectories.")
		return
	}

	if len(tools) > 0 {
		fmt.Println("Tool plugins:")
		for _, name := range tools {
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
