package cli

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/spf13/cobra"
	"gobot/internal/provider"
)

// onboardCmd creates the onboard command for initial setup
func OnboardCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "onboard",
		Short: "Set up GoBot for first use",
		Long: `Interactive setup wizard for GoBot.

This will guide you through:
  1. Creating the ~/.gobot directory
  2. Configuring your AI provider (Anthropic, OpenAI, etc.)
  3. Setting up your first channel (optional)

Examples:
  gobot onboard`,
		Run: func(cmd *cobra.Command, args []string) {
			runOnboard()
		},
	}
}

func runOnboard() {
	reader := bufio.NewReader(os.Stdin)

	fmt.Println()
	fmt.Println("\033[1m🤖 Welcome to GoBot Setup\033[0m")
	fmt.Println("=========================")
	fmt.Println()

	// Step 1: Create config directory
	homeDir, _ := os.UserHomeDir()
	gobotDir := filepath.Join(homeDir, ".gobot")

	if _, err := os.Stat(gobotDir); os.IsNotExist(err) {
		fmt.Printf("Creating config directory: %s\n", gobotDir)
		if err := os.MkdirAll(gobotDir, 0755); err != nil {
			fmt.Printf("\033[31m✗ Failed to create directory: %v\033[0m\n", err)
			os.Exit(1)
		}
		fmt.Println("\033[32m✓ Config directory created\033[0m")
	} else {
		fmt.Println("\033[32m✓ Config directory exists\033[0m")
	}
	fmt.Println()

	// Step 2: Configure AI provider
	fmt.Println("\033[1mStep 1: AI Provider Configuration\033[0m")
	fmt.Println("----------------------------------")
	fmt.Println()
	fmt.Println("Choose your AI provider:")
	fmt.Println("  1. Anthropic (Claude) - Recommended")
	fmt.Println("  2. OpenAI (GPT)")
	fmt.Println("  3. Google (Gemini)")
	fmt.Println("  4. Ollama (Local)")
	fmt.Println("  5. Skip for now")
	fmt.Println()

	fmt.Print("Enter choice [1-5]: ")
	choice, _ := reader.ReadString('\n')
	choice = strings.TrimSpace(choice)

	var providerName, apiKey, model string

	switch choice {
	case "1":
		providerName = "anthropic"
		model = provider.GetDefaultModel("anthropic")
		fmt.Println()
		fmt.Print("Enter your Anthropic API key: ")
		apiKey, _ = reader.ReadString('\n')
		apiKey = strings.TrimSpace(apiKey)
	case "2":
		providerName = "openai"
		model = provider.GetDefaultModel("openai")
		fmt.Println()
		fmt.Print("Enter your OpenAI API key: ")
		apiKey, _ = reader.ReadString('\n')
		apiKey = strings.TrimSpace(apiKey)
	case "3":
		providerName = "google"
		model = provider.GetDefaultModel("google")
		fmt.Println()
		fmt.Print("Enter your Google AI API key: ")
		apiKey, _ = reader.ReadString('\n')
		apiKey = strings.TrimSpace(apiKey)
	case "4":
		providerName = "ollama"
		model = provider.GetDefaultModel("ollama")
		fmt.Println()
		fmt.Printf("Enter Ollama model name [%s]: ", model)
		input, _ := reader.ReadString('\n')
		input = strings.TrimSpace(input)
		if input != "" {
			model = input
		}
	case "5":
		fmt.Println("\033[33m⚠ Skipping provider setup. Set ANTHROPIC_API_KEY environment variable later.\033[0m")
	default:
		fmt.Println("\033[33m⚠ Invalid choice. Skipping provider setup.\033[0m")
	}

	// Write config file
	configPath := filepath.Join(gobotDir, "config.yaml")
	if providerName != "" {
		config := generateConfig(providerName, apiKey, model)
		if err := os.WriteFile(configPath, []byte(config), 0600); err != nil {
			fmt.Printf("\033[31m✗ Failed to write config: %v\033[0m\n", err)
		} else {
			fmt.Printf("\033[32m✓ Config saved to %s\033[0m\n", configPath)
		}
	}
	fmt.Println()

	// Step 3: Optional channel setup
	fmt.Println("\033[1mStep 2: Channel Setup (Optional)\033[0m")
	fmt.Println("--------------------------------")
	fmt.Println()
	fmt.Println("Would you like to set up a messaging channel?")
	fmt.Println("  1. Telegram")
	fmt.Println("  2. Discord")
	fmt.Println("  3. Slack")
	fmt.Println("  4. Skip for now")
	fmt.Println()

	fmt.Print("Enter choice [1-4]: ")
	channelChoice, _ := reader.ReadString('\n')
	channelChoice = strings.TrimSpace(channelChoice)

	switch channelChoice {
	case "1":
		setupTelegram(reader, gobotDir)
	case "2":
		setupDiscord(reader, gobotDir)
	case "3":
		setupSlack(reader, gobotDir)
	case "4":
		fmt.Println("Skipping channel setup.")
	default:
		fmt.Println("Skipping channel setup.")
	}

	fmt.Println()
	fmt.Println("\033[1m🎉 Setup Complete!\033[0m")
	fmt.Println("==================")
	fmt.Println()
	fmt.Println("Next steps:")
	fmt.Println("  1. Start the gateway:  gobot gateway")
	fmt.Println("  2. Start chatting:     gobot chat --interactive")
	fmt.Println("  3. Check health:       gobot doctor")
	fmt.Println()
	fmt.Println("For more information: https://github.com/yourusername/gobot")
}

func generateConfig(provider, apiKey, model string) string {
	if provider == "ollama" {
		return fmt.Sprintf(`# GoBot Configuration
providers:
  - name: %s
    type: api
    model: %s
    base_url: http://localhost:11434

max_context: 100
max_iterations: 50

policy:
  level: normal
  ask_mode: auto
`, provider, model)
	}

	keyLine := ""
	if apiKey != "" {
		keyLine = fmt.Sprintf("    api_key: %s", apiKey)
	} else {
		envVar := strings.ToUpper(provider) + "_API_KEY"
		keyLine = fmt.Sprintf("    api_key: ${%s}", envVar)
	}

	return fmt.Sprintf(`# GoBot Configuration
providers:
  - name: %s
    type: api
%s
    model: %s

max_context: 100
max_iterations: 50

policy:
  level: normal
  ask_mode: auto
`, provider, keyLine, model)
}

func setupTelegram(reader *bufio.Reader, gobotDir string) {
	fmt.Println()
	fmt.Println("Telegram Setup")
	fmt.Println("--------------")
	fmt.Println("1. Open Telegram and message @BotFather")
	fmt.Println("2. Send /newbot and follow the prompts")
	fmt.Println("3. Copy the bot token")
	fmt.Println()

	fmt.Print("Enter your Telegram bot token: ")
	token, _ := reader.ReadString('\n')
	token = strings.TrimSpace(token)

	if token == "" {
		fmt.Println("\033[33m⚠ No token provided. Skipping Telegram setup.\033[0m")
		return
	}

	channelConfig := fmt.Sprintf(`telegram:
  bot_token: %s
  allowed_users: []  # Add Telegram user IDs to restrict access
`, token)

	channelsPath := filepath.Join(gobotDir, "channels.yaml")
	if err := appendToFile(channelsPath, channelConfig); err != nil {
		fmt.Printf("\033[31m✗ Failed to save channel config: %v\033[0m\n", err)
	} else {
		fmt.Printf("\033[32m✓ Telegram configured in %s\033[0m\n", channelsPath)
	}
}

func setupDiscord(reader *bufio.Reader, gobotDir string) {
	fmt.Println()
	fmt.Println("Discord Setup")
	fmt.Println("-------------")
	fmt.Println("1. Go to https://discord.com/developers/applications")
	fmt.Println("2. Create a new application")
	fmt.Println("3. Go to Bot section and create a bot")
	fmt.Println("4. Copy the bot token")
	fmt.Println()

	fmt.Print("Enter your Discord bot token: ")
	token, _ := reader.ReadString('\n')
	token = strings.TrimSpace(token)

	if token == "" {
		fmt.Println("\033[33m⚠ No token provided. Skipping Discord setup.\033[0m")
		return
	}

	channelConfig := fmt.Sprintf(`discord:
  bot_token: %s
  allowed_guilds: []  # Add Discord guild IDs to restrict access
`, token)

	channelsPath := filepath.Join(gobotDir, "channels.yaml")
	if err := appendToFile(channelsPath, channelConfig); err != nil {
		fmt.Printf("\033[31m✗ Failed to save channel config: %v\033[0m\n", err)
	} else {
		fmt.Printf("\033[32m✓ Discord configured in %s\033[0m\n", channelsPath)
	}
}

func setupSlack(reader *bufio.Reader, gobotDir string) {
	fmt.Println()
	fmt.Println("Slack Setup")
	fmt.Println("-----------")
	fmt.Println("1. Go to https://api.slack.com/apps")
	fmt.Println("2. Create a new app")
	fmt.Println("3. Add Bot Token Scopes: chat:write, app_mentions:read")
	fmt.Println("4. Install to workspace and copy the Bot Token")
	fmt.Println()

	fmt.Print("Enter your Slack bot token (xoxb-...): ")
	token, _ := reader.ReadString('\n')
	token = strings.TrimSpace(token)

	if token == "" {
		fmt.Println("\033[33m⚠ No token provided. Skipping Slack setup.\033[0m")
		return
	}

	fmt.Print("Enter your Slack app token (xapp-...): ")
	appToken, _ := reader.ReadString('\n')
	appToken = strings.TrimSpace(appToken)

	channelConfig := fmt.Sprintf(`slack:
  bot_token: %s
  app_token: %s
  allowed_channels: []  # Add Slack channel IDs to restrict access
`, token, appToken)

	channelsPath := filepath.Join(gobotDir, "channels.yaml")
	if err := appendToFile(channelsPath, channelConfig); err != nil {
		fmt.Printf("\033[31m✗ Failed to save channel config: %v\033[0m\n", err)
	} else {
		fmt.Printf("\033[32m✓ Slack configured in %s\033[0m\n", channelsPath)
	}
}

func appendToFile(path, content string) error {
	f, err := os.OpenFile(path, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	defer f.Close()

	_, err = f.WriteString(content + "\n")
	return err
}
