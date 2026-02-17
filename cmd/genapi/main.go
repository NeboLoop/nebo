// cmd/genapi/main.go - Custom TypeScript API generator
// Parses routes from server.go and generates TypeScript API client

package main

import (
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// Route defines an API route for code generation
type Route struct {
	Method      string   // GET, POST, PUT, DELETE
	Path        string   // /api/v1/users/{id}
	Handler     string   // functionName in TypeScript
	Description string   // JSDoc description
	Request     string   // Request type (if any)
	Response    string   // Response type
	PathParams  []string // Parameters in the path like {id}
	URLOnly     bool     // If true, generates a URL-returning function (not an API call)
}

// Explicit response type mappings for handlers that don't follow naming convention
var responseOverrides = map[string]string{
	"Register":             "LoginResponse",
	"GetCurrentUser":       "GetUserResponse",
	"UpdateCurrentUser":    "GetUserResponse",
	"GetAuthConfig":        "AuthConfigResponse",
	"GetCompanionChat":     "GetChatResponse",
	"GetAgentProfile":      "AgentProfileResponse",
	"GetMemoryStats":       "MemoryStatsResponse",
	"UpdateMemory":         "GetMemoryResponse",
	"GetAgents":            "ListAgentsResponse",
	"GetSystemStatus":      "SimpleAgentStatusResponse",
	"GetSimpleAgentStatus": "SimpleAgentStatusResponse",
	"GetSystemInfo":        "SystemInfoResponse",
	"GetUIView":            "UIView",
	"SendUIEvent":          "SendUIEventResponse",
	"ListAdvisors":         "ListAdvisorsResponse",
	"GetAdvisor":           "GetAdvisorResponse",
	"CreateAdvisor":        "GetAdvisorResponse",
	"UpdateAdvisor":        "GetAdvisorResponse",
	"DeleteAdvisor":        "DeleteAdvisorResponse",
	"Grants":               "GetAppOAuthGrantsResponse",
	"Disconnect":           "MessageResponse",
	"DeleteSkill":          "MessageResponse",
}

// Explicit handler name overrides (when auto-derived TS name isn't right)
var handlerNameOverrides = map[string]string{
	"Grants":     "getAppOAuthGrants",
	"Disconnect": "disconnectAppOAuth",
}

// Handlers to skip entirely (OAuth callbacks — not called from frontend)
var skipHandlers = map[string]bool{
	"Callback":              true,
	"NeboLoopOAuthCallback": true,
}

// Handlers that return a URL string instead of making an API call (e.g. OAuth redirects)
var urlOnlyHandlers = map[string]string{
	"Connect": "getAppOAuthConnectUrl",
}

// Explicit request type mappings for handlers that don't follow naming convention
var requestOverrides = map[string]string{
	"UpdateCurrentUser": "UpdateUserRequest",
}

// Field represents a struct field
type Field struct {
	Name     string
	Type     string
	JSONName string
	PathName string
	FormName string
	Optional bool
	Comment  string
}

// TypeDef represents a Go type definition
type TypeDef struct {
	Name    string
	Extends string // embedded struct name (TypeScript extends clause)
	Fields  []Field
	Comment string
}

func main() {
	// Parse types
	types, err := parseTypes("internal/types/types.go")
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error parsing types: %v\n", err)
		os.Exit(1)
	}

	// Generate TypeScript components
	componentsTS := generateComponents(types)
	outputPath := "app/src/lib/api/neboComponents.ts"
	if err := os.MkdirAll(filepath.Dir(outputPath), 0755); err != nil {
		fmt.Fprintf(os.Stderr, "Error creating directory: %v\n", err)
		os.Exit(1)
	}
	if err := os.WriteFile(outputPath, []byte(componentsTS), 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Error writing components: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Generated %s\n", outputPath)

	// Parse routes from server.go
	routes, err := parseRoutes("internal/server/server.go", types)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error parsing routes: %v\n", err)
		os.Exit(1)
	}

	// Generate API client
	apiTS := generateAPIClient(routes, types)
	apiPath := "app/src/lib/api/nebo.ts"
	if err := os.WriteFile(apiPath, []byte(apiTS), 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Error writing API client: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Generated %s\n", apiPath)

	fmt.Println("TypeScript API generation complete!")
}

// parseRoutes extracts route definitions from server.go
func parseRoutes(filename string, types []TypeDef) ([]Route, error) {
	content, err := os.ReadFile(filename)
	if err != nil {
		return nil, err
	}

	// Build a set of known type names for validation
	typeSet := make(map[string]bool)
	for _, t := range types {
		typeSet[t.Name] = true
	}

	var routes []Route

	// Match patterns like: r.Get("/path", handler.SomeHandler(svcCtx))
	// Also matches: handler.SomeHandler(svcCtx.Field) for dependency injection
	routePattern := regexp.MustCompile(`r\.(Get|Post|Put|Delete|Patch)\("([^"]+)",\s*(\w+)\.(\w+)\(svcCtx(?:\.\w+)?\)\)`)

	matches := routePattern.FindAllStringSubmatch(string(content), -1)

	for _, match := range matches {
		method := strings.ToUpper(match[1])
		path := match[2]
		handlerName := match[4]

		// Skip non-API routes
		if !strings.HasPrefix(path, "/") {
			continue
		}

		// Build the full path (routes in registerPublicRoutes and registerProtectedRoutes are under /api/v1)
		fullPath := path
		if !strings.HasPrefix(path, "/api/") && !strings.HasPrefix(path, "/health") && !strings.HasPrefix(path, "/ws") {
			fullPath = "/api/v1" + path
		}

		// Skip WebSocket and non-API routes
		if strings.Contains(fullPath, "/ws") || strings.Contains(fullPath, "/csrf-token") {
			continue
		}

		// Extract path parameters like {id}
		pathParamPattern := regexp.MustCompile(`\{(\w+)\}`)
		pathParamMatches := pathParamPattern.FindAllStringSubmatch(fullPath, -1)
		var pathParams []string
		for _, pm := range pathParamMatches {
			pathParams = append(pathParams, pm[1])
		}

		// Derive TypeScript function name from handler name
		// e.g., "ListChatsHandler" -> "listChats"
		baseName := strings.TrimSuffix(handlerName, "Handler")

		// Skip handlers that aren't API endpoints at all (OAuth callbacks)
		if skipHandlers[baseName] {
			continue
		}

		// Check if this is a URL-only handler (returns URL string, not JSON)
		urlOnly := false
		tsHandler := ""
		if urlName, ok := urlOnlyHandlers[baseName]; ok {
			urlOnly = true
			tsHandler = urlName
		} else if override, ok := handlerNameOverrides[baseName]; ok {
			tsHandler = override
		} else {
			tsHandler = toLowerCamel(baseName)
		}

		// Derive request/response types from handler name
		requestType := baseName + "Request"
		responseType := baseName + "Response"

		// Check for explicit request type override first
		if override, ok := requestOverrides[baseName]; ok {
			requestType = override
		} else if !typeSet[requestType] {
			requestType = ""
		}

		// Check for explicit response type override first
		if override, ok := responseOverrides[baseName]; ok {
			responseType = override
		} else if !typeSet[responseType] {
			// Try common alternatives
			if typeSet["MessageResponse"] && (method == "DELETE" || strings.HasPrefix(baseName, "Mark") || strings.HasPrefix(baseName, "Toggle")) {
				responseType = "MessageResponse"
			} else if typeSet[baseName] {
				// Sometimes the response is just the entity type (e.g., Chat)
				responseType = baseName
			} else {
				// Default to MessageResponse for mutations without specific response
				responseType = "MessageResponse"
			}
		}

		// Generate description from handler name
		description := generateDescription(baseName)

		routes = append(routes, Route{
			Method:      method,
			Path:        fullPath,
			Handler:     tsHandler,
			Description: description,
			Request:     requestType,
			Response:    responseType,
			PathParams:  pathParams,
			URLOnly:     urlOnly,
		})
	}

	// Deduplicate routes by handler name
	seen := make(map[string]bool)
	var deduped []Route
	for _, r := range routes {
		if !seen[r.Handler] {
			seen[r.Handler] = true
			deduped = append(deduped, r)
		}
	}

	return deduped, nil
}

// generateDescription creates a human-readable description from handler name
func generateDescription(baseName string) string {
	// Split by capital letters
	words := splitCamelCase(baseName)
	if len(words) == 0 {
		return baseName
	}
	words[0] = strings.Title(words[0])
	return strings.Join(words, " ")
}

// splitCamelCase splits a CamelCase string into words
func splitCamelCase(s string) []string {
	var words []string
	var current strings.Builder
	for i, r := range s {
		if i > 0 && r >= 'A' && r <= 'Z' {
			words = append(words, strings.ToLower(current.String()))
			current.Reset()
		}
		current.WriteRune(r)
	}
	if current.Len() > 0 {
		words = append(words, strings.ToLower(current.String()))
	}
	return words
}

func parseTypes(filename string) ([]TypeDef, error) {
	fset := token.NewFileSet()
	node, err := parser.ParseFile(fset, filename, nil, parser.ParseComments)
	if err != nil {
		return nil, err
	}

	var types []TypeDef

	for _, decl := range node.Decls {
		genDecl, ok := decl.(*ast.GenDecl)
		if !ok || genDecl.Tok != token.TYPE {
			continue
		}

		for _, spec := range genDecl.Specs {
			typeSpec, ok := spec.(*ast.TypeSpec)
			if !ok {
				continue
			}

			structType, ok := typeSpec.Type.(*ast.StructType)
			if !ok {
				continue
			}

			typeDef := TypeDef{
				Name: typeSpec.Name.Name,
			}

			if genDecl.Doc != nil {
				typeDef.Comment = genDecl.Doc.Text()
			}

			for _, field := range structType.Fields.List {
				if len(field.Names) == 0 {
					// Capture embedded struct name for TypeScript extends
					if ident, ok := field.Type.(*ast.Ident); ok {
						typeDef.Extends = ident.Name
					}
					continue
				}

				f := Field{
					Name: field.Names[0].Name,
					Type: exprToString(field.Type),
				}

				// Parse struct tag
				if field.Tag != nil {
					tag := strings.Trim(field.Tag.Value, "`")
					tagInfo := parseStructTag(tag)
					f.JSONName = tagInfo.JSONName
					f.PathName = tagInfo.PathName
					f.FormName = tagInfo.FormName
					f.Optional = tagInfo.Optional
				}

				// Use field name as JSON name if not specified
				if f.JSONName == "" && f.PathName == "" && f.FormName == "" {
					f.JSONName = toLowerCamel(f.Name)
				}

				if field.Comment != nil {
					f.Comment = strings.TrimSpace(field.Comment.Text())
				}

				typeDef.Fields = append(typeDef.Fields, f)
			}

			types = append(types, typeDef)
		}
	}

	return types, nil
}

func exprToString(expr ast.Expr) string {
	switch t := expr.(type) {
	case *ast.Ident:
		return t.Name
	case *ast.StarExpr:
		return "*" + exprToString(t.X)
	case *ast.ArrayType:
		return "[]" + exprToString(t.Elt)
	case *ast.MapType:
		return "map[" + exprToString(t.Key) + "]" + exprToString(t.Value)
	case *ast.SelectorExpr:
		return exprToString(t.X) + "." + t.Sel.Name
	default:
		return "any"
	}
}

// TagInfo contains parsed struct tag information
type TagInfo struct {
	JSONName string
	PathName string
	FormName string
	Optional bool
}

func parseStructTag(tag string) TagInfo {
	info := TagInfo{}

	// Parse json tag
	jsonRe := regexp.MustCompile(`json:"([^"]*)"`)
	if matches := jsonRe.FindStringSubmatch(tag); len(matches) >= 2 {
		jsonTag := matches[1]
		if jsonTag != "-" {
			parts := strings.Split(jsonTag, ",")
			info.JSONName = parts[0]
			for _, part := range parts[1:] {
				if part == "omitempty" || part == "optional" {
					info.Optional = true
				}
			}
		}
	}

	// Parse path tag
	pathRe := regexp.MustCompile(`path:"([^"]*)"`)
	if matches := pathRe.FindStringSubmatch(tag); len(matches) >= 2 {
		info.PathName = matches[1]
	}

	// Parse form tag
	formRe := regexp.MustCompile(`form:"([^"]*)"`)
	if matches := formRe.FindStringSubmatch(tag); len(matches) >= 2 {
		formTag := matches[1]
		parts := strings.Split(formTag, ",")
		info.FormName = parts[0]
		for _, part := range parts[1:] {
			if part == "optional" || part == "omitempty" {
				info.Optional = true
			}
		}
	}

	return info
}

func toLowerCamel(s string) string {
	if s == "" {
		return s
	}
	return strings.ToLower(s[:1]) + s[1:]
}

func goTypeToTS(goType string) string {
	// Handle pointers - make optional in caller
	goType = strings.TrimPrefix(goType, "*")

	switch goType {
	case "string":
		return "string"
	case "int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64":
		return "number"
	case "float32", "float64":
		return "number"
	case "bool":
		return "boolean"
	case "any", "interface{}", "json.RawMessage":
		return "any"
	}

	// Handle arrays
	if strings.HasPrefix(goType, "[]") {
		elemType := goType[2:]
		return "Array<" + goTypeToTS(elemType) + ">"
	}

	// Handle maps
	if strings.HasPrefix(goType, "map[") {
		// map[string]T -> { [key: string]: T }
		re := regexp.MustCompile(`map\[(\w+)\](.+)`)
		matches := re.FindStringSubmatch(goType)
		if len(matches) == 3 {
			keyType := goTypeToTS(matches[1])
			valType := goTypeToTS(matches[2])
			return "{ [key: " + keyType + "]: " + valType + " }"
		}
	}

	// Assume it's a custom type
	return goType
}

func generateComponents(types []TypeDef) string {
	var sb strings.Builder

	sb.WriteString("// Code generated by cmd/genapi. DO NOT EDIT.\n\n")

	// Sort types alphabetically
	sortedTypes := make([]TypeDef, len(types))
	copy(sortedTypes, types)
	sort.Slice(sortedTypes, func(i, j int) bool {
		return sortedTypes[i].Name < sortedTypes[j].Name
	})

	for _, t := range sortedTypes {
		var jsonFields []Field // Body fields (json tag)
		var formFields []Field // Query params (form tag only, NOT path)

		for _, f := range t.Fields {
			// Skip fields marked to ignore
			if f.JSONName == "-" {
				continue
			}

			// Categorize fields based on their tags
			if f.PathName != "" {
				// Path params - don't add to any interface, they become function args
			} else if f.FormName != "" {
				// Form/query params - add to Params interface
				formField := f
				formField.JSONName = f.FormName
				formFields = append(formFields, formField)
			} else if f.JSONName != "" {
				// This is a JSON body field
				jsonFields = append(jsonFields, f)
			}
		}

		// Write the main interface (JSON body fields only)
		writeInterface(&sb, t.Name, t.Extends, jsonFields, t.Comment)

		// For request types with form params, write a separate Params interface
		// (only contains form params, path params become function args)
		if len(formFields) > 0 {
			writeInterface(&sb, t.Name+"Params", "", formFields, "")
		}
	}

	return sb.String()
}

func writeInterface(sb *strings.Builder, name string, extends string, fields []Field, comment string) {
	if comment != "" {
		sb.WriteString("// " + strings.TrimSpace(comment) + "\n")
	}
	if extends != "" {
		sb.WriteString("export interface " + name + " extends " + extends + " {\n")
	} else {
		sb.WriteString("export interface " + name + " {\n")
	}

	for _, f := range fields {
		tsType := goTypeToTS(f.Type)
		optional := f.Optional || strings.HasPrefix(f.Type, "*")

		optMark := ""
		if optional {
			optMark = "?"
		}

		commentStr := ""
		if f.Comment != "" {
			commentStr = " // " + f.Comment
		}

		sb.WriteString("\t" + f.JSONName + optMark + ": " + tsType + commentStr + "\n")
	}

	sb.WriteString("}\n\n")
}

func generateAPIClient(routes []Route, types []TypeDef) string {
	var sb strings.Builder

	sb.WriteString(`import webapi from "./gocliRequest"
import * as components from "./neboComponents"
export * from "./neboComponents"

`)

	// Build a map of types for quick lookup
	typeMap := make(map[string]TypeDef)
	for _, t := range types {
		typeMap[t.Name] = t
	}

	// Sort routes for consistent output
	sort.Slice(routes, func(i, j int) bool {
		if routes[i].Path != routes[j].Path {
			return routes[i].Path < routes[j].Path
		}
		return routes[i].Method < routes[j].Method
	})

	for _, r := range routes {
		// Generate JSDoc
		sb.WriteString("/**\n")
		sb.WriteString(" * @description \"" + r.Description + "\"\n")

		// Document parameters
		if r.Request != "" && !r.URLOnly {
			sb.WriteString(" * @param req\n")
		}

		sb.WriteString(" */\n")

		if r.URLOnly {
			// URL-only routes return a path string (e.g. OAuth redirect endpoints)
			params := buildFunctionParams(r, typeMap)
			sb.WriteString("export function " + r.Handler + "(" + params + "): string {\n")
			sb.WriteString("\treturn `" + convertPathParams(r.Path) + "`\n")
		} else {
			// Standard JSON API routes
			params := buildFunctionParams(r, typeMap)
			sb.WriteString("export function " + r.Handler + "(" + params + ") {\n")
			body := buildFunctionBody(r, typeMap)
			sb.WriteString("\treturn " + body + "\n")
		}
		sb.WriteString("}\n\n")
	}

	return sb.String()
}

func buildFunctionParams(r Route, typeMap map[string]TypeDef) string {
	var params []string

	// Check if the request type has form params, path params, or body params
	hasFormParams := false
	hasBodyParams := false

	if r.Request != "" {
		if t, ok := typeMap[r.Request]; ok {
			for _, f := range t.Fields {
				if f.FormName != "" {
					hasFormParams = true
				}
				if f.JSONName != "" && f.PathName == "" && f.FormName == "" {
					hasBodyParams = true
				}
			}
		}
	}

	// Add params object only if there are actual form/query params (not just path params)
	if hasFormParams {
		params = append(params, "params: components."+r.Request+"Params")
	}

	// Add request body parameter for POST/PUT with body
	if hasBodyParams && r.Request != "" {
		params = append(params, "req: components."+r.Request)
	}

	// Add path parameters as trailing arguments
	for _, p := range r.PathParams {
		params = append(params, p+": string")
	}

	return strings.Join(params, ", ")
}

func buildFunctionBody(r Route, typeMap map[string]TypeDef) string {
	method := strings.ToLower(r.Method)
	path := convertPathParams(r.Path)

	// Check what parameters the request type has
	hasFormParams := false
	hasBodyParams := false

	if r.Request != "" {
		if t, ok := typeMap[r.Request]; ok {
			for _, f := range t.Fields {
				if f.FormName != "" {
					hasFormParams = true
				}
				if f.JSONName != "" && f.PathName == "" && f.FormName == "" {
					hasBodyParams = true
				}
			}
		}
	}

	switch method {
	case "get":
		if hasFormParams {
			return fmt.Sprintf("webapi.get<components.%s>(`%s`, params)", r.Response, path)
		}
		return fmt.Sprintf("webapi.get<components.%s>(`%s`)", r.Response, path)
	case "post":
		if hasFormParams && hasBodyParams {
			return fmt.Sprintf("webapi.post<components.%s>(`%s`, params, req)", r.Response, path)
		} else if hasBodyParams {
			return fmt.Sprintf("webapi.post<components.%s>(`%s`, req)", r.Response, path)
		} else if hasFormParams {
			return fmt.Sprintf("webapi.post<components.%s>(`%s`, params)", r.Response, path)
		}
		return fmt.Sprintf("webapi.post<components.%s>(`%s`)", r.Response, path)
	case "put":
		if hasFormParams && hasBodyParams {
			return fmt.Sprintf("webapi.put<components.%s>(`%s`, params, req)", r.Response, path)
		} else if hasBodyParams {
			return fmt.Sprintf("webapi.put<components.%s>(`%s`, req)", r.Response, path)
		} else if hasFormParams {
			return fmt.Sprintf("webapi.put<components.%s>(`%s`, params)", r.Response, path)
		}
		return fmt.Sprintf("webapi.put<components.%s>(`%s`)", r.Response, path)
	case "delete":
		if hasFormParams {
			return fmt.Sprintf("webapi.delete<components.%s>(`%s`, params)", r.Response, path)
		}
		return fmt.Sprintf("webapi.delete<components.%s>(`%s`)", r.Response, path)
	default:
		return fmt.Sprintf("webapi.%s<components.%s>(`%s`, req)", method, r.Response, path)
	}
}

func convertPathParams(path string) string {
	// Convert /api/v1/users/{id} to /api/v1/users/${id}
	re := regexp.MustCompile(`\{(\w+)\}`)
	return re.ReplaceAllString(path, "${$1}")
}
