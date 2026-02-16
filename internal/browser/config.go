package browser

import (
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"

	"github.com/neboloop/nebo/internal/defaults"
)

// Config is the browser configuration from nebo config.
type Config struct {
	// Enabled controls whether browser automation is available.
	Enabled bool `json:"enabled" yaml:"enabled"`

	// ControlPort is the port for the browser control HTTP server.
	ControlPort int `json:"controlPort,omitempty" yaml:"controlPort,omitempty"`

	// ExecutablePath overrides auto-detection of Chrome.
	ExecutablePath string `json:"executablePath,omitempty" yaml:"executablePath,omitempty"`

	// Headless runs browsers without UI.
	Headless bool `json:"headless,omitempty" yaml:"headless,omitempty"`

	// NoSandbox disables Chrome sandbox (needed in some containers).
	NoSandbox bool `json:"noSandbox,omitempty" yaml:"noSandbox,omitempty"`

	// Profiles defines named browser profiles.
	Profiles map[string]ProfileConfig `json:"profiles,omitempty" yaml:"profiles,omitempty"`
}

// ProfileConfig configures a browser profile.
type ProfileConfig struct {
	// CDPPort is the Chrome DevTools Protocol port for this profile.
	CDPPort int `json:"cdpPort,omitempty" yaml:"cdpPort,omitempty"`

	// CDPUrl overrides the CDP URL (for remote browsers).
	CDPUrl string `json:"cdpUrl,omitempty" yaml:"cdpUrl,omitempty"`

	// Driver is "nebo" (managed) or "extension" (Chrome extension relay).
	Driver string `json:"driver,omitempty" yaml:"driver,omitempty"`

	// Color is the profile theme color (hex, e.g. "#FF6B35").
	Color string `json:"color,omitempty" yaml:"color,omitempty"`
}

// ResolvedConfig is the fully resolved browser configuration.
type ResolvedConfig struct {
	Enabled        bool
	ControlPort    int
	ExecutablePath string
	Headless       bool
	NoSandbox      bool
	Profiles       map[string]*ResolvedProfile
}

// ResolvedProfile is a fully resolved browser profile.
type ResolvedProfile struct {
	Name         string
	CDPPort      int
	CDPUrl       string
	CDPIsLoopback bool
	Driver       string
	Color        string
	UserDataDir  string
}

// DefaultConfig returns the default browser configuration.
func DefaultConfig() Config {
	return Config{
		Enabled:     true,
		ControlPort: DefaultControlPort,
		Profiles: map[string]ProfileConfig{
			"nebo": {
				CDPPort: DefaultCDPPort,
				Driver:  DriverNebo,
				Color:   DefaultProfileColor,
			},
			"chrome": {
				// CDPPort is set automatically to ExtensionRelayPort in resolveProfile
				Driver: DriverExtension,
				Color:  "#4285F4", // Google blue
			},
		},
	}
}

// ResolveConfig resolves a browser config with defaults applied.
func ResolveConfig(cfg Config) *ResolvedConfig {
	resolved := &ResolvedConfig{
		Enabled:        cfg.Enabled,
		ControlPort:    cfg.ControlPort,
		ExecutablePath: cfg.ExecutablePath,
		Headless:       cfg.Headless,
		NoSandbox:      cfg.NoSandbox,
		Profiles:       make(map[string]*ResolvedProfile),
	}

	if resolved.ControlPort == 0 {
		resolved.ControlPort = DefaultControlPort
	}

	// If no profiles defined, use defaults
	profiles := cfg.Profiles
	if len(profiles) == 0 {
		profiles = DefaultConfig().Profiles
	}

	for name, profile := range profiles {
		resolved.Profiles[name] = resolveProfile(name, profile)
	}

	return resolved
}

func resolveProfile(name string, cfg ProfileConfig) *ResolvedProfile {
	profile := &ResolvedProfile{
		Name:   name,
		Driver: cfg.Driver,
		Color:  cfg.Color,
	}

	// Default driver based on profile name
	if profile.Driver == "" {
		if name == "chrome" {
			profile.Driver = DriverExtension
		} else {
			profile.Driver = DriverNebo
		}
	}

	// Default color
	if profile.Color == "" {
		if profile.Driver == DriverExtension {
			profile.Color = "#4285F4" // Google blue
		} else {
			profile.Color = DefaultProfileColor
		}
	}

	// CDP port/URL
	if cfg.CDPUrl != "" {
		profile.CDPUrl = cfg.CDPUrl
		profile.CDPPort = portFromURL(cfg.CDPUrl)
		profile.CDPIsLoopback = isLoopbackURL(cfg.CDPUrl)
	} else if profile.Driver == DriverExtension {
		// Extension driver uses the relay mounted on nebo's main server
		profile.CDPPort = ExtensionRelayPort
		profile.CDPUrl = fmt.Sprintf("ws://127.0.0.1:%d/relay/cdp", ExtensionRelayPort)
		profile.CDPIsLoopback = true
	} else {
		port := cfg.CDPPort
		if port == 0 {
			port = DefaultCDPPort
		}
		profile.CDPPort = port
		profile.CDPUrl = fmt.Sprintf("http://127.0.0.1:%d", port)
		profile.CDPIsLoopback = true
	}

	// User data directory for managed profiles
	if profile.Driver == DriverNebo {
		profile.UserDataDir = resolveUserDataDir(name)
	}

	return profile
}

func portFromURL(rawURL string) int {
	u, err := url.Parse(rawURL)
	if err != nil {
		return DefaultCDPPort
	}
	port := u.Port()
	if port == "" {
		if u.Scheme == "https" || u.Scheme == "wss" {
			return 443
		}
		return 80
	}
	var p int
	fmt.Sscanf(port, "%d", &p)
	if p == 0 {
		return DefaultCDPPort
	}
	return p
}

func isLoopbackURL(rawURL string) bool {
	u, err := url.Parse(rawURL)
	if err != nil {
		return false
	}
	host := u.Hostname()
	return host == "127.0.0.1" || host == "localhost" || host == "::1"
}

func resolveUserDataDir(profileName string) string {
	configDir := neboConfigDir()
	return filepath.Join(configDir, "browser", profileName, "user-data")
}

func neboConfigDir() string {
	if dir := os.Getenv("NEBO_CONFIG_DIR"); dir != "" {
		return dir
	}
	dir, err := defaults.DataDir()
	if err != nil {
		home, _ := os.UserHomeDir()
		return filepath.Join(home, ".config", "nebo")
	}
	return dir
}

// GetProfile returns a resolved profile by name.
func (c *ResolvedConfig) GetProfile(name string) *ResolvedProfile {
	if name == "" {
		name = DefaultProfileName
	}
	// Normalize: "default" -> "nebo"
	if strings.ToLower(name) == "default" {
		name = DefaultProfileName
	}
	return c.Profiles[name]
}
