package main

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// ── Rust Struct Parsing ─────────────────────────────────────────────────

// RustStruct represents a parsed Rust struct with serde attributes.
type RustStruct struct {
	Name      string
	Fields    []RustField
	RenameAll string // e.g. "camelCase"
	Source    string // source file path
}

// RustField represents a single field in a Rust struct.
type RustField struct {
	Name          string
	RustType      string
	Rename        string // #[serde(rename = "...")]
	Skip          bool   // #[serde(skip_serializing)] or #[serde(skip)]
	Optional      bool   // has #[serde(default)] or type is Option<T>
	SerializeWith string // #[serde(serialize_with = "...")]
	Flatten       bool   // #[serde(flatten)]: the type's fields sit beside this struct's own
}

// scanStructs reads all .rs files in dir (non-recursive) and extracts
// structs that derive Serialize.
func scanStructs(dir string) []*RustStruct {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var result []*RustStruct
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".rs") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		result = append(result, parseStructs(string(data), path)...)
	}
	return result
}

var (
	// Match derive attributes that include Serialize.
	reDerive = regexp.MustCompile(`#\[derive\([^)]*\bSerialize\b[^)]*\)\]`)
	// Match derive attributes that include Deserialize (for query param structs).
	reDeserialize = regexp.MustCompile(`#\[derive\([^)]*\bDeserialize\b[^)]*\)\]`)
	// Match serde container attributes.
	reSerdeCont = regexp.MustCompile(`#\[serde\(([^)]+)\)\]`)
	// Match pub struct Name {
	reStructHead = regexp.MustCompile(`pub\s+struct\s+(\w+)\s*\{`)
	// Match field-level serde attributes.
	reSerdeField = regexp.MustCompile(`#\[serde\(([^)]+)\)\]`)
	// Match a struct field: name: Type, — any visibility: serde writes a
	// private field (`value: Value` on a sheet cell) like a pub one.
	// The type runs to the line's trailing comma, so `BTreeMap<String, f64>`
	// stays whole.
	reField = regexp.MustCompile(`^(?:pub(?:\([^)]*\))?\s+)?(\w+)\s*:\s*(.+?)\s*[,}]?\s*$`)
	// Match Query<StructName> in handler function signatures.
	reQueryType = regexp.MustCompile(`Query<(\w+)>`)
)

func parseStructs(src, path string) []*RustStruct {
	return parseStructsMatching(src, path, reDerive)
}

func parseDeserializeStructs(src, path string) []*RustStruct {
	return parseStructsMatching(src, path, reDeserialize)
}

func parseStructsMatching(src, path string, deriveMatcher *regexp.Regexp) []*RustStruct {
	lines := strings.Split(src, "\n")
	var result []*RustStruct

	i := 0
	for i < len(lines) {
		line := strings.TrimSpace(lines[i])

		// Look for matching derive attribute.
		if !deriveMatcher.MatchString(line) {
			i++
			continue
		}

		// Collect serde container attributes between derive and struct head.
		renameAll := ""
		j := i + 1
		for j < len(lines) {
			trimmed := strings.TrimSpace(lines[j])
			if m := reSerdeCont.FindStringSubmatch(trimmed); m != nil {
				renameAll = parseSerdeContainerAttr(m[1])
				j++
				continue
			}
			if reStructHead.MatchString(trimmed) {
				break
			}
			if trimmed == "" || strings.HasPrefix(trimmed, "//") || strings.HasPrefix(trimmed, "#[") {
				j++
				continue
			}
			break
		}

		if j >= len(lines) {
			i = j
			continue
		}

		headLine := strings.TrimSpace(lines[j])
		m := reStructHead.FindStringSubmatch(headLine)
		if m == nil {
			i = j + 1
			continue
		}

		structName := m[1]
		j++ // move past the struct head line

		// Parse fields until closing brace.
		var fields []RustField
		var pendingSerdeAttrs []string

		for j < len(lines) {
			fline := strings.TrimSpace(lines[j])
			if fline == "}" {
				j++
				break
			}

			// Collect serde field attributes.
			if sm := reSerdeField.FindStringSubmatch(fline); sm != nil {
				pendingSerdeAttrs = append(pendingSerdeAttrs, sm[1])
				j++
				continue
			}

			// Skip comments and other attributes.
			if strings.HasPrefix(fline, "//") || strings.HasPrefix(fline, "#[") || fline == "" {
				j++
				continue
			}

			// Try to match a field.
			// A type split over lines (`HashMap<` … `>,`) is not one field line.
			if fm := reField.FindStringSubmatch(fline); fm != nil && strings.Count(fm[2], "<") == strings.Count(fm[2], ">") {
				field := RustField{
					Name:     fm[1],
					RustType: strings.TrimSpace(fm[2]),
				}
				// Apply pending serde attributes.
				for _, attr := range pendingSerdeAttrs {
					applySerdeFieldAttr(&field, attr)
				}
				if strings.HasPrefix(field.RustType, "Option<") {
					field.Optional = true
				}
				fields = append(fields, field)
				pendingSerdeAttrs = nil
			}
			j++
		}

		result = append(result, &RustStruct{
			Name:      structName,
			Fields:    fields,
			RenameAll: renameAll,
			Source:    path,
		})
		i = j
	}
	return result
}

func parseSerdeContainerAttr(attr string) string {
	// Extract rename_all = "camelCase"
	re := regexp.MustCompile(`rename_all\s*=\s*"([\w-]+)"`)
	if m := re.FindStringSubmatch(attr); m != nil {
		return m[1]
	}
	return ""
}

func applySerdeFieldAttr(f *RustField, attr string) {
	parts := strings.Split(attr, ",")
	for _, p := range parts {
		p = strings.TrimSpace(p)
		if strings.HasPrefix(p, "rename") && strings.Contains(p, "=") {
			re := regexp.MustCompile(`rename\s*=\s*"([^"]+)"`)
			if m := re.FindStringSubmatch(p); m != nil {
				f.Rename = m[1]
			}
		}
		if p == "skip_serializing" || p == "skip" {
			f.Skip = true
		}
		// The key is left out when the predicate holds: it may be absent.
		if strings.HasPrefix(p, "skip_serializing_if") {
			f.Optional = true
		}
		if p == "flatten" {
			f.Flatten = true
		}
		if p == "default" || strings.HasPrefix(p, "default") {
			f.Optional = true
		}
		if strings.HasPrefix(p, "serialize_with") {
			re := regexp.MustCompile(`serialize_with\s*=\s*"([^"]+)"`)
			if m := re.FindStringSubmatch(p); m != nil {
				f.SerializeWith = m[1]
			}
		}
	}
}

// ── Route Parsing ───────────────────────────────────────────────────────

// Route represents a parsed Axum route.
type Route struct {
	Method  string // GET, POST, PUT, DELETE, PATCH
	Path    string // /agents/{id}/chats
	Handler string // handlers::agents::list_agent_chats
	Source  string // routes/roles.rs
}

// scanRoutes reads all .rs files in the routes dir and extracts .route() calls.
func scanRoutes(dir string) []Route {
	entries, err := os.ReadDir(dir)
	if err != nil {
		fmt.Fprintf(os.Stderr, "warning: cannot read routes dir %s: %v\n", dir, err)
		return nil
	}
	var routes []Route
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".rs") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		routes = append(routes, parseRoutes(string(data), e.Name())...)
	}
	return routes
}

var reRoute = regexp.MustCompile(`\.route\(\s*"([^"]+)"\s*,\s*axum::routing::(get|post|put|delete|patch)\(([^)]+?)\)`)

// reChainedMethod matches chained method calls like .put(handler) after the initial method.
var reChainedMethod = regexp.MustCompile(`\.(get|post|put|delete|patch)\(([^)]+?)\)`)

func parseRoutes(src, sourceFile string) []Route {
	var routes []Route
	// Collapse newlines so multi-line .route() calls match the regex.
	collapsed := strings.ReplaceAll(src, "\n", " ")
	for _, idx := range reRoute.FindAllStringSubmatchIndex(collapsed, -1) {
		routePath := collapsed[idx[2]:idx[3]]
		method := strings.ToUpper(collapsed[idx[4]:idx[5]])
		handler := strings.TrimSpace(collapsed[idx[6]:idx[7]])

		routes = append(routes, Route{
			Method:  method,
			Path:    routePath,
			Handler: handler,
			Source:  sourceFile,
		})

		// Scan for chained .method(handler) calls on the same .route() call.
		// Start scanning from after the first method's closing paren.
		rest := collapsed[idx[1]:]
		for _, cm := range reChainedMethod.FindAllStringSubmatch(rest, -1) {
			chainedMethod := strings.ToUpper(cm[1])
			chainedHandler := strings.TrimSpace(cm[2])
			// Stop if we hit the next .route( call — we've left this route.
			chainedPos := strings.Index(rest, "."+cm[0][1:])
			nextRoute := strings.Index(rest, ".route(")
			if nextRoute >= 0 && chainedPos > nextRoute {
				break
			}
			routes = append(routes, Route{
				Method:  chainedMethod,
				Path:    routePath,
				Handler: chainedHandler,
				Source:  sourceFile,
			})
			// Advance rest past this match to find further chained methods.
			rest = rest[chainedPos+len(cm[0]):]
		}
	}
	return routes
}

// ── Handler Response Parsing ────────────────────────────────────────────

// HandlerInfo captures a handler function's name and its inferred response shape.
type HandlerInfo struct {
	// FuncName is the Rust function name (e.g. list_agents).
	FuncName string
	// QualifiedName includes the module path (e.g. handlers::agents::list_agents).
	QualifiedName string
	// ResponseKeys are the top-level keys from the json!({...}) response.
	// If the handler returns a struct directly, this is nil.
	ResponseKeys []ResponseKey
	// ResponseStruct is the name of a typed response struct, if any.
	ResponseStruct string
	// QueryParams are typed parameters extracted from Query<T> in the function signature.
	QueryParams []QueryParam
	Source      string
}

// QueryParam represents a typed query parameter for a handler function.
type QueryParam struct {
	Name     string // TypeScript name (camelCase)
	TSType   string // TypeScript type (string, number, etc.)
	Optional bool   // whether the param has a default or is Option<T>
}

// ResponseKey is a single key in a json!({...}) response.
type ResponseKey struct {
	Key       string
	ValueExpr string // raw Rust expression
	InferredTS string // best-guess TS type
}

// scanStoreMethodTypes reads all query .rs files and extracts method return types.
// Returns a map from method_name → Rust return type (unwrapped from Result<T, E>).
func scanStoreMethodTypes(dir string) map[string]string {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	result := make(map[string]string)
	// (?s) makes . match newlines (multi-line function signatures). The
	// parameter list excludes braces so a method that does not return Result
	// (`pub fn cron_ref(..) -> String {`) cannot swallow the next method's
	// signature and take its return type.
	reMethod := regexp.MustCompile(`(?s)pub\s+fn\s+(\w+)\s*\([^{}]*?\)\s*->\s*Result<(.+?)(?:\s*\{)`)
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".rs") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		for _, m := range reMethod.FindAllStringSubmatch(string(data), -1) {
			methodName := m[1]
			// Extract first generic arg from Result<T, E> respecting nested brackets.
			returnType := extractFirstGenericArg(m[2])
			if returnType != "" {
				result[methodName] = returnType
			}
		}
	}
	return result
}

// extractFirstGenericArg extracts the first type argument from "T, E>" or "T>"
// while respecting nested angle brackets (e.g. "Vec<Agent>, NeboError>" → "Vec<Agent>").
func extractFirstGenericArg(s string) string {
	depth := 0
	for i, ch := range s {
		switch ch {
		case '<':
			depth++
		case '>':
			if depth == 0 {
				// Top-level > — end of Result<T>
				return strings.TrimSpace(s[:i])
			}
			depth--
		case ',':
			if depth == 0 {
				// Top-level , — end of first arg in Result<T, E>
				return strings.TrimSpace(s[:i])
			}
		}
	}
	return strings.TrimSpace(s)
}

// scanDeserializeStructs reads handler .rs files and extracts Deserialize structs
// (used for Query<T> parameter inference — not emitted as TS interfaces).
// Keys are "filename::StructName" to avoid collisions between identically-named
// structs in different handler files (e.g. multiple ListQuery structs).
func scanDeserializeStructs(dir string) map[string]*RustStruct {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	result := make(map[string]*RustStruct)
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".rs") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		for _, s := range parseDeserializeStructs(string(data), path) {
			// Key by filename::Name so same-named structs don't collide.
			result[e.Name()+"::"+s.Name] = s
		}
	}
	return result
}

// scanHandlers reads all handler .rs files and extracts response shapes.
func scanHandlers(dir string, structs map[string]*RustStruct, storeMethodTypes map[string]string, queryStructs map[string]*RustStruct) map[string]*HandlerInfo {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	result := make(map[string]*HandlerInfo)
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".rs") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		module := strings.TrimSuffix(e.Name(), ".rs")
		for _, h := range parseHandlers(string(data), module, e.Name(), structs, storeMethodTypes, queryStructs) {
			result[h.QualifiedName] = h
		}
	}
	return result
}

var reFuncSig = regexp.MustCompile(`pub\s+async\s+fn\s+(\w+)\s*\(`)

// reAnyFuncSig is any function, a private helper included.
var reAnyFuncSig = regexp.MustCompile(`^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)\s*[<(]`)

// reHandlerReturn captures a typed response struct from `-> HandlerResult<StructName>`
// or `-> Result<Json<StructName>, E>` (a handler with its own error type),
// the struct path-qualified or not (`sheet::Edited`).
var reHandlerReturn = regexp.MustCompile(`->\s*(?:HandlerResult<|Result<\s*(?:axum::)?(?:response::)?Json<)\s*((?:\w+::)*[A-Za-z_][A-Za-z0-9_]*)\s*>`)

// reReturnedVar is the handler's final `Ok(Json(body))`.
var reReturnedVar = regexp.MustCompile(`Ok\(\s*(?:axum::)?(?:response::)?Json\(\s*(\w+)\s*\)\s*\)`)

// reCall is a call of a plain function: `roster(`.
var reCall = regexp.MustCompile(`(?:^|[^\w.:])(\w+)\s*\(`)

func parseHandlers(src, module, sourceFile string, structs map[string]*RustStruct, storeMethodTypes map[string]string, queryStructs map[string]*RustStruct) []*HandlerInfo {
	var result []*HandlerInfo

	// Split into functions by finding pub async fn signatures.
	funcBodies := splitFunctions(src)
	// Every function in the file, its private helpers included: a handler
	// may return the body a helper builds.
	helpers := splitFunctionsMatching(src, reAnyFuncSig)

	for funcName, body := range funcBodies {
		h := &HandlerInfo{
			FuncName:      funcName,
			QualifiedName: "handlers::" + module + "::" + funcName,
			Source:        sourceFile,
		}

		// Extract query parameters from Query<T> in the function signature.
		h.QueryParams = extractQueryParams(body, sourceFile, queryStructs, structs)

		// Build a variable type map from let bindings + store method calls.
		varTypes := extractVarTypes(body)
		traceStoreMethodCalls(body, varTypes, storeMethodTypes)
		traceHelperCalls(body, varTypes, helpers)

		// Find the last Ok(Json(serde_json::json!({...}))) pattern.
		keys := extractJsonResponse(body)
		if keys == nil {
			// `Ok(Json(body))` where `let body = roster(..)`: the helper
			// builds the response (GET /agents, built off the workers).
			if hb := helperResponseBody(body, funcName, helpers); hb != "" {
				keys = extractJsonResponse(hb)
				varTypes = extractVarTypes(hb)
				traceStoreMethodCalls(hb, varTypes, storeMethodTypes)
				traceHelperCalls(hb, varTypes, helpers)
			}
		}
		if keys != nil {
			// Refine inferred types using variable type info and key-name heuristics.
			for i := range keys {
				keys[i].InferredTS = refineType(keys[i], varTypes, structs, handlerToFuncName(h.QualifiedName), h.FuncName)
			}
			h.ResponseKeys = keys
		} else if m := reHandlerReturn.FindStringSubmatch(body); m != nil {
			// No json! response — wire the typed return struct `-> HandlerResult<StructName>`
			// when StructName is a known Serialize struct.
			name := m[1]
			if i := strings.LastIndex(name, "::"); i >= 0 {
				name = name[i+2:]
			}
			if _, ok := structs[name]; ok && m[1] != "serde_json::Value" {
				h.ResponseStruct = name
			}
		}

		result = append(result, h)
	}
	return result
}

// extractQueryParams finds Query<StructName> in a handler function signature
// and converts the struct's fields to typed query parameters.
// sourceFile is the handler's filename (e.g. "memory.rs") — used to prefer
// the file-local Deserialize struct when multiple files define the same name.
func extractQueryParams(body string, sourceFile string, queryStructs map[string]*RustStruct, allStructs map[string]*RustStruct) []QueryParam {
	// Only look in the function signature (before the first opening brace).
	sigEnd := strings.Index(body, "{")
	if sigEnd < 0 {
		return nil
	}
	sig := body[:sigEnd]

	m := reQueryType.FindStringSubmatch(sig)
	if m == nil {
		return nil
	}

	queryTypeName := m[1]

	// Look up in Deserialize structs: prefer file-local key (filename::Name),
	// then fall back to allStructs.
	var s *RustStruct
	if queryStructs != nil {
		s = queryStructs[sourceFile+"::"+queryTypeName]
	}
	if s == nil {
		s = allStructs[queryTypeName]
	}
	if s == nil {
		return nil
	}

	var params []QueryParam
	for _, f := range s.Fields {
		if f.Skip {
			continue
		}
		params = append(params, QueryParam{
			Name:     toTSFieldName(f, s.RenameAll),
			TSType:   rustTypeToTS(f.RustType, allStructs),
			Optional: f.Optional,
		})
	}
	return params
}

// extractVarTypes finds `let varname: Type = ...` patterns in a function body.
func extractVarTypes(body string) map[string]string {
	result := make(map[string]string)
	re := regexp.MustCompile(`let\s+(?:mut\s+)?(\w+)\s*:\s*([^=;]+?)\s*=`)
	for _, m := range re.FindAllStringSubmatch(body, -1) {
		varName := m[1]
		varType := strings.TrimSpace(m[2])
		result[varName] = varType
	}
	return result
}

// traceStoreMethodCalls finds `let var = state.store.method(...)...` patterns
// and resolves the variable type from the store method's return type.
func traceStoreMethodCalls(body string, varTypes map[string]string, storeMethodTypes map[string]string) {
	// Strategy 1: Direct assignment — let var = state.store.method_name(...)
	re := regexp.MustCompile(`let\s+(?:mut\s+)?(\w+)\s*=\s*state\.store\.(\w+)\s*\(`)
	for _, mi := range re.FindAllStringSubmatchIndex(body, -1) {
		varName := body[mi[2]:mi[3]]
		methodName := body[mi[4]:mi[5]]
		if _, already := varTypes[varName]; already {
			continue // explicit annotation takes priority
		}
		// Check that the store result isn't transformed (.len(), .map(), etc.)
		stmtEnd := findStatementEnd(body, mi[1])
		if stmtEnd > mi[1] && storeResultTransformed(body[mi[1]:stmtEnd]) {
			continue
		}
		if retType, ok := storeMethodTypes[methodName]; ok {
			varTypes[varName] = retType
		}
	}

	// Strategy 2: Conditional assignment — let var = if ... { state.store.method(...) } else { ... }
	// Also catches: let var = state\n  .store\n  .method(...)
	reLetBlock := regexp.MustCompile(`let\s+(?:mut\s+)?(\w+)\s*=`)
	for _, m := range reLetBlock.FindAllStringSubmatchIndex(body, -1) {
		varName := body[m[2]:m[3]]
		if _, already := varTypes[varName]; already {
			continue
		}
		// Find the statement boundary (semicolon at depth 0).
		start := m[1]
		end := findStatementEnd(body, start)
		if end <= start {
			continue
		}
		chunk := body[start:end]

		// A match's value is its arms', not its scrutinee's: `let key =
		// match state.store.get_chat(..) { Ok(Some(_)) => .. }` is no Chat.
		if strings.HasPrefix(strings.TrimSpace(chunk), "match ") {
			continue
		}

		// Skip if the expression transforms the result (.len(), .map(), .count(), etc.)
		if storeResultTransformed(chunk) {
			continue
		}

		// Find first state.store.method() or state\n.store\n.method()
		reStore := regexp.MustCompile(`state\s*\.?\s*store\s*\.\s*(\w+)\s*\(`)
		sm := reStore.FindStringSubmatch(chunk)
		if sm != nil {
			methodName := sm[1]
			if retType, ok := storeMethodTypes[methodName]; ok {
				varTypes[varName] = retType
			}
		}
	}

	// Strategy 3: Multiline method chain — state\n    .store\n    .method()
	reMultiLine := regexp.MustCompile(`let\s+(?:mut\s+)?(\w+)\s*=\s*state\s*\n\s*\.store\s*\n?\s*\.(\w+)\s*\(`)
	for _, m := range reMultiLine.FindAllStringSubmatch(body, -1) {
		varName := m[1]
		methodName := m[2]
		if _, already := varTypes[varName]; already {
			continue
		}
		if retType, ok := storeMethodTypes[methodName]; ok {
			varTypes[varName] = retType
		}
	}
}

// reLetCall is `let var = name(` — a call of a plain function — or
// `let var = format!(`.
var reLetCall = regexp.MustCompile(`let\s+(?:mut\s+)?(\w+)\s*=\s*(\w+!?)\s*\(`)

// reHelperReturn is a function signature's return type, up to its body.
var reHelperReturn = regexp.MustCompile(`(?s)^[^{]*?\)\s*->\s*(.+?)\s*(?:where\b|\{)`)

// traceHelperCalls types `let var = helper(..)` from the same-file helper's
// return type (`Result<T, E>` is T: the call ends in `?`), and `let var =
// format!(..)` as a String.
func traceHelperCalls(body string, varTypes map[string]string, helpers map[string]string) {
	for _, mi := range reLetCall.FindAllStringSubmatchIndex(body, -1) {
		varName := body[mi[2]:mi[3]]
		callee := body[mi[4]:mi[5]]
		if _, already := varTypes[varName]; already {
			continue
		}
		stmtEnd := findStatementEnd(body, mi[1])
		if stmtEnd > mi[1] && storeResultTransformed(body[mi[1]:stmtEnd]) {
			continue
		}
		if callee == "format!" {
			varTypes[varName] = "String"
			continue
		}
		hb, ok := helpers[callee]
		if !ok {
			continue
		}
		m := reHelperReturn.FindStringSubmatch(hb)
		if m == nil {
			continue
		}
		ret := strings.TrimSpace(m[1])
		if strings.HasPrefix(ret, "Result<") && strings.HasSuffix(ret, ">") {
			ret = extractFirstGenericArg(ret[len("Result<"):])
		}
		if i := strings.LastIndex(ret, "::"); i >= 0 && !strings.Contains(ret, "<") {
			ret = ret[i+2:] // types::packs::Pack → Pack
		}
		varTypes[varName] = ret
	}
}

// findStatementEnd finds the end of a Rust statement (semicolon at brace depth 0)
// starting from position start. Returns the position of the semicolon, or start+500.
func findStatementEnd(body string, start int) int {
	limit := start + 500
	if limit > len(body) {
		limit = len(body)
	}
	depth := 0
	for i := start; i < limit; i++ {
		switch body[i] {
		case '{', '(':
			depth++
		case '}', ')':
			depth--
		case ';':
			if depth <= 0 {
				return i
			}
		}
	}
	return limit
}

// storeResultTransformed returns true if the chunk between a store method call
// and the semicolon contains type-transforming operations like .len(), .map().
func storeResultTransformed(chunk string) bool {
	transformers := []string{".len()", ".map(", ".count()", ".filter(", ".into_iter()",
		".collect()", ".fold(", ".any(", ".all(", ".find(", ".position("}
	for _, t := range transformers {
		if strings.Contains(chunk, t) {
			return true
		}
	}
	return false
}

// refineType improves a ResponseKey's TS type using variable types, struct map, and key-name heuristics.
// handlerName is the camelCase TS function name (e.g. "listAgents").
// rustName is the raw Rust function name (e.g. "list_agents") for override lookups.
func refineType(k ResponseKey, varTypes map[string]string, structs map[string]*RustStruct, handlerName, rustName string) string {
	expr := strings.TrimSpace(k.ValueExpr)

	// 0. Check explicit type overrides first (highest priority).
	// Overrides can use either the snake_case Rust name or the camelCase TS name.
	for _, name := range []string{rustName, handlerName} {
		overrideKey := name + "." + k.Key
		if tsType, ok := typeOverrides[overrideKey]; ok {
			return tsType
		}
	}

	// 1. If value is a known variable with a type annotation, use it.
	if varType, ok := varTypes[expr]; ok {
		return rustVarTypeToTSWithStructs(varType, structs)
	}

	// 2. High-confidence expression patterns (literals, .len(), etc.)
	//    These beat key-name heuristics because they're unambiguous.
	if hc := inferHighConfidence(expr); hc != "" {
		return hc
	}

	// 3. Key-name based inference.
	if tsType := inferFromKeyName(k.Key); tsType != "" {
		return tsType
	}

	// 4. Fall back to expression-based inference (variable name patterns).
	return inferTSType(expr)
}

// inferHighConfidence returns a type for unambiguous expression patterns.
func inferHighConfidence(expr string) string {
	// String literals or format! macros.
	if strings.HasPrefix(expr, "\"") || strings.HasPrefix(expr, "format!(") {
		return "string"
	}
	// Boolean literals.
	if expr == "true" || expr == "false" {
		return "boolean"
	}
	// Numeric literals.
	if isNumericExpr(expr) {
		return "number"
	}
	// .len() calls → always number.
	if strings.HasSuffix(expr, ".len()") || strings.Contains(expr, ".len()") {
		return "number"
	}
	// .is_empty(), .is_some(), etc. → always boolean.
	if strings.HasSuffix(expr, ".is_empty()") || strings.HasSuffix(expr, ".is_some()") ||
		strings.HasSuffix(expr, ".is_none()") {
		return "boolean"
	}
	// Ternary-like: if expr { "a" } else { "b" } → look at the first literal
	if strings.HasPrefix(expr, "if ") && strings.Contains(expr, "\"") {
		return "string"
	}
	return ""
}

// rustVarTypeToTS converts a Rust variable type annotation to TS (no struct lookup).
func rustVarTypeToTS(rustType string) string {
	return rustVarTypeToTSWithStructs(rustType, nil)
}

// rustVarTypeToTSWithStructs converts a Rust variable type annotation to TS,
// using the struct map to verify struct names exist as interfaces.
func rustVarTypeToTSWithStructs(rustType string, structs map[string]*RustStruct) string {
	rustType = strings.TrimSpace(rustType)

	// Option<T> → unwrap
	if strings.HasPrefix(rustType, "Option<") && strings.HasSuffix(rustType, ">") {
		inner := rustType[7 : len(rustType)-1]
		return rustVarTypeToTSWithStructs(inner, structs)
	}

	// Vec<serde_json::Value> → unknown[]
	if strings.HasPrefix(rustType, "Vec<") && strings.HasSuffix(rustType, ">") {
		inner := strings.TrimSpace(rustType[4 : len(rustType)-1])
		if inner == "serde_json::Value" || inner == "Value" {
			return "unknown[]"
		}
		if inner == "String" || inner == "&str" {
			return "string[]"
		}
		// Vec<(String, String)> → [string, string][]
		if strings.HasPrefix(inner, "(") {
			return "unknown[]"
		}
		// Vec<StructName> → StructName[] (with struct verification)
		parts := strings.Split(inner, "::")
		name := parts[len(parts)-1]
		if structs != nil {
			if _, ok := structs[name]; ok {
				return name + "[]"
			}
		}
		return name + "[]"
	}

	// HashMap/BTreeMap
	for _, prefix := range []string{"HashMap<", "BTreeMap<"} {
		if strings.HasPrefix(rustType, prefix) && strings.HasSuffix(rustType, ">") {
			inner := rustType[len(prefix) : len(rustType)-1]
			parts := splitGenericArgs(inner)
			if len(parts) == 2 {
				k := rustVarTypeToTSWithStructs(parts[0], structs)
				v := rustVarTypeToTSWithStructs(parts[1], structs)
				return "Record<" + k + ", " + v + ">"
			}
		}
	}

	if rustType == "serde_json::Value" || rustType == "Value" {
		return "unknown"
	}
	if rustType == "String" || rustType == "&str" || rustType == "Cow<'_, str>" {
		return "string"
	}
	if rustType == "bool" {
		return "boolean"
	}
	if rustType == "i8" || rustType == "i16" || rustType == "i32" || rustType == "i64" ||
		rustType == "u8" || rustType == "u16" || rustType == "u32" || rustType == "u64" ||
		rustType == "usize" || rustType == "isize" || rustType == "f32" || rustType == "f64" {
		return "number"
	}

	// Check if it's a known struct
	if structs != nil {
		if _, ok := structs[rustType]; ok {
			return rustType
		}
	}

	return "unknown"
}

// Known array key names (response keys that are always arrays).
var arrayKeys = map[string]bool{
	"agents": true, "chats": true, "sessions": true, "messages": true,
	"runs": true, "tasks": true, "memories": true, "plugins": true,
	"skills": true, "advisors": true, "profiles": true, "items": true,
	"errors": true, "entries": true, "connections": true, "results": true,
	"extensions": true, "sources": true, "workflows": true, "notifications": true,
	"lanes": true, "history": true, "aliases": true, "models": true,
	"events": true, "members": true, "tools": true, "fields": true,
	"filesystemAgents": true, "recentErrors": true, "changes": true,
	"bindings": true, "categories": true, "logs": true, "images": true,
}

// Known string key names.
var stringKeys = map[string]bool{
	"message": true, "error": true, "status": true, "name": true,
	"description": true, "agentId": true, "chatId": true, "sessionId": true,
	"id": true, "version": true, "title": true, "token": true,
	"refreshToken": true, "email": true, "slug": true, "bindingName": true,
	"plan": true, "type": true, "newChatId": true, "sessionKey": true,
	"installPath": true, "localVersion": true, "remoteVersion": true,
	"source": true, "activeChatId": true, "key": true, "value": true,
	"url": true, "path": true, "ownerId": true, "profileId": true,
	"displayName": true, "reason": true, "checksum": true, "platform": true,
}

// Known number key names.
var numberKeys = map[string]bool{
	"total": true, "count": true, "offset": true, "limit": true,
	"expiresAt": true, "totalMessages": true,
	"compactionCount": true, "removedCount": true, "sessionCount": true,
	"createdAt": true, "updatedAt": true, "port": true, "pid": true,
	"uptime": true, "size": true, "activeTasks": true, "queuedTasks": true,
}

// Known boolean key names.
var booleanKeys = map[string]bool{
	"success": true, "ok": true, "connected": true, "authenticated": true,
	"hasUpdate": true, "installed": true, "activated": true, "isActive": true,
	"hasMore": true, "started": true, "reconnected": true, "janusProvider": true,
}

// Known object key names (single object, not array).
var objectKeys = map[string]bool{
	"agent": true, "chat": true, "session": true, "profile": true,
	"task": true, "memory": true, "plugin": true, "config": true,
	"settings": true, "stats": true, "workflow": true, "trigger": true,
	"installReport": true, "providers": true, "taskRouting": true,
	"laneRouting": true, "quota": true, "weekly": true,
}

func inferFromKeyName(key string) string {
	if arrayKeys[key] {
		return "unknown[]"
	}
	if stringKeys[key] {
		return "string"
	}
	if numberKeys[key] {
		return "number"
	}
	if booleanKeys[key] {
		return "boolean"
	}
	if objectKeys[key] {
		return "unknown"
	}
	return "" // no heuristic match
}

// helperResponseBody is the body of the same-file helper a handler returns
// the value of (`let body = ... roster(..) ...; Ok(Json(body))`), when the
// handler builds no json! of its own. Empty when there is none.
func helperResponseBody(body, self string, helpers map[string]string) string {
	ms := reReturnedVar.FindAllStringSubmatch(body, -1)
	if len(ms) == 0 {
		return ""
	}
	v := ms[len(ms)-1][1]
	let := regexp.MustCompile(`let\s+(?:mut\s+)?` + regexp.QuoteMeta(v) + `\s*=`).FindStringIndex(body)
	if let == nil {
		return ""
	}
	stmt := body[let[1]:findStatementEnd(body, let[1])]
	for _, c := range reCall.FindAllStringSubmatch(stmt, -1) {
		if c[1] == self {
			continue
		}
		if hb, ok := helpers[c[1]]; ok && strings.Contains(hb, "json!({") {
			return hb
		}
	}
	return ""
}

// splitFunctions splits Rust source into named function bodies.
func splitFunctions(src string) map[string]string {
	return splitFunctionsMatching(src, reFuncSig)
}

// splitFunctionsMatching splits Rust source into the bodies of the
// functions whose signature line sig matches. The first of a name wins (a
// test module's same-named function comes later in the file).
func splitFunctionsMatching(src string, sig *regexp.Regexp) map[string]string {
	result := make(map[string]string)
	lines := strings.Split(src, "\n")

	i := 0
	for i < len(lines) {
		m := sig.FindStringSubmatch(lines[i])
		if m == nil {
			i++
			continue
		}
		funcName := m[1]
		// Find the function body by tracking braces.
		depth := 0
		started := false
		closed := false
		var bodyLines []string
		for j := i; j < len(lines); j++ {
			line := lines[j]
			bodyLines = append(bodyLines, line)
			depth += strings.Count(line, "{") - strings.Count(line, "}")
			if depth > 0 {
				started = true
			}
			if started && depth <= 0 {
				if _, dup := result[funcName]; !dup {
					result[funcName] = strings.Join(bodyLines, "\n")
				}
				i = j + 1
				closed = true
				break
			}
		}
		// No body (a trait method's `;`), or braces that never balance.
		if !closed {
			i++
		}
	}
	return result
}

// extractJsonResponse finds the last serde_json::json!({...}) in a function body
// and extracts the top-level key-value pairs.
func extractJsonResponse(body string) []ResponseKey {
	// The response is the LAST TOP-LEVEL serde_json::json!({...}) in the body —
	// the last return, whichever branch wrote it.
	//
	// "Top-level" is the part that matters and the part a plain LastIndex got
	// wrong: a handler that builds one of its own fields with a fallback
	// (`.unwrap_or_else(|| serde_json::json!({}))`) ends on that empty NESTED
	// literal, so the response read as "no keys" and the handler's whole
	// interface vanished from the generated client — every caller of it typed
	// `{}`. So collect every json!({ span and keep the last one that no other
	// span encloses.
	type jsonSpan struct{ start, end, content int }
	var spans []jsonSpan
	for i := 0; ; {
		j := strings.Index(body[i:], "json!({")
		if j < 0 {
			break
		}
		pos := i + j
		contentAt := pos + len("json!(")
		i = pos + len("json!({")
		inner := extractBraced(body[contentAt:])
		if inner == "" {
			continue
		}
		spans = append(spans, jsonSpan{pos, contentAt + len(inner) + 2, contentAt})
	}
	content := ""
	for k := range spans {
		// An error body (`(StatusCode::BAD_REQUEST, Json(json!({..})))`)
		// is not the response the client reads.
		if errorStatusBefore(body[:spans[k].start]) {
			continue
		}
		enclosed := false
		for m := range spans {
			if m != k && spans[m].start < spans[k].start && spans[k].end <= spans[m].end {
				enclosed = true
				break
			}
		}
		if !enclosed {
			content = extractBraced(body[spans[k].content:])
		}
	}
	if content == "" {
		// Also try json!([...]) for array responses.
		return nil
	}

	// Parse top-level "key": value pairs from the json!({...}) content.
	return parseJsonMacroKeys(content)
}

// reErrorStatus is a non-2xx status paired with the Json body that follows:
// `StatusCode::BAD_REQUEST, axum::Json(serde_json::`.
var reErrorStatus = regexp.MustCompile(`StatusCode::(\w+)\s*,\s*(?:axum::)?(?:response::)?Json\(\s*(?:serde_json::)?$`)

// errorStatusBefore reports whether the json! literal right after before is
// the body of an error status.
func errorStatusBefore(before string) bool {
	if len(before) > 200 {
		before = before[len(before)-200:]
	}
	m := reErrorStatus.FindStringSubmatch(before)
	if m == nil {
		return false
	}
	switch m[1] {
	case "OK", "CREATED", "ACCEPTED", "NO_CONTENT":
		return false
	}
	return true
}

// extractBraced extracts a balanced {...} block from the start of s.
func extractBraced(s string) string {
	if len(s) == 0 || s[0] != '{' {
		return ""
	}
	depth := 0
	for i, ch := range s {
		switch ch {
		case '{':
			depth++
		case '}':
			depth--
			if depth == 0 {
				return s[:i+1]
			}
		}
	}
	return ""
}

// splitJsonPairsToLines splits comma-separated top-level key-value pairs
// onto separate lines so the per-line regex in parseJsonMacroKeys works.
// e.g. `"runs": runs, "total": total` → `"runs": runs,\n"total": total`
func splitJsonPairsToLines(s string) string {
	var result strings.Builder
	depth := 0
	for i := 0; i < len(s); i++ {
		ch := s[i]
		switch ch {
		case '{', '[':
			depth++
		case '}', ']':
			depth--
		}
		result.WriteByte(ch)
		// At top level, after a comma followed by optional whitespace and a quote,
		// insert a newline so each pair gets its own line.
		if ch == ',' && depth == 0 && i+1 < len(s) {
			// Look ahead past whitespace for a quote.
			j := i + 1
			for j < len(s) && (s[j] == ' ' || s[j] == '\t') {
				j++
			}
			if j < len(s) && s[j] == '"' {
				result.WriteByte('\n')
				i = j - 1 // skip whitespace, loop will advance to j
			}
		}
	}
	return result.String()
}

// parseJsonMacroKeys extracts "key": expr pairs from a json!({...}) body.
// Only extracts top-level keys — skips content nested inside [...] or {...}.
func parseJsonMacroKeys(content string) []ResponseKey {
	// Remove outer braces.
	inner := strings.TrimSpace(content)
	if len(inner) < 2 {
		return nil
	}
	inner = inner[1 : len(inner)-1]

	var keys []ResponseKey

	// Normalize single-line json! into multi-line so the per-line regex works.
	// Split top-level comma-separated pairs onto separate lines.
	inner = splitJsonPairsToLines(inner)

	// Track nesting depth so we only match top-level "key": value pairs.
	depth := 0
	reKV := regexp.MustCompile(`"(\w+)"\s*:\s*(.+?)(?:\s*,\s*$|\s*$)`)
	for _, line := range strings.Split(inner, "\n") {
		trimmed := strings.TrimSpace(line)
		if trimmed == "" || strings.HasPrefix(trimmed, "//") {
			continue
		}
		// Update depth BEFORE matching — count openers and closers on this line.
		// Only match keys at depth 0 (top-level of the json! object).
		prevDepth := depth
		for _, ch := range trimmed {
			switch ch {
			case '{', '[':
				depth++
			case '}', ']':
				depth--
			}
		}
		if prevDepth == 0 {
			if m := reKV.FindStringSubmatch(trimmed); m != nil {
				key := m[1]
				expr := strings.TrimRight(strings.TrimSpace(m[2]), ",")
				keys = append(keys, ResponseKey{
					Key:        key,
					ValueExpr:  expr,
					InferredTS: inferTSType(expr),
				})
			}
		}
	}
	return keys
}

// inferTSType guesses a TypeScript type from a Rust expression.
func inferTSType(expr string) string {
	expr = strings.TrimSpace(expr)

	// String literals.
	if strings.HasPrefix(expr, "\"") || strings.HasPrefix(expr, "format!(") {
		return "string"
	}
	// Boolean literals.
	if expr == "true" || expr == "false" {
		return "boolean"
	}
	// Numeric expressions.
	if isNumericExpr(expr) {
		return "number"
	}
	// .len() calls.
	if strings.HasSuffix(expr, ".len()") || strings.Contains(expr, ".len()") {
		return "number"
	}
	// .is_empty(), .is_some(), etc.
	if strings.HasSuffix(expr, ".is_empty()") || strings.HasSuffix(expr, ".is_some()") ||
		strings.HasSuffix(expr, ".is_none()") {
		return "boolean"
	}
	// Known variable patterns — arrays/vecs.
	if strings.HasSuffix(expr, "agents") || strings.HasSuffix(expr, "sessions") ||
		strings.HasSuffix(expr, "messages") || strings.HasSuffix(expr, "chats") ||
		strings.HasSuffix(expr, "runs") || strings.HasSuffix(expr, "tasks") ||
		strings.HasSuffix(expr, "memories") || strings.HasSuffix(expr, "plugins") ||
		strings.HasSuffix(expr, "skills") || strings.HasSuffix(expr, "advisors") ||
		strings.HasSuffix(expr, "profiles") || strings.HasSuffix(expr, "items") ||
		strings.HasSuffix(expr, "errors") || strings.HasSuffix(expr, "entries") ||
		strings.HasSuffix(expr, "connections") || strings.HasSuffix(expr, "results") ||
		strings.HasSuffix(expr, "extensions") || strings.HasSuffix(expr, "sources") ||
		strings.HasSuffix(expr, "workflows") || strings.HasSuffix(expr, "notifications") ||
		strings.HasSuffix(expr, "lanes") || strings.HasSuffix(expr, "history") ||
		strings.HasSuffix(expr, "aliases") || strings.HasSuffix(expr, "models") {
		return "unknown[]"
	}
	// Nested json!({...}).
	if strings.HasPrefix(expr, "serde_json::json!") || strings.HasPrefix(expr, "json!") {
		return "unknown"
	}
	// Field access on a variable (e.g. agent.name).
	if strings.Contains(expr, ".") && !strings.Contains(expr, "(") {
		return "string" // conservative default for field access
	}
	// Variable name ending in common suffixes.
	if strings.HasSuffix(expr, "_id") || strings.HasSuffix(expr, "Id") ||
		strings.HasSuffix(expr, "name") || strings.HasSuffix(expr, "description") ||
		strings.HasSuffix(expr, "title") || strings.HasSuffix(expr, "message") ||
		strings.HasSuffix(expr, "status") || strings.HasSuffix(expr, "version") ||
		strings.HasSuffix(expr, "path") || strings.HasSuffix(expr, "key") ||
		strings.HasSuffix(expr, "slug") || strings.HasSuffix(expr, "url") ||
		strings.HasSuffix(expr, "token") || strings.HasSuffix(expr, "email") {
		return "string"
	}
	if strings.HasSuffix(expr, "count") || strings.HasSuffix(expr, "total") ||
		strings.HasSuffix(expr, "offset") || strings.HasSuffix(expr, "limit") {
		return "number"
	}
	if strings.HasSuffix(expr, "success") || strings.HasSuffix(expr, "enabled") ||
		strings.HasSuffix(expr, "active") || strings.HasSuffix(expr, "connected") ||
		strings.HasSuffix(expr, "authenticated") || strings.HasSuffix(expr, "installed") {
		return "boolean"
	}

	return "unknown"
}

func isNumericExpr(s string) bool {
	s = strings.TrimSpace(s)
	if s == "" {
		return false
	}
	for _, ch := range s {
		if ch >= '0' && ch <= '9' {
			continue
		}
		return false
	}
	return true
}

// ── WebSocket Event Parsing ─────────────────────────────────────────────

// WSEvent represents a WebSocket event type with its payload shape.
type WSEvent struct {
	EventType string
	Keys      []ResponseKey
	Direction string // "server" (broadcast) or "client" (incoming)
}

func scanWSEvents(wsFile string) []WSEvent {
	data, err := os.ReadFile(wsFile)
	if err != nil {
		return nil
	}
	return parseWSEvents(string(data))
}

var reBroadcast = regexp.MustCompile(`(?:hub\.broadcast|broadcast)\(\s*"(\w+)"\s*,`)
var reWSType = regexp.MustCompile(`"type"\s*:\s*"(\w+)"`)

func parseWSEvents(src string) []WSEvent {
	seen := make(map[string]bool)
	var events []WSEvent

	// Find broadcast("event_type", json!({...})) calls.
	for _, m := range reBroadcast.FindAllStringSubmatchIndex(src, -1) {
		eventType := src[m[2]:m[3]]
		if seen[eventType] {
			continue
		}
		seen[eventType] = true

		// Try to find json!({...}) after this match.
		rest := src[m[1]:]
		idx := strings.Index(rest, "json!({")
		if idx >= 0 && idx < 200 { // within reasonable distance
			content := extractBraced(rest[idx+len("json!("):])
			if content != "" {
				keys := parseJsonMacroKeys(content)
				events = append(events, WSEvent{
					EventType: eventType,
					Keys:      keys,
					Direction: "server",
				})
				continue
			}
		}
		events = append(events, WSEvent{
			EventType: eventType,
			Direction: "server",
		})
	}

	// Find client message types by looking for match arms on message type.
	reClientMsg := regexp.MustCompile(`"(chat|cancel|auth|connect|ping|pong|session_reset|session_compact|workspace_action|a2ui_event)"`)
	for _, m := range reClientMsg.FindAllStringSubmatch(src, -1) {
		msgType := m[1]
		if !seen["client:"+msgType] {
			seen["client:"+msgType] = true
			events = append(events, WSEvent{
				EventType: msgType,
				Direction: "client",
			})
		}
	}

	return events
}

// ── Rust Enum Parsing ───────────────────────────────────────────────────

// RustEnum is a Serialize enum whose variants are all units: serde writes
// each as its (renamed) name, so in TypeScript it is a union of strings.
type RustEnum struct {
	Name   string
	Values []string
	Source string
}

var reEnumHead = regexp.MustCompile(`^pub\s+enum\s+(\w+)\s*\{`)

// reUnitVariant is a variant with no data: `Low,` (or the last, `Max`).
var reUnitVariant = regexp.MustCompile(`^(\w+)\s*,?$`)

// scanEnums reads all .rs files in dir (non-recursive) and extracts the
// unit-only enums that derive Serialize.
func scanEnums(dir string) []*RustEnum {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	var result []*RustEnum
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".rs") {
			continue
		}
		path := filepath.Join(dir, e.Name())
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		result = append(result, parseEnums(string(data), path)...)
	}
	return result
}

func parseEnums(src, path string) []*RustEnum {
	lines := strings.Split(src, "\n")
	var result []*RustEnum
	for i := 0; i < len(lines); i++ {
		if !reDerive.MatchString(strings.TrimSpace(lines[i])) {
			continue
		}
		// Container attributes up to the head. A tagged or untagged enum is
		// not a plain string on the wire.
		renameAll := ""
		plain := true
		j := i + 1
		for ; j < len(lines); j++ {
			t := strings.TrimSpace(lines[j])
			if m := reSerdeCont.FindStringSubmatch(t); m != nil {
				if strings.Contains(m[1], "tag") {
					plain = false
				}
				if r := parseSerdeContainerAttr(m[1]); r != "" {
					renameAll = r
				}
				continue
			}
			if t == "" || strings.HasPrefix(t, "//") || strings.HasPrefix(t, "#[") {
				continue
			}
			break
		}
		if j >= len(lines) {
			break
		}
		m := reEnumHead.FindStringSubmatch(strings.TrimSpace(lines[j]))
		if m == nil {
			continue
		}
		var values []string
		rename := ""
		unit := true
		for j++; j < len(lines); j++ {
			t := strings.TrimSpace(lines[j])
			if t == "}" {
				break
			}
			if sm := reSerdeField.FindStringSubmatch(t); sm != nil {
				if rm := regexp.MustCompile(`rename\s*=\s*"([^"]+)"`).FindStringSubmatch(sm[1]); rm != nil {
					rename = rm[1]
				}
				if strings.Contains(sm[1], "skip") {
					rename = "\x00skip"
				}
				continue
			}
			if t == "" || strings.HasPrefix(t, "//") || strings.HasPrefix(t, "#[") {
				continue
			}
			vm := reUnitVariant.FindStringSubmatch(t)
			if vm == nil {
				unit = false // a variant carries data
				continue
			}
			switch {
			case rename == "\x00skip":
			case rename != "":
				values = append(values, rename)
			default:
				values = append(values, renameVariant(vm[1], renameAll))
			}
			rename = ""
		}
		i = j
		if plain && unit && len(values) > 0 {
			result = append(result, &RustEnum{Name: m[1], Values: values, Source: path})
		}
	}
	return result
}

// renameVariant applies a container rename_all to a PascalCase variant name.
func renameVariant(name, renameAll string) string {
	var words []string
	start := 0
	for i := 1; i < len(name); i++ {
		if name[i] >= 'A' && name[i] <= 'Z' {
			words = append(words, name[start:i])
			start = i
		}
	}
	words = append(words, name[start:])
	lower := make([]string, len(words))
	for i, w := range words {
		lower[i] = strings.ToLower(w)
	}
	switch renameAll {
	case "lowercase":
		return strings.ToLower(name)
	case "UPPERCASE":
		return strings.ToUpper(name)
	case "snake_case":
		return strings.Join(lower, "_")
	case "SCREAMING_SNAKE_CASE":
		return strings.ToUpper(strings.Join(lower, "_"))
	case "kebab-case":
		return strings.Join(lower, "-")
	case "SCREAMING-KEBAB-CASE":
		return strings.ToUpper(strings.Join(lower, "-"))
	case "camelCase":
		return strings.ToLower(name[:1]) + name[1:]
	default: // PascalCase and none: the variant's own name
		return name
	}
}
