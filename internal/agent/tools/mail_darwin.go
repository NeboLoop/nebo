//go:build darwin && !ios

package tools

import (
	"context"
	"encoding/json"
	"fmt"
)

// MailTool provides macOS Mail.app integration via AppleScript.
type MailTool struct{}

func NewMailTool() *MailTool { return &MailTool{} }

func (t *MailTool) Name() string { return "mail" }

func (t *MailTool) Description() string {
	return "Read and send emails using Mail.app - check unread, search, and compose messages."
}

func (t *MailTool) Schema() json.RawMessage {
	return json.RawMessage(`{
		"type": "object",
		"properties": {
			"action": {
				"type": "string",
				"enum": ["read", "send", "unread", "search", "accounts"],
				"description": "Action: read (recent), send (compose), unread (count), search, accounts (list)"
			},
			"to": {"type": "array", "items": {"type": "string"}, "description": "Recipients (for send)"},
			"cc": {"type": "array", "items": {"type": "string"}, "description": "CC recipients (for send)"},
			"subject": {"type": "string", "description": "Email subject (for send)"},
			"body": {"type": "string", "description": "Email body (for send)"},
			"mailbox": {"type": "string", "description": "Mailbox name (INBOX, Sent, etc.)"},
			"query": {"type": "string", "description": "Search query"},
			"count": {"type": "integer", "description": "Number of emails to fetch (default: 10)"}
		},
		"required": ["action"]
	}`)
}

func (t *MailTool) RequiresApproval() bool { return true }

type mailInput struct {
	Action  string   `json:"action"`
	To      []string `json:"to"`
	Cc      []string `json:"cc"`
	Subject string   `json:"subject"`
	Body    string   `json:"body"`
	Mailbox string   `json:"mailbox"`
	Query   string   `json:"query"`
	Count   int      `json:"count"`
}

func (t *MailTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	var p mailInput
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	switch p.Action {
	case "accounts":
		return t.listAccounts()
	case "unread":
		return t.getUnreadCount()
	case "read":
		count := p.Count
		if count <= 0 {
			count = 10
		}
		return t.readRecentEmails(count, p.Mailbox)
	case "send":
		return t.sendEmail(p)
	case "search":
		return t.searchEmails(p.Query, p.Count)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *MailTool) listAccounts() (*ToolResult, error) {
	script := `tell application "Mail"
		set accountList to {}
		repeat with acct in accounts
			set end of accountList to name of acct & " (" & (count of mailboxes of acct) & " mailboxes)"
		end repeat
		return accountList
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Email accounts:\n%s", out)}, nil
}

func (t *MailTool) getUnreadCount() (*ToolResult, error) {
	script := `tell application "Mail"
		set unreadList to {}
		repeat with acct in accounts
			set inboxMailbox to mailbox "INBOX" of acct
			set unreadCount to unread count of inboxMailbox
			if unreadCount > 0 then
				set end of unreadList to name of acct & ": " & unreadCount & " unread"
			end if
		end repeat
		if length of unreadList = 0 then return "No unread emails"
		return unreadList
	end tell`
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *MailTool) readRecentEmails(count int, mailbox string) (*ToolResult, error) {
	if mailbox == "" {
		mailbox = "INBOX"
	}
	script := fmt.Sprintf(`tell application "Mail"
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
	end tell`, mailbox, count)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Recent emails in %s:\n%s", mailbox, out)}, nil
}

func (t *MailTool) sendEmail(p mailInput) (*ToolResult, error) {
	if len(p.To) == 0 {
		return &ToolResult{Content: "At least one recipient is required", IsError: true}, nil
	}
	if p.Subject == "" {
		return &ToolResult{Content: "Subject is required", IsError: true}, nil
	}
	if p.Body == "" {
		return &ToolResult{Content: "Body is required", IsError: true}, nil
	}

	var toRecipients, ccRecipients string
	for _, addr := range p.To {
		toRecipients += fmt.Sprintf(`make new to recipient with properties {address:"%s"}
				`, escapeAS(addr))
	}
	for _, addr := range p.Cc {
		ccRecipients += fmt.Sprintf(`make new cc recipient with properties {address:"%s"}
				`, escapeAS(addr))
	}

	script := fmt.Sprintf(`tell application "Mail"
		set newMessage to make new outgoing message with properties {subject:"%s", content:"%s", visible:true}
		tell newMessage
			%s
			%s
		end tell
		send newMessage
	end tell
	return "Email sent successfully"`, escapeAS(p.Subject), escapeAS(p.Body), toRecipients, ccRecipients)

	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	return &ToolResult{Content: out}, nil
}

func (t *MailTool) searchEmails(query string, count int) (*ToolResult, error) {
	if query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}
	if count <= 0 {
		count = 10
	}
	script := fmt.Sprintf(`tell application "Mail"
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
	end tell`, escapeAS(query), escapeAS(query), count)
	out, err := execAppleScript(script)
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed: %v", err), IsError: true}, nil
	}
	if out == "" || out == "{}" {
		return &ToolResult{Content: fmt.Sprintf("No emails found matching '%s'", query)}, nil
	}
	return &ToolResult{Content: fmt.Sprintf("Search results for '%s':\n%s", query, out)}, nil
}

