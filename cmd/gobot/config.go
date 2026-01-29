package cli

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"

	"gobot/agent/ai"
	agentcfg "gobot/agent/config"
)

// configCmd creates the config command
func ConfigCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "config",
		Short: "Show or manage configuration",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			showConfig(cfg)
		},
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "init",
		Short: "Initialize configuration file",
		Run: func(cmd *cobra.Command, args []string) {
			initConfig()
		},
	})

	return cmd
}

// loadAgentConfig loads the agent configuration
func loadAgentConfig() *agentcfg.Config {
	var cfg *agentcfg.Config
	var err error

	if cfgFile != "" {
		cfg, err = agentcfg.LoadFrom(cfgFile)
	} else {
		cfg, err = agentcfg.Load()
	}

	if err != nil {
		fmt.Fprintf(os.Stderr, "Error loading config: %v\n", err)
		os.Exit(1)
	}

	if err := cfg.EnsureDataDir(); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating data directory: %v\n", err)
		os.Exit(1)
	}

	return cfg
}

// showConfig displays the current configuration
func showConfig(cfg *agentcfg.Config) {
	fmt.Println("GoBot Configuration")
	fmt.Println("===================")
	fmt.Printf("Data Directory: %s\n", cfg.DataDir)
	fmt.Printf("Database: %s\n", cfg.DBPath())
	fmt.Printf("Max Context: %d messages\n", cfg.MaxContext)
	fmt.Printf("Max Iterations: %d\n", cfg.MaxIterations)
	fmt.Println()

	fmt.Println("Providers:")
	for _, p := range cfg.Providers {
		status := "\033[31m✗\033[0m"
		statusInfo := ""
		if p.Type == "cli" && p.Command != "" {
			if ai.CheckCLIAvailable(p.Command) {
				status = "\033[32m✓\033[0m"
				statusInfo = fmt.Sprintf(" (command: %s)", p.Command)
			} else {
				status = "\033[31m✗\033[0m"
				statusInfo = fmt.Sprintf(" (command '%s' not found)", p.Command)
			}
		} else if p.APIKey != "" {
			status = "\033[32m✓\033[0m"
		}
		fmt.Printf("  %s %s (%s)%s\n", status, p.Name, p.Type, statusInfo)
		if p.Model != "" {
			fmt.Printf("      Model: %s\n", p.Model)
		}
	}
	fmt.Println()

	fmt.Println("Policy:")
	fmt.Printf("  Level: %s\n", cfg.Policy.Level)
	fmt.Printf("  Ask Mode: %s\n", cfg.Policy.AskMode)
}

// initConfig initializes a new configuration file
func initConfig() {
	cfg := agentcfg.DefaultConfig()

	configPath := cfg.DataDir + "/config.yaml"
	if _, err := os.Stat(configPath); err == nil {
		fmt.Printf("Config file already exists: %s\n", configPath)
		return
	}

	if err := cfg.Save(); err != nil {
		fmt.Fprintf(os.Stderr, "Error saving config: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Created config file: %s\n", configPath)
	fmt.Println("\nEdit this file to configure providers and settings.")
	fmt.Println("Or set ANTHROPIC_API_KEY environment variable to get started.")
}
