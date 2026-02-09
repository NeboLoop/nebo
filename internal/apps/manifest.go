package apps

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"github.com/nebolabs/nebo/internal/apps/settings"
)

// Known capability types that apps can provide.
const (
	CapGateway  = "gateway"
	CapVision   = "vision"
	CapBrowser  = "browser"
	CapComm     = "comm"
	CapUI       = "ui"
	CapSchedule = "schedule"
)

// Capability prefixes for parameterized capabilities.
const (
	CapPrefixTool    = "tool:"
	CapPrefixChannel = "channel:"
)

// Known permission prefixes.
const (
	// Storage & Config
	PermPrefixNetwork    = "network:"
	PermPrefixFilesystem = "filesystem:"
	PermPrefixSettings   = "settings:"
	PermPrefixCapability = "capability:"

	// Agent Core
	PermPrefixMemory  = "memory:"
	PermPrefixSession = "session:"
	PermPrefixContext = "context:"

	// Execution
	PermPrefixTool     = "tool:"
	PermPrefixShell    = "shell:"
	PermPrefixSubagent = "subagent:"
	PermPrefixLane     = "lane:"

	// Communication
	PermPrefixChannel      = "channel:"
	PermPrefixComm         = "comm:"
	PermPrefixNotification = "notification:"

	// Knowledge
	PermPrefixEmbedding = "embedding:"
	PermPrefixSkill     = "skill:"
	PermPrefixAdvisor   = "advisor:"

	// AI
	PermPrefixModel = "model:"
	PermPrefixMCP   = "mcp:"

	// Storage
	PermPrefixDatabase = "database:"
	PermPrefixStorage  = "storage:"

	// System
	PermPrefixSchedule = "schedule:"
	PermPrefixVoice    = "voice:"
	PermPrefixBrowser  = "browser:"
	PermPrefixOAuth    = "oauth:"
	PermPrefixUser     = "user:"
)

// AppManifest represents an app's manifest.json — the "plist" for Nebo apps.
// Apps declare what they provide (capabilities) and what they need (permissions).
type AppManifest struct {
	ID          string            `json:"id"`
	Name        string            `json:"name"`
	Version     string            `json:"version"`
	Description string            `json:"description,omitempty"`
	Runtime     string            `json:"runtime"`  // "local" or "remote"
	Protocol    string            `json:"protocol"` // "grpc"
	Signature   ManifestSignature `json:"signature,omitempty"`
	Provides    []string          `json:"provides"`
	Permissions []string          `json:"permissions"`
	Settings    []SettingsField   `json:"settings,omitempty"`
}

// ManifestSignature holds NeboLoop code-signing information.
type ManifestSignature struct {
	Algorithm string `json:"algorithm,omitempty"` // e.g. "ed25519"
	PublicKey string `json:"public_key,omitempty"`
	Signature string `json:"signature,omitempty"` // Base64 signature of manifest (minus signature field)
	BinarySig string `json:"binary_sig,omitempty"` // Base64 signature of binary
}

// SettingsField describes a configurable setting for the app UI.
// Mirrors plugin.SettingsField for compatibility.
type SettingsField struct {
	Key         string   `json:"key"`
	Title       string   `json:"title"`
	Type        string   `json:"type"` // text, password, toggle, select, number, url
	Required    bool     `json:"required,omitempty"`
	Default     string   `json:"default,omitempty"`
	Placeholder string   `json:"placeholder,omitempty"`
	Options     []Option `json:"options,omitempty"`
	Description string   `json:"description,omitempty"`
	Secret      bool     `json:"secret,omitempty"`
}

// Option is a choice for select-type settings fields.
type Option struct {
	Label string `json:"label"`
	Value string `json:"value"`
}

// LoadManifest reads and validates a manifest.json from an app directory.
func LoadManifest(dir string) (*AppManifest, error) {
	path := filepath.Join(dir, "manifest.json")
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("read manifest: %w", err)
	}

	var m AppManifest
	if err := json.Unmarshal(data, &m); err != nil {
		return nil, fmt.Errorf("parse manifest: %w", err)
	}

	if err := ValidateManifest(&m); err != nil {
		return nil, fmt.Errorf("invalid manifest: %w", err)
	}

	return &m, nil
}

// ValidateManifest checks that required fields are present and values are valid.
func ValidateManifest(m *AppManifest) error {
	if m.ID == "" {
		return fmt.Errorf("missing required field: id")
	}
	if m.Name == "" {
		return fmt.Errorf("missing required field: name")
	}
	if m.Version == "" {
		return fmt.Errorf("missing required field: version")
	}
	if len(m.Provides) == 0 {
		return fmt.Errorf("missing required field: provides (must declare at least one capability)")
	}
	if m.Protocol != "" && m.Protocol != "grpc" {
		return fmt.Errorf("unsupported protocol: %s (only grpc is supported)", m.Protocol)
	}
	if m.Runtime != "" && m.Runtime != "local" && m.Runtime != "remote" {
		return fmt.Errorf("unsupported runtime: %s (must be local or remote)", m.Runtime)
	}

	for _, cap := range m.Provides {
		if !isValidCapability(cap) {
			return fmt.Errorf("invalid capability: %s", cap)
		}
	}

	for _, perm := range m.Permissions {
		if !isValidPermission(perm) {
			return fmt.Errorf("invalid permission: %s", perm)
		}
	}

	return nil
}

// VerifySignature is now implemented in signing.go as VerifyAppSignatures.
// This stub exists for backward compatibility during the transition.
// When keyProvider is nil (dev mode), signature verification is skipped.
func VerifySignature(m *AppManifest, binaryPath string) error {
	return nil
}

// HasCapability returns true if the manifest declares the given capability.
func HasCapability(m *AppManifest, cap string) bool {
	for _, c := range m.Provides {
		if c == cap {
			return true
		}
	}
	return false
}

// HasCapabilityPrefix returns true if any capability starts with the given prefix.
func HasCapabilityPrefix(m *AppManifest, prefix string) bool {
	for _, c := range m.Provides {
		if strings.HasPrefix(c, prefix) {
			return true
		}
	}
	return false
}

// HasPermissionPrefix returns true if any permission starts with the given prefix.
func HasPermissionPrefix(m *AppManifest, prefix string) bool {
	for _, p := range m.Permissions {
		if strings.HasPrefix(p, prefix) {
			return true
		}
	}
	return false
}

// CheckPermission returns true if the manifest includes the given permission.
func CheckPermission(m *AppManifest, perm string) bool {
	for _, p := range m.Permissions {
		if p == perm {
			return true
		}
		// Check wildcard: "network:*" matches any "network:..." check
		prefix, _, ok := strings.Cut(p, ":")
		if ok {
			reqPrefix, _, reqOk := strings.Cut(perm, ":")
			if reqOk && prefix == reqPrefix && strings.HasSuffix(p, ":*") {
				return true
			}
		}
	}
	return false
}

// ToSettingsManifest converts the app's settings fields to the plugin.SettingsManifest
// format for compatibility with the existing plugin settings UI.
func (m *AppManifest) ToSettingsManifest() settings.SettingsManifest {
	if len(m.Settings) == 0 {
		return settings.SettingsManifest{}
	}

	fields := make([]settings.SettingsField, len(m.Settings))
	for i, f := range m.Settings {
		var opts []settings.Option
		for _, o := range f.Options {
			opts = append(opts, settings.Option{Label: o.Label, Value: o.Value})
		}
		fields[i] = settings.SettingsField{
			Key:         f.Key,
			Title:       f.Title,
			Type:        f.Type,
			Required:    f.Required,
			Default:     f.Default,
			Placeholder: f.Placeholder,
			Options:     opts,
			Description: f.Description,
			Secret:      f.Secret,
		}
	}

	return settings.SettingsManifest{
		Groups: []settings.SettingsGroup{
			{
				Title:  m.Name + " Settings",
				Fields: fields,
			},
		},
	}
}

func isValidCapability(cap string) bool {
	switch cap {
	case CapGateway, CapVision, CapBrowser, CapComm, CapUI, CapSchedule:
		return true
	}
	if strings.HasPrefix(cap, CapPrefixTool) || strings.HasPrefix(cap, CapPrefixChannel) {
		return true
	}
	return false
}

// validPermissionPrefixes lists all recognized permission prefixes.
// Unknown prefixes are rejected — apps must use the documented taxonomy.
var validPermissionPrefixes = []string{
	PermPrefixNetwork, PermPrefixFilesystem, PermPrefixSettings, PermPrefixCapability,
	PermPrefixMemory, PermPrefixSession, PermPrefixContext,
	PermPrefixTool, PermPrefixShell, PermPrefixSubagent, PermPrefixLane,
	PermPrefixChannel, PermPrefixComm, PermPrefixNotification,
	PermPrefixEmbedding, PermPrefixSkill, PermPrefixAdvisor,
	PermPrefixModel, PermPrefixMCP,
	PermPrefixDatabase, PermPrefixStorage,
	PermPrefixSchedule, PermPrefixVoice, PermPrefixBrowser, PermPrefixOAuth, PermPrefixUser,
}

func isValidPermission(perm string) bool {
	for _, prefix := range validPermissionPrefixes {
		if strings.HasPrefix(perm, prefix) {
			return true
		}
	}
	return false
}
