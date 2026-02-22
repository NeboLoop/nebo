// Package settings provides DB-backed CRUD for app configuration.
//
//   - Values are stored in the DB as key-value pairs (plugin_settings table)
//   - Changes are hot-reloaded via OnSettingsChanged() without restart
//   - Apps serve their own settings UI via HandleRequest (no manifest schema needed)
package settings

// Configurable is implemented by apps that support hot-reload of settings.
// When settings change in the DB, OnSettingsChanged is called.
type Configurable interface {
	// OnSettingsChanged is called when any setting value changes in the DB.
	// The full current settings map is provided. The app should apply
	// the new values without requiring a restart.
	OnSettingsChanged(settings map[string]string) error
}
