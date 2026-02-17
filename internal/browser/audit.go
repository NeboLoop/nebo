package browser

import (
	"log/slog"
	"time"
)

// sensitiveCommands are CDP methods whose params are logged for audit purposes.
var sensitiveCommands = map[string]bool{
	"Runtime.evaluate":              true,
	"Runtime.callFunctionOn":        true,
	"Page.navigate":                 true,
	"Network.setCookie":             true,
	"Network.deleteCookies":         true,
	"Network.setExtraHTTPHeaders":   true,
	"Storage.clearDataForOrigin":    true,
	"Input.dispatchKeyEvent":        true,
	"DOM.setAttributeValue":         true,
	"Page.setDocumentContent":       true,
	"Fetch.fulfillRequest":          true,
	"Debugger.setBreakpointByUrl":   true,
	"Security.setIgnoreCertErrors":  true,
	"Browser.grantPermissions":      true,
	"Target.createBrowserContext":   true,
	"Emulation.setUserAgentOverride": true,
}

type cdpAuditLogger struct {
	logger *slog.Logger
}

func newCDPAuditLogger() *cdpAuditLogger {
	return &cdpAuditLogger{
		logger: slog.Default().With("component", "cdp-relay"),
	}
}

func (l *cdpAuditLogger) logCommand(clientID string, method string, sessionID string) {
	if l == nil {
		return
	}

	attrs := []any{
		"client", truncateID(clientID),
		"method", method,
		"ts", time.Now().Unix(),
	}

	if sessionID != "" {
		attrs = append(attrs, "session", truncateID(sessionID))
	}

	if sensitiveCommands[method] {
		l.logger.Warn("cdp_sensitive_command", attrs...)
	} else {
		l.logger.Info("cdp_command", attrs...)
	}
}

func truncateID(id string) string {
	if len(id) > 8 {
		return id[:8]
	}
	return id
}
