package cli

import (
	"github.com/spf13/cobra"

	"github.com/nebolabs/nebo/internal/config"
)

// Shared CLI flags (used across multiple command files)
var (
	cfgFile        string
	sessionKey     string
	providerArg    string
	verbose        bool
	headless       bool
	dangerouslyAll = true // Default to dangerous mode for now (bypass approvals)
)

// ServerConfig holds the loaded server configuration (set by main)
var ServerConfig *config.Config

// SetupRootCmd configures the root command with all subcommands and flags
func SetupRootCmd(c *config.Config) *cobra.Command {
	ServerConfig = c

	rootCmd := &cobra.Command{
		Use:   "nebo",
		Short: "Nebo - AI Assistant",
		Long: `Nebo is an AI assistant with tool use capabilities for software development and automation.

Just type 'nebo' to start both the server and agent together.
Use --headless to run without a native window (browser-only mode).`,
		Run: func(cmd *cobra.Command, args []string) {
			if headless {
				RunAll()
			} else {
				RunDesktop()
			}
		},
	}

	// Global flags
	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", "config file (default: platform data directory)")
	rootCmd.PersistentFlags().StringVarP(&sessionKey, "session", "s", "default", "session key for conversation history")
	rootCmd.PersistentFlags().StringVarP(&providerArg, "provider", "p", "", "provider to use (default: first available)")
	rootCmd.PersistentFlags().BoolVarP(&verbose, "verbose", "v", false, "verbose output")

	// Root-only flags
	rootCmd.Flags().BoolVar(&headless, "headless", false, "run without native window (HTTP server + agent only)")
	rootCmd.Flags().BoolVar(&dangerouslyAll, "dangerously", false, "100% autonomous mode - bypass ALL tool approval prompts")

	// Add commands
	rootCmd.AddCommand(ServeCmd())
	rootCmd.AddCommand(AgentCmd())
	rootCmd.AddCommand(ChatCmd())
	rootCmd.AddCommand(ConfigCmd())
	rootCmd.AddCommand(SessionCmd())
	rootCmd.AddCommand(SkillsCmd())
	rootCmd.AddCommand(PluginsCmd())
	rootCmd.AddCommand(CapabilitiesCmd())
	rootCmd.AddCommand(MessageCmd())
	rootCmd.AddCommand(DoctorCmd())
	rootCmd.AddCommand(OnboardCmd())

	return rootCmd
}
