package events

import "fmt"

const (
	TopicCDPBroadcast = "cdp.broadcast"
)

func CDPClientTopic(clientID string) string {
	return fmt.Sprintf("cdp.client.%s", clientID)
}
