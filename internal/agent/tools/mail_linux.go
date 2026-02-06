//go:build linux

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
)

// MailTool provides Linux email integration via mutt/neomutt, s-nail, or notmuch.
type MailTool struct {
	sendBackend string // "mutt", "neomutt", "s-nail", "sendmail", or ""
	readBackend string // "notmuch", "mutt", or ""
}

func NewMailTool() *MailTool {
	t := &MailTool{}
	t.sendBackend, t.readBackend = t.detectBackends()
	return t
}

func (t *MailTool) detectBackends() (send, read string) {
	// Detect send backend
	if _, err := exec.LookPath("neomutt"); err == nil {
		send = "neomutt"
	} else if _, err := exec.LookPath("mutt"); err == nil {
		send = "mutt"
	} else if _, err := exec.LookPath("s-nail"); err == nil {
		send = "s-nail"
	} else if _, err := exec.LookPath("mail"); err == nil {
		send = "mail"
	} else if _, err := exec.LookPath("sendmail"); err == nil {
		send = "sendmail"
	}

	// Detect read backend
	if _, err := exec.LookPath("notmuch"); err == nil {
		// Check if notmuch is configured
		notmuchDir := filepath.Join(os.Getenv("HOME"), ".notmuch-config")
		if _, err := os.Stat(notmuchDir); err == nil {
			read = "notmuch"
		}
	}
	if read == "" && (send == "mutt" || send == "neomutt") {
		read = send
	}

	return send, read
}

func (t *MailTool) Name() string { return "mail" }

func (t *MailTool) Description() string {
	if t.sendBackend == "" && t.readBackend == "" {
		return "Read and send emails - requires mutt/neomutt, s-nail, or notmuch to be installed."
	}

	var parts []string
	if t.readBackend != "" {
		parts = append(parts, fmt.Sprintf("read via %s", t.readBackend))
	}
	if t.sendBackend != "" {
		parts = append(parts, fmt.Sprintf("send via %s", t.sendBackend))
	}

	return fmt.Sprintf("Read and send emails (%s) - check unread, search, and compose messages.", strings.Join(parts, ", "))
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

type mailInputLinux struct {
	Action  string   `json:"action"`
	To      []string `json:"to"`
	CC      []string `json:"cc"`
	Subject string   `json:"subject"`
	Body    string   `json:"body"`
	Mailbox string   `json:"mailbox"`
	Query   string   `json:"query"`
	Count   int      `json:"count"`
}

func (t *MailTool) Execute(ctx context.Context, input json.RawMessage) (*ToolResult, error) {
	if t.sendBackend == "" && t.readBackend == "" {
		return &ToolResult{
			Content: "No email backend available. Please install one of:\n" +
				"  - neomutt: sudo apt install neomutt (recommended)\n" +
				"  - mutt: sudo apt install mutt\n" +
				"  - s-nail: sudo apt install s-nail\n" +
				"  - notmuch: sudo apt install notmuch (for advanced searching)\n\n" +
				"After installation, configure your email account in ~/.muttrc or ~/.mailrc",
			IsError: true,
		}, nil
	}

	var p mailInputLinux
	if err := json.Unmarshal(input, &p); err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to parse input: %v", err), IsError: true}, nil
	}

	if p.Count <= 0 {
		p.Count = 10
	}

	switch p.Action {
	case "send":
		return t.sendEmail(ctx, p)
	case "read":
		return t.readEmails(ctx, p)
	case "unread":
		return t.getUnreadCount(ctx, p)
	case "search":
		return t.searchEmails(ctx, p)
	case "accounts":
		return t.listAccounts(ctx)
	default:
		return &ToolResult{Content: fmt.Sprintf("Unknown action: %s", p.Action), IsError: true}, nil
	}
}

func (t *MailTool) sendEmail(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	if t.sendBackend == "" {
		return &ToolResult{Content: "No email sending backend available", IsError: true}, nil
	}

	if len(p.To) == 0 {
		return &ToolResult{Content: "At least one recipient is required", IsError: true}, nil
	}
	if p.Subject == "" {
		return &ToolResult{Content: "Subject is required", IsError: true}, nil
	}

	switch t.sendBackend {
	case "neomutt", "mutt":
		return t.sendViaMutt(ctx, p)
	case "s-nail", "mail":
		return t.sendViaSNail(ctx, p)
	case "sendmail":
		return t.sendViaSendmail(ctx, p)
	default:
		return &ToolResult{Content: "Unknown send backend", IsError: true}, nil
	}
}

func (t *MailTool) sendViaMutt(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	args := []string{"-s", p.Subject}

	// Add CC recipients
	for _, cc := range p.CC {
		args = append(args, "-c", cc)
	}

	// Add recipients
	args = append(args, p.To...)

	cmd := exec.CommandContext(ctx, t.sendBackend, args...)
	cmd.Stdin = strings.NewReader(p.Body)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to send email: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Email sent to %s", strings.Join(p.To, ", "))}, nil
}

func (t *MailTool) sendViaSNail(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	args := []string{"-s", p.Subject}

	// Add CC recipients
	for _, cc := range p.CC {
		args = append(args, "-c", cc)
	}

	// Add recipients
	args = append(args, p.To...)

	cmd := exec.CommandContext(ctx, t.sendBackend, args...)
	cmd.Stdin = strings.NewReader(p.Body)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to send email: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Email sent to %s", strings.Join(p.To, ", "))}, nil
}

func (t *MailTool) sendViaSendmail(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	// Build raw email
	var email strings.Builder
	email.WriteString(fmt.Sprintf("To: %s\n", strings.Join(p.To, ", ")))
	if len(p.CC) > 0 {
		email.WriteString(fmt.Sprintf("Cc: %s\n", strings.Join(p.CC, ", ")))
	}
	email.WriteString(fmt.Sprintf("Subject: %s\n", p.Subject))
	email.WriteString("Content-Type: text/plain; charset=UTF-8\n")
	email.WriteString("\n")
	email.WriteString(p.Body)

	args := []string{"-t"}
	cmd := exec.CommandContext(ctx, "sendmail", args...)
	cmd.Stdin = strings.NewReader(email.String())
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to send email: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Email sent to %s", strings.Join(p.To, ", "))}, nil
}

func (t *MailTool) readEmails(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	if t.readBackend == "" {
		return &ToolResult{Content: "No email reading backend available. Install notmuch or configure mutt."}, nil
	}

	switch t.readBackend {
	case "notmuch":
		return t.readViaNotmuch(ctx, p)
	case "mutt", "neomutt":
		return t.readViaMutt(ctx, p)
	default:
		return &ToolResult{Content: "No email reading backend available"}, nil
	}
}

func (t *MailTool) readViaNotmuch(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	query := "date:1month..today"
	if p.Mailbox != "" {
		query = fmt.Sprintf("folder:%s and %s", p.Mailbox, query)
	}

	args := []string{"search", "--format=text", "--output=summary", "--limit=" + strconv.Itoa(p.Count), query}
	cmd := exec.CommandContext(ctx, "notmuch", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to read emails: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No recent emails found"}, nil
	}

	// Format the output
	lines := strings.Split(output, "\n")
	var results []string
	for _, line := range lines {
		if line != "" {
			results = append(results, line)
		}
	}

	return &ToolResult{Content: fmt.Sprintf("Recent emails (%d):\n%s", len(results), strings.Join(results, "\n"))}, nil
}

func (t *MailTool) readViaMutt(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	// Use mutt to list recent emails
	// This requires proper mutt configuration
	mailbox := p.Mailbox
	if mailbox == "" {
		mailbox = "INBOX"
	}

	// Use mutt in batch mode to list messages
	script := fmt.Sprintf(`echo "quit" | %s -f "%s" -e "set pager_index_lines=0" -e "push '<limit>~d<1m<enter><pipe-message>head -20<enter>q'" 2>/dev/null | head -%d`, t.readBackend, mailbox, p.Count)

	cmd := exec.CommandContext(ctx, "bash", "-c", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		// Mutt batch mode often fails, provide alternative
		return &ToolResult{
			Content: "Could not read emails programmatically.\n\n" +
				"For better email reading support, install notmuch:\n" +
				"  sudo apt install notmuch\n" +
				"  notmuch setup\n" +
				"  notmuch new",
		}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: "No recent emails found"}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Recent emails:\n%s", output)}, nil
}

func (t *MailTool) getUnreadCount(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	if t.readBackend == "" {
		return &ToolResult{Content: "No email reading backend available"}, nil
	}

	switch t.readBackend {
	case "notmuch":
		return t.unreadViaNotmuch(ctx, p)
	default:
		return &ToolResult{Content: "Unread count requires notmuch. Install with: sudo apt install notmuch"}, nil
	}
}

func (t *MailTool) unreadViaNotmuch(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	query := "tag:unread"
	if p.Mailbox != "" {
		query = fmt.Sprintf("folder:%s and %s", p.Mailbox, query)
	}

	args := []string{"count", query}
	cmd := exec.CommandContext(ctx, "notmuch", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get unread count: %v", err), IsError: true}, nil
	}

	count := strings.TrimSpace(string(out))
	return &ToolResult{Content: fmt.Sprintf("Unread emails: %s", count)}, nil
}

func (t *MailTool) searchEmails(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	if p.Query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	if t.readBackend == "" {
		return &ToolResult{Content: "Email search requires notmuch. Install with: sudo apt install notmuch"}, nil
	}

	switch t.readBackend {
	case "notmuch":
		return t.searchViaNotmuch(ctx, p)
	default:
		return &ToolResult{Content: "Email search requires notmuch"}, nil
	}
}

func (t *MailTool) searchViaNotmuch(ctx context.Context, p mailInputLinux) (*ToolResult, error) {
	query := p.Query
	if p.Mailbox != "" {
		query = fmt.Sprintf("folder:%s and (%s)", p.Mailbox, query)
	}

	args := []string{"search", "--format=text", "--output=summary", "--limit=" + strconv.Itoa(p.Count), query}
	cmd := exec.CommandContext(ctx, "notmuch", args...)
	out, err := cmd.CombinedOutput()
	if err != nil {
		output := strings.TrimSpace(string(out))
		if output == "" {
			return &ToolResult{Content: fmt.Sprintf("No emails found matching '%s'", p.Query)}, nil
		}
		return &ToolResult{Content: fmt.Sprintf("Search failed: %v\nOutput: %s", err, output), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" {
		return &ToolResult{Content: fmt.Sprintf("No emails found matching '%s'", p.Query)}, nil
	}

	lines := strings.Split(output, "\n")
	return &ToolResult{Content: fmt.Sprintf("Found %d emails matching '%s':\n%s", len(lines), p.Query, output)}, nil
}

func (t *MailTool) listAccounts(ctx context.Context) (*ToolResult, error) {
	var accounts []string

	// Check for mutt config
	muttrc := filepath.Join(os.Getenv("HOME"), ".muttrc")
	if _, err := os.Stat(muttrc); err == nil {
		accounts = append(accounts, "mutt: configured via ~/.muttrc")
	}

	neomuttrc := filepath.Join(os.Getenv("HOME"), ".neomuttrc")
	if _, err := os.Stat(neomuttrc); err == nil {
		accounts = append(accounts, "neomutt: configured via ~/.neomuttrc")
	}

	// Check for mailrc
	mailrc := filepath.Join(os.Getenv("HOME"), ".mailrc")
	if _, err := os.Stat(mailrc); err == nil {
		accounts = append(accounts, "mail: configured via ~/.mailrc")
	}

	// Check notmuch config
	notmuchConfig := filepath.Join(os.Getenv("HOME"), ".notmuch-config")
	if _, err := os.Stat(notmuchConfig); err == nil {
		// Try to get email from notmuch config
		cmd := exec.CommandContext(ctx, "notmuch", "config", "get", "user.primary_email")
		out, err := cmd.Output()
		if err == nil {
			email := strings.TrimSpace(string(out))
			accounts = append(accounts, fmt.Sprintf("notmuch: %s", email))
		} else {
			accounts = append(accounts, "notmuch: configured via ~/.notmuch-config")
		}
	}

	if len(accounts) == 0 {
		return &ToolResult{
			Content: "No email accounts configured.\n\n" +
				"To configure email:\n" +
				"  - mutt/neomutt: Create ~/.muttrc with your IMAP/SMTP settings\n" +
				"  - notmuch: Run 'notmuch setup' to configure\n" +
				"  - s-nail: Create ~/.mailrc with your settings",
		}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Configured accounts:\n%s", strings.Join(accounts, "\n"))}, nil
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewMailTool(),
		Platforms: []string{PlatformLinux},
		Category:  "productivity",
	})
}
