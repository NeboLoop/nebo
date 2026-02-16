package main

import (
	"database/sql"
	"fmt"
	"log"
	"os"

	"github.com/neboloop/nebo/internal/db/migrations"

	_ "modernc.org/sqlite"
)

func main() {
	if len(os.Args) < 2 {
		fmt.Println("Usage: go run cmd/migrate/main.go <command>")
		fmt.Println("Commands:")
		fmt.Println("  status - Show migration status")
		fmt.Println("  up     - Apply all pending migrations")
		fmt.Println("  down   - Rollback last migration")
		os.Exit(1)
	}

	dbPath := os.Getenv("SQLITE_PATH")
	if dbPath == "" {
		dbPath = "data/nebo.db"
	}

	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		log.Fatalf("Failed to open database: %v", err)
	}
	defer db.Close()

	switch os.Args[1] {
	case "status":
		if err := migrations.Status(db); err != nil {
			log.Fatalf("Failed to get status: %v", err)
		}
	case "up":
		if err := migrations.Run(db); err != nil {
			log.Fatalf("Failed to run migrations: %v", err)
		}
		fmt.Println("Migrations applied successfully")
	case "down":
		if err := migrations.Down(db); err != nil {
			log.Fatalf("Failed to rollback migration: %v", err)
		}
		fmt.Println("Migration rolled back successfully")
	default:
		fmt.Printf("Unknown command: %s\n", os.Args[1])
		os.Exit(1)
	}
}
