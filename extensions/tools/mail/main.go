// Mail Plugin - macOS Mail.app integration via AppleScript
// Build: go build -o ~/.gobot/plugins/tools/mail
package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net/rpc"
	"os/exec"
	"strings"

	"github.com/hashicorp/go-plugin"
)

var Handshake = plugin.HandshakeConfig{
	ProtocolVersion:  1,
	MagicCookieKey:   "GOBOT_PLUGIN",
	MagicCookieValue: "gobot-plugin-v1",
}

type MailTool struct{}

type mailInput struct {
	Action   string   `json:"action"`    // read, send, unread, search, accounts
	To       []string `json:"to"`        // Recipients
	Cc       []string `json:"cc"`        // CC recipients
	Subject  string   `json:"subject"`   // Email subject
	Body     string   `json:"body"`      // Email body
	Account  string   `json:"account"`   // Account name
	Mailbox  string   `json:"mailbox"`   // Mailbox name (INBOX, Sent, etc.)
	Query    string   `json:"query"`     // Search query
	Count    int      `json:"count"`     // Number of emails to fetch
}

type ToolResult struct {
	Content string `json:"content"`
	IsError bool   `json:"is_error"`
}

func (t *MailTool) Name() string {
	return "mail"
}

func (t *MailTool) Description() string {
	return "Read and send emails using macOS Mail.app. Check unread messages, search emails, and compose new messages."
}

func (t *MailTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["read", "send", "unread", "search", "accounts"],
				"description": "Action: read (recent emails), send (compose), unread (unread count), search, accounts (list)"
			},
			"to": {
				"type": "array",
				"items": {"type": "string"},
				"description": "Recipient email addresses (for send)"
			},
			"cc": {
				"type": "array",
				"items": {"type": "string"},
				"description": "CC recipients (for send)"
			},
			"subject": {
				"type": "string",
				"description": "Email subject (for send)"
			},
			"body": {
				"type": "string",
				"description": "Email body content (for send)"
			},
			"account": {
				"type": "string",
				"description": "Email account name"
			},
			"mailbox": {
				"type": "string",
				"description": "Mailbox name (INBOX, Sent, Drafts, etc.)"
			},
			"query": {
				"type": "string",
				"description": "Search query (for search action)"
			},
			"count": {
				"type": "integer",
				"description": "Number of emails to fetch (default: 10)"
			}
		},
		"required": ["action"]
	}`)
}

func (t *MailTool) RequiresApproval() bool {
	return true // Sending email requires approval
}

func (t *MailTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var params mailInput
	if err := json.Unmarshal(input, &params); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch params.Action {
	case "accounts":
		return t.listAccounts()
	case "unread":
		return t.getUnreadCount()
	case "read":
		count := params.Count
		if count <= 0 {
			count = 10
		}
		return t.readRecentEmails(count, params.Mailbox)
	case "send":
		return t.sendEmail(params)
	case "search":
		return t.searchEmails(params.Query, params.Count)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", params.Action), IsError: true}, nil
	}
}

func (t *MailTool) listAccounts() (*ToolResult, error) {
	script := `
		tell application "Mail"
			set accountList to {}
			repeat with acct in accounts
				set end of accountList to name of acct & " (" & (count of mailboxes of acct) & " mailboxes)"
			end repeat
			return accountList
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list accounts: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Email accounts:\n%s", output), IsError: false}, nil
}

func (t *MailTool) getUnreadCount() (*ToolResult, error) {
	script := `
		tell application "Mail"
			set unreadList to {}
			repeat with acct in accounts
				set inboxMailbox to mailbox "INBOX" of acct
				set unreadCount to unread count of inboxMailbox
				if unreadCount > 0 then
					set end of unreadList to name of acct & ": " & unreadCount & " unread"
				end if
			end repeat
			if length of unreadList = 0 then
				return "No unread emails"
			end if
			return unreadList
		end tell
	`
	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get unread count: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MailTool) readRecentEmails(count int, mailbox string) (*ToolResult, error) {
	if mailbox == "" {
		mailbox = "INBOX"
	}

	script := fmt.Sprintf(`
		tell application "Mail"
			set emailList to {}
			repeat with acct in accounts
				try
					set theMailbox to mailbox "%s" of acct
					set theMessages to messages 1 thru %d of theMailbox
					repeat with msg in theMessages
						set msgInfo to "From: " & (sender of msg) & return & "Subject: " & (subject of msg) & return & "Date: " & (date received of msg as string) & return & "---"
						set end of emailList to msgInfo
					end repeat
				end try
			end repeat
			return emailList
		end tell
	`, mailbox, count)

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to read emails: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Recent emails in %s:\n%s", mailbox, output), IsError: false}, nil
}

func (t *MailTool) sendEmail(params mailInput) (*ToolResult, error) {
	if len(params.To) == 0 {
		return &ToolResult{Content: "At least one recipient is required", IsError: true}, nil
	}
	if params.Subject == "" {
		return &ToolResult{Content: "Subject is required", IsError: true}, nil
	}
	if params.Body == "" {
		return &ToolResult{Content: "Body is required", IsError: true}, nil
	}

	// Build recipient list
	toRecipients := ""
	for _, addr := range params.To {
		toRecipients += fmt.Sprintf(`make new to recipient with properties {address:"%s"}
				`, escapeAppleScript(addr))
	}

	ccRecipients := ""
	for _, addr := range params.Cc {
		ccRecipients += fmt.Sprintf(`make new cc recipient with properties {address:"%s"}
				`, escapeAppleScript(addr))
	}

	script := fmt.Sprintf(`
		tell application "Mail"
			set newMessage to make new outgoing message with properties {subject:"%s", content:"%s", visible:true}
			tell newMessage
				%s
				%s
			end tell
			send newMessage
		end tell
		return "Email sent successfully"
	`, escapeAppleScript(params.Subject), escapeAppleScript(params.Body), toRecipients, ccRecipients)

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to send email: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: output, IsError: false}, nil
}

func (t *MailTool) searchEmails(query string, count int) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}
	if count <= 0 {
		count = 10
	}

	script := fmt.Sprintf(`
		tell application "Mail"
			set searchResults to {}
			repeat with acct in accounts
				try
					set theMailbox to mailbox "INBOX" of acct
					set foundMessages to (messages of theMailbox whose subject contains "%s" or sender contains "%s")
					set msgCount to 0
					repeat with msg in foundMessages
						if msgCount < %d then
							set msgInfo to "From: " & (sender of msg) & return & "Subject: " & (subject of msg) & return & "---"
							set end of searchResults to msgInfo
							set msgCount to msgCount + 1
						end if
					end repeat
				end try
			end repeat
			return searchResults
		end tell
	`, escapeAppleScript(query), escapeAppleScript(query), count)

	output, err := runAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v", err), IsError: true}, nil
	}
	if output == "" || output == "{}" {
		return &ToolResult{Content: fmt.Sprintf("No emails found matching '%s'", query), IsError: false}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Search results for '%s':\n%s", query, output), IsError: false}, nil
}

func escapeAppleScript(s string) string {
	s = strings.ReplaceAll(s, "\\", "\\\\")
	s = strings.ReplaceAll(s, "\"", "\\\"")
	return s
}

func runAppleScript(script string) (string, error) {
	cmd := exec.Command("osascript", "-e", script)
	output, err := cmd.CombinedOutput()
	return strings.TrimSpace(string(output)), err
}

// RPC wrapper
type MailToolRPC struct {
	tool *MailTool
}

func (t *MailToolRPC) Name(args interface{}, reply *string) error {
	*reply = t.tool.Name()
	return nil
}

func (t *MailToolRPC) Description(args interface{}, reply *string) error {
	*reply = t.tool.Description()
	return nil
}

func (t *MailToolRPC) Schema(args interface{}, reply *json.RawMessage) error {
	*reply = t.tool.Schema()
	return nil
}

func (t *MailToolRPC) RequiresApproval(args interface{}, reply *bool) error {
	*reply = t.tool.RequiresApproval()
	return nil
}

type ExecuteArgs struct {
	Input json.RawMessage
}

func (t *MailToolRPC) Execute(args *ExecuteArgs, reply *ToolResult) error {
	result, err := t.tool.Execute(context.Background(), args.Input)
	if err != nil {
		return err
	}
	*reply = *result
	return nil
}

type MailPlugin struct {
	tool *MailTool
}

func (p *MailPlugin) Server(*plugin.MuxBroker) (interface{}, error) {
	return &MailToolRPC{tool: p.tool}, nil
}

func (p *MailPlugin) Client(b *plugin.MuxBroker, c *rpc.Client) (interface{}, error) {
	return nil, nil
}

func main() {
	plugin.Serve(&plugin.ServeConfig{
		HandshakeConfig: Handshake,
		Plugins: map[string]plugin.Plugin{
			"tool": &MailPlugin{tool: &MailTool{}},
		},
	})
}
