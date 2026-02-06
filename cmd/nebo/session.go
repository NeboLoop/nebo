package cli

import (
	"fmt"
	"os"

	"github.com/spf13/cobra"

	agentcfg "github.com/nebolabs/nebo/internal/agent/config"
	"github.com/nebolabs/nebo/internal/agent/session"
	"github.com/nebolabs/nebo/internal/db"
)

// sessionCmd creates the session command
func SessionCmd() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "session",
		Short: "Manage chat sessions",
	}

	cmd.AddCommand(&cobra.Command{
		Use:   "list",
		Short: "List all sessions",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			listSessions(cfg)
		},
	})

	cmd.AddCommand(&cobra.Command{
		Use:   "clear [session-key]",
		Short: "Clear a session's history",
		Run: func(cmd *cobra.Command, args []string) {
			cfg := loadAgentConfig()
			key := sessionKey
			if len(args) > 0 {
				key = args[0]
			}
			clearSession(cfg, key)
		},
	})

	return cmd
}

// listSessions lists all sessions
func listSessions(cfg *agentcfg.Config) {
	store, err := db.NewSQLite(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error opening database: %v\n", err)
		os.Exit(1)
	}
	defer store.Close()

	sessions, err := session.New(store.GetDB())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	list, err := sessions.ListSessions("")
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	if len(list) == 0 {
		fmt.Println("No sessions found.")
		return
	}

	fmt.Println("Sessions:")
	for _, s := range list {
		fmt.Printf("  %s (updated: %s)\n", s.SessionKey, s.UpdatedAt.Format("2006-01-02 15:04:05"))
	}
}

// clearSession clears a session's history
func clearSession(cfg *agentcfg.Config, key string) {
	store, err := db.NewSQLite(cfg.DBPath())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error opening database: %v\n", err)
		os.Exit(1)
	}
	defer store.Close()

	sessions, err := session.New(store.GetDB())
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	sess, err := sessions.GetOrCreate(key, "")
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	if err := sessions.Reset(sess.ID); err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}

	fmt.Printf("Cleared session: %s\n", key)
}
