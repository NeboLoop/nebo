package cli

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"strings"

	"github.com/spf13/cobra"
)

// messageCmd creates the message command
func MessageCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "message",
		Short: "Send messages to channels",
		Long:  `Send messages to various channels (Telegram, Discord, Slack, etc.)`,
	}

	cmd.AddCommand(messageSendCmd())
	cmd.AddCommand(messageListChannelsCmd())

	return cmd
}

func messageSendCmd() *cobra.Command {
	var to string
	var channel string

	cmd := &cobra.Command{
		Use:   "send [message]",
		Short: "Send a message to a channel",
		Long: `Send a message to a specific channel or user.

Examples:
  gobot message send "Hello world" --to telegram:123456789
  gobot message send "Hello" --channel slack:general
  gobot message send "Hi there" --to discord:user123`,
		Args: cobra.MinimumNArgs(1),
		Run: func(cmd *cobra.Command, args []string) {
			message := strings.Join(args, " ")
			runMessageSend(message, to, channel)
		},
	}

	cmd.Flags().StringVar(&to, "to", "", "Target (format: channel_type:id, e.g., telegram:123456789)")
	cmd.Flags().StringVar(&channel, "channel", "", "Channel (format: channel_type:channel_id)")

	return cmd
}

func messageListChannelsCmd() *cobra.Command {
	return &cobra.Command{
		Use:   "channels",
		Short: "List available channels",
		Long:  `List all connected messaging channels.`,
		Run: func(cmd *cobra.Command, args []string) {
			runListChannels()
		},
	}
}

func runMessageSend(message, to, channel string) {
	if to == "" && channel == "" {
		fmt.Println("Error: Must specify --to or --channel")
		os.Exit(1)
	}

	target := to
	if target == "" {
		target = channel
	}

	// Parse target format: type:id
	parts := strings.SplitN(target, ":", 2)
	if len(parts) != 2 {
		fmt.Println("Error: Target must be in format 'type:id' (e.g., telegram:123456789)")
		os.Exit(1)
	}

	channelType := parts[0]
	channelID := parts[1]

	// Get gateway URL from config or use default
	gatewayURL := os.Getenv("GOBOT_GATEWAY_URL")
	if gatewayURL == "" {
		gatewayURL = "http://localhost:27895"
	}

	payload := map[string]interface{}{
		"channel_type": channelType,
		"channel_id":   channelID,
		"message":      message,
	}

	jsonData, err := json.Marshal(payload)
	if err != nil {
		fmt.Printf("Error encoding message: %v\n", err)
		os.Exit(1)
	}

	resp, err := http.Post(gatewayURL+"/api/v1/messages/send", "application/json", bytes.NewBuffer(jsonData))
	if err != nil {
		fmt.Printf("Error sending message: %v\n", err)
		fmt.Println("Make sure the Gateway is running (gobot gateway)")
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		fmt.Printf("Error: %s\n", string(body))
		os.Exit(1)
	}

	fmt.Printf("\033[32m✓ Message sent to %s:%s\033[0m\n", channelType, channelID)
}

func runListChannels() {
	gatewayURL := os.Getenv("GOBOT_GATEWAY_URL")
	if gatewayURL == "" {
		gatewayURL = "http://localhost:27895"
	}

	resp, err := http.Get(gatewayURL + "/api/v1/channels")
	if err != nil {
		fmt.Printf("Error fetching channels: %v\n", err)
		fmt.Println("Make sure the Gateway is running (gobot gateway)")
		os.Exit(1)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		fmt.Printf("Error: %s\n", string(body))
		os.Exit(1)
	}

	var result struct {
		Channels []struct {
			Type      string `json:"type"`
			ID        string `json:"id"`
			Name      string `json:"name"`
			Connected bool   `json:"connected"`
		} `json:"channels"`
	}

	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		fmt.Printf("Error parsing response: %v\n", err)
		os.Exit(1)
	}

	if len(result.Channels) == 0 {
		fmt.Println("No channels connected.")
		fmt.Println("\nRun 'gobot onboard' to set up your first channel.")
		return
	}

	fmt.Println("Connected Channels:")
	fmt.Println("-------------------")
	for _, ch := range result.Channels {
		status := "\033[32m●\033[0m"
		if !ch.Connected {
			status = "\033[31m○\033[0m"
		}
		fmt.Printf("%s %s:%s (%s)\n", status, ch.Type, ch.ID, ch.Name)
	}
}
