//go:build windows

package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"os/exec"
	"strconv"
	"strings"
)

// MailTool provides Windows email integration via Outlook COM automation.
type MailTool struct {
	hasOutlook bool
}

func NewMailTool() *MailTool {
	t := &MailTool{}
	t.hasOutlook = t.checkOutlook()
	return t
}

func (t *MailTool) checkOutlook() bool {
	// Check if Outlook COM is available
	script := `try { $null = New-Object -ComObject Outlook.Application; Write-Output "true" } catch { Write-Output "false" }`
	cmd := exec.Command("powershell", "-NoProfile", "-Command", script)
	out, err := cmd.Output()
	if err != nil {
		return false
	}
	return strings.TrimSpace(string(out)) == "true"
}

func (t *MailTool) Name() string { return "mail" }

func (t *MailTool) Description() string {
	if !t.hasOutlook {
		return "Read and send emails - requires Microsoft Outlook to be installed."
	}
	return "Read and send emails (using Outlook) - check unread, search, and compose messages."
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
			"mailbox": {"type": "string", "description": "Mailbox name (Inbox, Sent Items, etc.)"},
			"query": {"type": "string", "description": "Search query"},
			"count": {"type": "integer", "description": "Number of emails to fetch (default: 10)"}
		},
		"required": ["action"]
	}`)
}

func (t *MailTool) RequiresApproval() bool { return true }

type mailInputWin struct {
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
	if !t.hasOutlook {
		return &ToolResult{
			Content: "Microsoft Outlook is not installed or not accessible.\n\n" +
				"Email integration on Windows requires Outlook. Please install Microsoft Outlook to use this feature.",
			IsError: true,
		}, nil
	}

	var p mailInputWin
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

func (t *MailTool) sendEmail(ctx context.Context, p mailInputWin) (*ToolResult, error) {
	if len(p.To) == 0 {
		return &ToolResult{Content: "At least one recipient is required", IsError: true}, nil
	}
	if p.Subject == "" {
		return &ToolResult{Content: "Subject is required", IsError: true}, nil
	}

	toList := strings.Join(p.To, ";")
	ccList := strings.Join(p.CC, ";")

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$mail = $outlook.CreateItem(0)
$mail.To = "%s"
`, escapeMailPS(toList))

	if ccList != "" {
		script += fmt.Sprintf(`$mail.CC = "%s"
`, escapeMailPS(ccList))
	}

	script += fmt.Sprintf(`$mail.Subject = "%s"
$mail.Body = "%s"
$mail.Send()
Write-Output "Email sent successfully"
`, escapeMailPS(p.Subject), escapeMailPS(p.Body))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to send email: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: fmt.Sprintf("Email sent to %s", strings.Join(p.To, ", "))}, nil
}

func (t *MailTool) readEmails(ctx context.Context, p mailInputWin) (*ToolResult, error) {
	folderID := "6" // olFolderInbox
	folderName := "Inbox"
	if p.Mailbox != "" {
		folderName = p.Mailbox
		switch strings.ToLower(p.Mailbox) {
		case "inbox":
			folderID = "6"
		case "sent", "sent items":
			folderID = "5"
		case "drafts":
			folderID = "16"
		case "deleted", "deleted items", "trash":
			folderID = "3"
		case "outbox":
			folderID = "4"
		default:
			// Try to find folder by name
			folderID = ""
		}
	}

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
`)

	if folderID != "" {
		script += fmt.Sprintf(`$folder = $namespace.GetDefaultFolder(%s)
`, folderID)
	} else {
		script += fmt.Sprintf(`
$inbox = $namespace.GetDefaultFolder(6)
$folder = $null
foreach ($f in $inbox.Parent.Folders) {
    if ($f.Name -eq "%s") {
        $folder = $f
        break
    }
}
if (-not $folder) {
    $folder = $namespace.GetDefaultFolder(6)
}
`, escapeMailPS(folderName))
	}

	script += fmt.Sprintf(`
$items = $folder.Items
$items.Sort("[ReceivedTime]", $true)

$count = 0
$results = @()
foreach ($item in $items) {
    if ($count -ge %d) { break }
    $from = $item.SenderName
    $subject = $item.Subject
    $date = $item.ReceivedTime.ToString("yyyy-MM-dd HH:mm")
    $results += "$date | $from | $subject"
    $count++
}

if ($results.Count -eq 0) {
    Write-Output "No emails found in %s"
} else {
    $results | ForEach-Object { Write-Output $_ }
}
`, p.Count, escapeMailPS(folderName))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to read emails: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	output := strings.TrimSpace(string(out))
	if output == "" || strings.Contains(output, "No emails found") {
		return &ToolResult{Content: fmt.Sprintf("No emails found in %s", folderName)}, nil
	}

	lines := strings.Split(output, "\n")
	return &ToolResult{Content: fmt.Sprintf("Recent emails in %s (%d):\n%s", folderName, len(lines), output)}, nil
}

func (t *MailTool) getUnreadCount(ctx context.Context, p mailInputWin) (*ToolResult, error) {
	script := `
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$inbox = $namespace.GetDefaultFolder(6)
$unread = $inbox.UnReadItemCount
Write-Output $unread
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to get unread count: %v", err), IsError: true}, nil
	}

	count := strings.TrimSpace(string(out))
	return &ToolResult{Content: fmt.Sprintf("Unread emails: %s", count)}, nil
}

func (t *MailTool) searchEmails(ctx context.Context, p mailInputWin) (*ToolResult, error) {
	if p.Query == "" {
		return &ToolResult{Content: "Search query is required", IsError: true}, nil
	}

	script := fmt.Sprintf(`
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$inbox = $namespace.GetDefaultFolder(6)

# Search in subject and body
$filter = "[Subject] LIKE '%%%s%%' OR [Body] LIKE '%%%s%%'"
$items = $inbox.Items.Restrict($filter)
$items.Sort("[ReceivedTime]", $true)

$count = 0
$results = @()
foreach ($item in $items) {
    if ($count -ge %s) { break }
    $from = $item.SenderName
    $subject = $item.Subject
    $date = $item.ReceivedTime.ToString("yyyy-MM-dd HH:mm")
    $results += "$date | $from | $subject"
    $count++
}

if ($results.Count -eq 0) {
    Write-Output "No emails found matching '%s'"
} else {
    Write-Output "Found $($results.Count) emails:"
    $results | ForEach-Object { Write-Output $_ }
}
`, escapeMailPS(p.Query), escapeMailPS(p.Query), strconv.Itoa(p.Count), escapeMailPS(p.Query))

	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to search emails: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func (t *MailTool) listAccounts(ctx context.Context) (*ToolResult, error) {
	script := `
$outlook = New-Object -ComObject Outlook.Application
$namespace = $outlook.GetNamespace("MAPI")
$accounts = @()
foreach ($account in $namespace.Accounts) {
    $accounts += "$($account.DisplayName) <$($account.SmtpAddress)>"
}

if ($accounts.Count -eq 0) {
    Write-Output "No email accounts configured in Outlook"
} else {
    Write-Output "Configured accounts:"
    $accounts | ForEach-Object { Write-Output "  - $_" }
}
`
	cmd := exec.CommandContext(ctx, "powershell", "-NoProfile", "-Command", script)
	out, err := cmd.CombinedOutput()
	if err != nil {
		return &ToolResult{Content: fmt.Sprintf("Failed to list accounts: %v\nOutput: %s", err, string(out)), IsError: true}, nil
	}

	return &ToolResult{Content: strings.TrimSpace(string(out))}, nil
}

func escapeMailPS(s string) string {
	s = strings.ReplaceAll(s, "`", "``")
	s = strings.ReplaceAll(s, `"`, "`\"")
	s = strings.ReplaceAll(s, "$", "`$")
	s = strings.ReplaceAll(s, "\n", "`n")
	return s
}

func init() {
	RegisterCapability(&Capability{
		Tool:      NewMailTool(),
		Platforms: []string{PlatformWindows},
		Category:  "productivity",
	})
}
