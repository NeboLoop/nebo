package cli

import (
	"github.com/spf13/cobra"

	"gobot/internal/config"
)

// Shared CLI flags (used across multiple command files)
var (
	cfgFile     string
	sessionKey  string
	providerArg string
	verbose     bool
)

// ServerConfig holds the loaded server configuration (set by main)
var ServerConfig *config.Config

// SetupRootCmd configures the root command with all subcommands and flags
func SetupRootCmd(c *config.Config) *cobra.Command {
	ServerConfig = c

	rootCmd := &cobra.Command{
		Use:   "gobot",
		Short: "GoBot - AI Assistant",
		Long: `GoBot is an AI assistant with tool use capabilities for software development and automation.

Just type 'gobot' to start both the server and agent together.`,
		Run: func(cmd *cobra.Command, args []string) {
			RunAll()
		},
	}

	// Global flags
	rootCmd.PersistentFlags().StringVar(&cfgFile, "config", "", "config file (default: ~/.gobot/config.yaml)")
	rootCmd.PersistentFlags().StringVarP(&sessionKey, "session", "s", "default", "session key for conversation history")
	rootCmd.PersistentFlags().StringVarP(&providerArg, "provider", "p", "", "provider to use (default: first available)")
	rootCmd.PersistentFlags().BoolVarP(&verbose, "verbose", "v", false, "verbose output")

	// Add commands
	rootCmd.AddCommand(ServeCmd())
	rootCmd.AddCommand(AgentCmd())
	rootCmd.AddCommand(ChatCmd())
	rootCmd.AddCommand(ConfigCmd())
	rootCmd.AddCommand(SessionCmd())
	rootCmd.AddCommand(SkillsCmd())
	rootCmd.AddCommand(PluginsCmd())
	rootCmd.AddCommand(MessageCmd())
	rootCmd.AddCommand(DoctorCmd())
	rootCmd.AddCommand(OnboardCmd())

	return rootCmd
}
