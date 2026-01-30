package config

import (
	"os"
	"path/filepath"
	"strings"

	"gopkg.in/yaml.v3"
)

// LoadFromBytes loads configuration from YAML bytes with environment variable expansion
func LoadFromBytes(data []byte) (Config, error) {
	var c Config
	expanded := os.ExpandEnv(string(data))
	if err := yaml.Unmarshal([]byte(expanded), &c); err != nil {
		return c, err
	}
	// Apply defaults
	applyDefaults(&c)
	return c, nil
}

// applyDefaults sets default values for unset config fields
func applyDefaults(c *Config) {
	if c.Host == "" {
		c.Host = "0.0.0.0"
	}
	if c.Port == 0 {
		c.Port = 29875
	}
	if c.Auth.RefreshTokenExpire == 0 {
		c.Auth.RefreshTokenExpire = 604800
	}
	if c.Database.SQLitePath == "" {
		// Use ~/.nebo/data/gobot.db as the canonical database location
		home, _ := os.UserHomeDir()
		c.Database.SQLitePath = filepath.Join(home, ".nebo", "data", "gobot.db")
	}
	if c.Security.CSRFEnabled == "" {
		c.Security.CSRFEnabled = "true"
	}
	if c.Security.CSRFTokenExpiry == 0 {
		c.Security.CSRFTokenExpiry = 43200
	}
	if c.Security.CSRFSecureCookie == "" {
		c.Security.CSRFSecureCookie = "true"
	}
	if c.Security.RateLimitEnabled == "" {
		c.Security.RateLimitEnabled = "true"
	}
	if c.Security.RateLimitRequests == 0 {
		c.Security.RateLimitRequests = 100
	}
	if c.Security.RateLimitInterval == 0 {
		c.Security.RateLimitInterval = 60
	}
	if c.Security.RateLimitBurst == 0 {
		c.Security.RateLimitBurst = 20
	}
	if c.Security.AuthRateLimitRequests == 0 {
		c.Security.AuthRateLimitRequests = 5
	}
	if c.Security.AuthRateLimitInterval == 0 {
		c.Security.AuthRateLimitInterval = 60
	}
	if c.Security.EnableSecurityHeaders == "" {
		c.Security.EnableSecurityHeaders = "true"
	}
	if c.Security.MaxRequestBodySize == 0 {
		c.Security.MaxRequestBodySize = 10485760
	}
	if c.Security.MaxURLLength == 0 {
		c.Security.MaxURLLength = 2048
	}
	if c.Email.SMTPPort == 0 {
		c.Email.SMTPPort = 587
	}
	if c.Email.FromName == "" {
		c.Email.FromName = "gobot"
	}
	if c.Email.BaseURL == "" {
		c.Email.BaseURL = "http://localhost:27458"
	}
	if c.Features.NotificationsEnabled == "" {
		c.Features.NotificationsEnabled = "true"
	}
}

// parseBool parses a string as boolean with a default value.
// Accepts: "true", "1", "yes" as true; empty or other values return default.
func parseBool(s string, defaultVal bool) bool {
	s = strings.TrimSpace(strings.ToLower(s))
	if s == "" {
		return defaultVal
	}
	return s == "true" || s == "1" || s == "yes"
}

type Config struct {
	// Server configuration
	Name string `yaml:"Name"`
	Host string `yaml:"Host"`
	Port int    `yaml:"Port"`
	App struct {
		BaseURL        string `yaml:"BaseURL"`
		Domain         string `yaml:"Domain"`
		ProductionMode string `yaml:"ProductionMode"`
		AdminEmail     string `yaml:"AdminEmail"`
	} `yaml:"App"`
	Auth struct {
		AccessSecret       string `yaml:"AccessSecret"`
		AccessExpire       int64  `yaml:"AccessExpire"`
		RefreshTokenExpire int64  `yaml:"RefreshTokenExpire"`
	} `yaml:"Auth"`
	Database struct {
		SQLitePath string `yaml:"SQLitePath"`
	} `yaml:"Database"`
	Security struct {
		CSRFEnabled           string `yaml:"CSRFEnabled"`
		CSRFSecret            string `yaml:"CSRFSecret"`
		CSRFTokenExpiry       int64  `yaml:"CSRFTokenExpiry"`
		CSRFSecureCookie      string `yaml:"CSRFSecureCookie"`
		RateLimitEnabled      string `yaml:"RateLimitEnabled"`
		RateLimitRequests     int    `yaml:"RateLimitRequests"`
		RateLimitInterval     int    `yaml:"RateLimitInterval"`
		RateLimitBurst        int    `yaml:"RateLimitBurst"`
		AuthRateLimitRequests int    `yaml:"AuthRateLimitRequests"`
		AuthRateLimitInterval int    `yaml:"AuthRateLimitInterval"`
		EnableSecurityHeaders string `yaml:"EnableSecurityHeaders"`
		ContentSecurityPolicy string `yaml:"ContentSecurityPolicy"`
		AllowedOrigins        string `yaml:"AllowedOrigins"`
		ForceHTTPS            string `yaml:"ForceHTTPS"`
		MaxRequestBodySize    int64  `yaml:"MaxRequestBodySize"`
		MaxURLLength          int    `yaml:"MaxURLLength"`
	} `yaml:"Security"`
	Email struct {
		SMTPHost    string `yaml:"SMTPHost"`
		SMTPPort    int    `yaml:"SMTPPort"`
		SMTPUser    string `yaml:"SMTPUser"`
		SMTPPass    string `yaml:"SMTPPass"`
		FromAddress string `yaml:"FromAddress"`
		FromName    string `yaml:"FromName"`
		ReplyTo     string `yaml:"ReplyTo"`
		BaseURL     string `yaml:"BaseURL"`
	} `yaml:"Email"`
	OAuth struct {
		GoogleEnabled      string `yaml:"GoogleEnabled"`
		GoogleClientID     string `yaml:"GoogleClientID"`
		GoogleClientSecret string `yaml:"GoogleClientSecret"`
		GitHubEnabled      string `yaml:"GitHubEnabled"`
		GitHubClientID     string `yaml:"GitHubClientID"`
		GitHubClientSecret string `yaml:"GitHubClientSecret"`
		CallbackBaseURL    string `yaml:"CallbackBaseURL"`
	} `yaml:"OAuth"`
	Features struct {
		NotificationsEnabled string `yaml:"NotificationsEnabled"`
		OAuthEnabled         string `yaml:"OAuthEnabled"`
	} `yaml:"Features"`
}

func (c Config) IsProductionMode() bool {
	return parseBool(c.App.ProductionMode, false)
}

func (c Config) IsCSRFEnabled() bool {
	return parseBool(c.Security.CSRFEnabled, true)
}

func (c Config) IsCSRFSecureCookie() bool {
	return parseBool(c.Security.CSRFSecureCookie, true)
}

func (c Config) IsRateLimitEnabled() bool {
	return parseBool(c.Security.RateLimitEnabled, true)
}

func (c Config) IsSecurityHeadersEnabled() bool {
	return parseBool(c.Security.EnableSecurityHeaders, true)
}

func (c Config) IsForceHTTPS() bool {
	return parseBool(c.Security.ForceHTTPS, false)
}

func (c Config) IsGoogleOAuthEnabled() bool {
	return parseBool(c.OAuth.GoogleEnabled, false)
}

func (c Config) IsGitHubOAuthEnabled() bool {
	return parseBool(c.OAuth.GitHubEnabled, false)
}

func (c Config) IsNotificationsEnabled() bool {
	return parseBool(c.Features.NotificationsEnabled, true)
}

func (c Config) IsOAuthEnabled() bool {
	return parseBool(c.Features.OAuthEnabled, false)
}
