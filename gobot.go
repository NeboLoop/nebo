package main

import (
	_ "embed"
	"fmt"
	"os"

	cli "gobot/cmd/gobot"
	"gobot/internal/config"
	"gobot/internal/local"

	"github.com/zeromicro/go-zero/core/conf"
)

//go:embed etc/gobot.yaml
var embeddedConfig []byte

func main() {
	// Load embedded config (defaults)
	var c config.Config
	if err := conf.LoadFromYamlBytes([]byte(os.ExpandEnv(string(embeddedConfig))), &c); err != nil {
		fmt.Printf("Failed to load embedded config: %v\n", err)
		os.Exit(1)
	}

	// Load local settings (auto-generates secret on first run)
	settings, err := local.LoadSettings()
	if err != nil {
		fmt.Printf("Failed to load local settings: %v\n", err)
		os.Exit(1)
	}

	// Override auth config with local settings
	c.Auth.AccessSecret = settings.AccessSecret
	if settings.AccessExpire > 0 {
		c.Auth.AccessExpire = settings.AccessExpire
	}
	if settings.RefreshTokenExpire > 0 {
		c.Auth.RefreshTokenExpire = settings.RefreshTokenExpire
	}

	// Pass config to CLI and execute
	if err := cli.SetupRootCmd(&c).Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
