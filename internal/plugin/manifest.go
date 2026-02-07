// Package plugin provides the SettingsManifest system for plugin configuration.
//
// This follows the iPhone Settings.bundle model:
//   - Plugins declare their settings schema via SettingsManifest()
//   - The UI renders settings dynamically from the manifest
//   - Values are stored in the DB as key-value pairs (plugin_settings table)
//   - Changes are hot-reloaded via OnSettingsChanged() without restart
//
// Nebo = iPhone, NeboLoop = App Store.
package plugin

// Field type constants (mirrors iOS PSSpecifier types)
const (
	FieldText     = "text"     // Single-line text input
	FieldPassword = "password" // Masked text input (stored with is_secret=1)
	FieldToggle   = "toggle"  // Boolean on/off switch
	FieldSelect   = "select"  // Dropdown / picker
	FieldNumber   = "number"  // Numeric input
	FieldURL      = "url"     // URL input with validation
)

// SettingsField represents a single configurable field
// (like an iOS PSTextFieldSpecifier / PSToggleSwitchSpecifier)
type SettingsField struct {
	Key         string   `json:"key"`                    // Config map key (e.g., "broker")
	Title       string   `json:"title"`                  // Human-readable label
	Description string   `json:"description,omitempty"`  // Help text shown below field
	Type        string   `json:"type"`                   // FieldText, FieldPassword, FieldToggle, etc.
	Default     string   `json:"default,omitempty"`      // Default value
	Required    bool     `json:"required,omitempty"`     // Is this field required?
	Options     []Option `json:"options,omitempty"`       // For FieldSelect type
	Placeholder string   `json:"placeholder,omitempty"`  // Placeholder text
	Validation  string   `json:"validation,omitempty"`   // Optional regex pattern
	Secret      bool     `json:"secret,omitempty"`       // Stored encrypted / masked in UI
}

// Option represents a selectable option for FieldSelect fields
type Option struct {
	Label string `json:"label"`
	Value string `json:"value"`
}

// SettingsGroup groups related fields (like an iOS Settings section)
type SettingsGroup struct {
	Title       string         `json:"title"`
	Description string         `json:"description,omitempty"`
	Fields      []SettingsField `json:"fields"`
}

// SettingsManifest declares what settings a plugin needs.
// This is the schema — it tells the UI what to render.
// Actual values live in the plugin_settings DB table.
type SettingsManifest struct {
	Groups []SettingsGroup `json:"groups"`
}

// Configurable is implemented by plugins that declare their settings schema.
// When settings change in the DB, OnSettingsChanged is called for hot-reload.
type Configurable interface {
	// Manifest returns the settings schema for this plugin.
	// Called once at registration to populate plugin_registry.settings_manifest.
	Manifest() SettingsManifest

	// OnSettingsChanged is called when any setting value changes in the DB.
	// The full current settings map is provided. The plugin should apply
	// the new values without requiring a restart.
	OnSettingsChanged(settings map[string]string) error
}
