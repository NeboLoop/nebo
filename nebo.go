package main

import (
	_ "embed"
	"fmt"
	"os"
	"path/filepath"

	cli "github.com/nebolabs/nebo/cmd/nebo"
	"github.com/nebolabs/nebo/internal/config"
	"github.com/nebolabs/nebo/internal/defaults"
	"github.com/nebolabs/nebo/internal/local"

	"github.com/joho/godotenv"
)

//go:embed etc/nebo.yaml
var embeddedConfig []byte

func main() {
	// Load .env file if present (ignore error if not found)
	_ = godotenv.Load()

	// Load embedded config (defaults)
	c, err := config.LoadFromBytes(embeddedConfig)
	if err != nil {
		fmt.Printf("Failed to load embedded config: %v\n", err)
		os.Exit(1)
	}

	// Override database path to use <data_dir>/data/nebo.db
	dataDir, err := defaults.DataDir()
	if err == nil {
		dbDir := filepath.Join(dataDir, "data")
		os.MkdirAll(dbDir, 0755)
		c.Database.SQLitePath = filepath.Join(dbDir, "nebo.db")
		fmt.Printf("Using database: %s\n", c.Database.SQLitePath)
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
