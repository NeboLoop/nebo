// genapi reads Rust source code and generates the TypeScript API client.
//
// It produces:
//   - neboComponents.ts  — TypeScript interfaces from Rust structs + handler responses
//   - nebo.ts            — typed API functions from route definitions
//
// Usage:
//
//	go run ./scripts/genapi            (from repo root)
//	make gen                           (via Makefile)
package main

import (
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

func main() {
	root := detectRoot()

	routesDir := filepath.Join(root, "crates/server/src/routes")
	handlersDir := filepath.Join(root, "crates/server/src/handlers")
	outDir := filepath.Join(root, "app/src/lib/api")

	// Directories to scan for Rust structs.
	structDirs := []string{
		filepath.Join(root, "crates/db/src"),
		filepath.Join(root, "crates/types/src"),
		filepath.Join(root, "crates/server/src/handlers"),
	}

	// ── 1. Parse Rust structs ───────────────────────────────────────────
	fmt.Println("Scanning Rust structs...")
	allStructs := make(map[string]*RustStruct)
	for _, dir := range structDirs {
		structs := scanStructs(dir)
		for _, s := range structs {
			allStructs[s.Name] = s
		}
	}
	fmt.Printf("  Found %d serializable structs\n", len(allStructs))

	// ── 2. Parse routes ─────────────────────────────────────────────────
	fmt.Println("Scanning routes...")
	routes := scanRoutes(routesDir)
	fmt.Printf("  Found %d routes\n", len(routes))

	// ── 3. Parse store method return types (for tracing handler vars) ──
	fmt.Println("Scanning store method types...")
	queriesDir := filepath.Join(root, "crates/db/src/queries")
	storeMethodTypes := scanStoreMethodTypes(queriesDir)
	fmt.Printf("  Found %d store methods\n", len(storeMethodTypes))

	// ── 4. Parse handler responses ──────────────────────────────────────
	fmt.Println("Scanning handler responses...")
	handlers := scanHandlers(handlersDir, allStructs, storeMethodTypes)
	fmt.Printf("  Found %d handler functions\n", len(handlers))

	// ── 5. Parse WebSocket events ───────────────────────────────────────
	fmt.Println("Scanning WebSocket events...")
	wsEvents := scanWSEvents(filepath.Join(handlersDir, "ws.rs"))
	fmt.Printf("  Found %d WS event types\n", len(wsEvents))

	// ── 6. Generate neboComponents.ts ───────────────────────────────────
	fmt.Println("Generating neboComponents.ts...")
	componentsPath := filepath.Join(outDir, "neboComponents.ts")
	generateComponents(componentsPath, allStructs, handlers, wsEvents)

	// ── 7. Generate nebo.ts ─────────────────────────────────────────────
	fmt.Println("Generating nebo.ts...")
	apiPath := filepath.Join(outDir, "nebo.ts")
	generateAPI(apiPath, routes, handlers)

	fmt.Printf("\nDone. Generated:\n  %s\n  %s\n", componentsPath, apiPath)
}

// detectRoot walks up from cwd looking for Cargo.toml to find the repo root.
func detectRoot() string {
	dir, _ := os.Getwd()
	for {
		if _, err := os.Stat(filepath.Join(dir, "Cargo.toml")); err == nil {
			return dir
		}
		parent := filepath.Dir(dir)
		if parent == dir {
			break
		}
		dir = parent
	}
	// Fallback: assume cwd IS scripts/genapi, walk up 2.
	dir, _ = os.Getwd()
	candidate := filepath.Join(dir, "../..")
	if _, err := os.Stat(filepath.Join(candidate, "Cargo.toml")); err == nil {
		abs, _ := filepath.Abs(candidate)
		return abs
	}
	fmt.Fprintln(os.Stderr, "error: could not find repo root (Cargo.toml)")
	os.Exit(1)
	return ""
}

// ── Sorting helpers ─────────────────────────────────────────────────────

func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	return keys
}

func indent(s string, n int) string {
	prefix := strings.Repeat("\t", n)
	lines := strings.Split(s, "\n")
	for i, l := range lines {
		if l != "" {
			lines[i] = prefix + l
		}
	}
	return strings.Join(lines, "\n")
}
