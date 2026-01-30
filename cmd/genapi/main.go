// cmd/genapi/main.go - Custom TypeScript API generator
// Replaces goctl with a custom solution for generating TypeScript interfaces and API clients

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
	Method      string // GET, POST, PUT, DELETE
	Path        string // /api/v1/users/{id}
	Handler     string // functionName in TypeScript
	Description string // JSDoc description
	Request     string // Request type (if any)
	Response    string // Response type
	PathParams  []string // Parameters in the path like {id}
}

// Field represents a struct field
type Field struct {
	Name       string
	Type       string
	JSONName   string
	PathName   string
	FormName   string
	Optional   bool
	Comment    string
}

// TypeDef represents a Go type definition
type TypeDef struct {
	Name    string
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

	// Generate API client
	routes := getRoutes()
	apiTS := generateAPIClient(routes, types)
	apiPath := "app/src/lib/api/nebo.ts"
	if err := os.WriteFile(apiPath, []byte(apiTS), 0644); err != nil {
		fmt.Fprintf(os.Stderr, "Error writing API client: %v\n", err)
		os.Exit(1)
	}
	fmt.Printf("Generated %s\n", apiPath)

	fmt.Println("TypeScript API generation complete!")
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
					continue // Skip embedded fields
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
			if part == "optional" {
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
	case "any", "interface{}":
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
		hasPathOrFormParams := false

		for _, f := range t.Fields {
			// Skip fields marked to ignore
			if f.JSONName == "-" {
				continue
			}

			// Categorize fields based on their tags
			if f.PathName != "" {
				// Path params - don't add to any interface, they become function args
				hasPathOrFormParams = true
			} else if f.FormName != "" {
				// Form/query params - add to Params interface
				hasPathOrFormParams = true
				formField := f
				formField.JSONName = f.FormName
				formFields = append(formFields, formField)
			} else if f.JSONName != "" {
				// This is a JSON body field
				jsonFields = append(jsonFields, f)
			}
		}

		// Write the main interface (JSON body fields only)
		writeInterface(&sb, t.Name, jsonFields, t.Comment)

		// For request types with path/form params, write a separate Params interface
		// (only contains form params, path params become function args)
		if hasPathOrFormParams {
			writeInterface(&sb, t.Name+"Params", formFields, "")
		}
	}

	return sb.String()
}

func writeInterface(sb *strings.Builder, name string, fields []Field, comment string) {
	if comment != "" {
		sb.WriteString("// " + strings.TrimSpace(comment) + "\n")
	}
	sb.WriteString("export interface " + name + " {\n")

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

	for _, r := range routes {
		// Generate JSDoc
		sb.WriteString("/**\n")
		sb.WriteString(" * @description \"" + r.Description + "\"\n")

		// Document parameters
		if r.Request != "" {
			sb.WriteString(" * @param req\n")
		}

		sb.WriteString(" */\n")

		// Generate function signature
		params := buildFunctionParams(r, typeMap)
		sb.WriteString("export function " + r.Handler + "(" + params + ") {\n")

		// Generate function body
		body := buildFunctionBody(r, typeMap)
		sb.WriteString("\treturn " + body + "\n")
		sb.WriteString("}\n\n")
	}

	return sb.String()
}

func buildFunctionParams(r Route, typeMap map[string]TypeDef) string {
	var params []string

	// Check if the request type has form params or path params (needs *Params interface)
	hasPathOrFormParams := false
	hasBodyParams := false

	if r.Request != "" {
		if t, ok := typeMap[r.Request]; ok {
			for _, f := range t.Fields {
				if f.PathName != "" || f.FormName != "" {
					hasPathOrFormParams = true
				}
				if f.JSONName != "" && f.PathName == "" && f.FormName == "" {
					hasBodyParams = true
				}
			}
		}
	}

	// Add params object for form params (empty object for path-only requests to match goctl behavior)
	if hasPathOrFormParams {
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
	hasPathOrFormParams := false
	hasBodyParams := false

	if r.Request != "" {
		if t, ok := typeMap[r.Request]; ok {
			for _, f := range t.Fields {
				if f.PathName != "" || f.FormName != "" {
					hasPathOrFormParams = true
				}
				if f.JSONName != "" && f.PathName == "" && f.FormName == "" {
					hasBodyParams = true
				}
			}
		}
	}

	switch method {
	case "get":
		if hasPathOrFormParams {
			return fmt.Sprintf("webapi.get<components.%s>(`%s`, params)", r.Response, path)
		}
		return fmt.Sprintf("webapi.get<components.%s>(`%s`)", r.Response, path)
	case "post":
		if hasPathOrFormParams && hasBodyParams {
			return fmt.Sprintf("webapi.post<components.%s>(`%s`, params, req)", r.Response, path)
		} else if hasBodyParams {
			return fmt.Sprintf("webapi.post<components.%s>(`%s`, req)", r.Response, path)
		} else if hasPathOrFormParams {
			return fmt.Sprintf("webapi.post<components.%s>(`%s`, params)", r.Response, path)
		}
		return fmt.Sprintf("webapi.post<components.%s>(`%s`)", r.Response, path)
	case "put":
		if hasPathOrFormParams && hasBodyParams {
			return fmt.Sprintf("webapi.put<components.%s>(`%s`, params, req)", r.Response, path)
		} else if hasBodyParams {
			return fmt.Sprintf("webapi.put<components.%s>(`%s`, req)", r.Response, path)
		} else if hasPathOrFormParams {
			return fmt.Sprintf("webapi.put<components.%s>(`%s`, params)", r.Response, path)
		}
		return fmt.Sprintf("webapi.put<components.%s>(`%s`)", r.Response, path)
	case "delete":
		if hasPathOrFormParams {
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

// getRoutes returns all API routes for code generation
func getRoutes() []Route {
	return []Route{
		// Health
		{Method: "GET", Path: "/health", Handler: "healthCheck", Description: "Health check endpoint", Response: "HealthResponse"},

		// Agent routes
		{Method: "GET", Path: "/api/v1/agent/sessions", Handler: "listAgentSessions", Description: "List agent sessions", Response: "ListAgentSessionsResponse"},
		{Method: "DELETE", Path: "/api/v1/agent/sessions/{id}", Handler: "deleteAgentSession", Description: "Delete agent session", Request: "DeleteAgentSessionRequest", Response: "MessageResponse", PathParams: []string{"id"}},
		{Method: "GET", Path: "/api/v1/agent/sessions/{id}/messages", Handler: "getAgentSessionMessages", Description: "Get session messages", Request: "GetAgentSessionRequest", Response: "GetAgentSessionMessagesResponse", PathParams: []string{"id"}},
		{Method: "GET", Path: "/api/v1/agent/settings", Handler: "getAgentSettings", Description: "Get agent settings", Response: "GetAgentSettingsResponse"},
		{Method: "PUT", Path: "/api/v1/agent/settings", Handler: "updateAgentSettings", Description: "Update agent settings", Request: "UpdateAgentSettingsRequest", Response: "GetAgentSettingsResponse"},
		{Method: "GET", Path: "/api/v1/agent/heartbeat", Handler: "getHeartbeat", Description: "Get heartbeat content (HEARTBEAT.md)", Response: "GetHeartbeatResponse"},
		{Method: "PUT", Path: "/api/v1/agent/heartbeat", Handler: "updateHeartbeat", Description: "Update heartbeat content (HEARTBEAT.md)", Request: "UpdateHeartbeatRequest", Response: "UpdateHeartbeatResponse"},
		{Method: "GET", Path: "/api/v1/agent/status", Handler: "getSimpleAgentStatus", Description: "Get simple agent status (single agent model)", Response: "SimpleAgentStatusResponse"},
		{Method: "GET", Path: "/api/v1/agents", Handler: "listAgents", Description: "List connected agents", Response: "ListAgentsResponse"},
		{Method: "GET", Path: "/api/v1/agents/{agentId}/status", Handler: "getAgentStatus", Description: "Get agent status", Request: "AgentStatusRequest", Response: "AgentStatusResponse", PathParams: []string{"agentId"}},

		// Auth routes
		{Method: "GET", Path: "/api/v1/auth/config", Handler: "getAuthConfig", Description: "Get auth configuration (OAuth providers enabled)", Response: "AuthConfigResponse"},
		{Method: "GET", Path: "/api/v1/auth/dev-login", Handler: "devLogin", Description: "Dev auto-login (local development only)", Response: "LoginResponse"},
		{Method: "POST", Path: "/api/v1/auth/forgot-password", Handler: "forgotPassword", Description: "Request password reset", Request: "ForgotPasswordRequest", Response: "MessageResponse"},
		{Method: "POST", Path: "/api/v1/auth/login", Handler: "login", Description: "User login", Request: "LoginRequest", Response: "LoginResponse"},
		{Method: "POST", Path: "/api/v1/auth/refresh", Handler: "refreshToken", Description: "Refresh authentication token", Request: "RefreshTokenRequest", Response: "RefreshTokenResponse"},
		{Method: "POST", Path: "/api/v1/auth/register", Handler: "register", Description: "Register new user", Request: "RegisterRequest", Response: "LoginResponse"},
		{Method: "POST", Path: "/api/v1/auth/resend-verification", Handler: "resendVerification", Description: "Resend email verification", Request: "ResendVerificationRequest", Response: "MessageResponse"},
		{Method: "POST", Path: "/api/v1/auth/reset-password", Handler: "resetPassword", Description: "Reset password with token", Request: "ResetPasswordRequest", Response: "MessageResponse"},
		{Method: "POST", Path: "/api/v1/auth/verify-email", Handler: "verifyEmail", Description: "Verify email address with token", Request: "EmailVerificationRequest", Response: "MessageResponse"},

		// Chat routes
		{Method: "GET", Path: "/api/v1/chats", Handler: "listChats", Description: "List user chats", Request: "ListChatsRequest", Response: "ListChatsResponse"},
		{Method: "POST", Path: "/api/v1/chats", Handler: "createChat", Description: "Create new chat", Request: "CreateChatRequest", Response: "CreateChatResponse"},
		{Method: "GET", Path: "/api/v1/chats/companion", Handler: "getCompanionChat", Description: "Get companion chat (auto-creates if needed)", Response: "GetChatResponse"},
		{Method: "GET", Path: "/api/v1/chats/days", Handler: "listChatDays", Description: "List days with messages for history browsing", Request: "ListChatDaysRequest", Response: "ListChatDaysResponse"},
		{Method: "GET", Path: "/api/v1/chats/history/{day}", Handler: "getHistoryByDay", Description: "Get messages for a specific day", Request: "GetHistoryByDayRequest", Response: "GetHistoryByDayResponse", PathParams: []string{"day"}},
		{Method: "POST", Path: "/api/v1/chats/message", Handler: "sendMessage", Description: "Send message (creates chat if needed)", Request: "SendMessageRequest", Response: "SendMessageResponse"},
		{Method: "GET", Path: "/api/v1/chats/search", Handler: "searchChatMessages", Description: "Search chat messages", Request: "SearchChatMessagesRequest", Response: "SearchChatMessagesResponse"},
		{Method: "GET", Path: "/api/v1/chats/{id}", Handler: "getChat", Description: "Get chat with messages", Request: "GetChatRequest", Response: "GetChatResponse", PathParams: []string{"id"}},
		{Method: "PUT", Path: "/api/v1/chats/{id}", Handler: "updateChat", Description: "Update chat title", Request: "UpdateChatRequest", Response: "Chat", PathParams: []string{"id"}},
		{Method: "DELETE", Path: "/api/v1/chats/{id}", Handler: "deleteChat", Description: "Delete chat", Request: "DeleteChatRequest", Response: "MessageResponse", PathParams: []string{"id"}},

		// Extensions routes
		{Method: "GET", Path: "/api/v1/extensions", Handler: "listExtensions", Description: "List all extensions (tools, skills, plugins)", Response: "ListExtensionsResponse"},
		{Method: "GET", Path: "/api/v1/skills/{name}", Handler: "getSkill", Description: "Get single skill details", Request: "GetSkillRequest", Response: "GetSkillResponse", PathParams: []string{"name"}},
		{Method: "POST", Path: "/api/v1/skills/{name}/toggle", Handler: "toggleSkill", Description: "Toggle skill enabled/disabled", Request: "ToggleSkillRequest", Response: "ToggleSkillResponse", PathParams: []string{"name"}},

		// Notification routes
		{Method: "GET", Path: "/api/v1/notifications", Handler: "listNotifications", Description: "List user notifications", Request: "ListNotificationsRequest", Response: "ListNotificationsResponse"},
		{Method: "DELETE", Path: "/api/v1/notifications/{id}", Handler: "deleteNotification", Description: "Delete notification", Request: "DeleteNotificationRequest", Response: "MessageResponse", PathParams: []string{"id"}},
		{Method: "PUT", Path: "/api/v1/notifications/{id}/read", Handler: "markNotificationRead", Description: "Mark notification as read", Request: "MarkNotificationReadRequest", Response: "MessageResponse", PathParams: []string{"id"}},
		{Method: "PUT", Path: "/api/v1/notifications/read-all", Handler: "markAllNotificationsRead", Description: "Mark all notifications as read", Response: "MessageResponse"},
		{Method: "GET", Path: "/api/v1/notifications/unread-count", Handler: "getUnreadCount", Description: "Get unread notification count", Response: "GetUnreadCountResponse"},

		// OAuth routes
		{Method: "POST", Path: "/api/v1/oauth/{provider}/callback", Handler: "oAuthCallback", Description: "OAuth callback - exchange code for tokens", Request: "OAuthLoginRequest", Response: "OAuthLoginResponse", PathParams: []string{"provider"}},
		{Method: "GET", Path: "/api/v1/oauth/{provider}/url", Handler: "getOAuthUrl", Description: "Get OAuth authorization URL", Request: "GetOAuthUrlRequest", Response: "GetOAuthUrlResponse", PathParams: []string{"provider"}},
		{Method: "DELETE", Path: "/api/v1/oauth/{provider}", Handler: "disconnectOAuth", Description: "Disconnect OAuth provider", Request: "DisconnectOAuthRequest", Response: "MessageResponse", PathParams: []string{"provider"}},
		{Method: "GET", Path: "/api/v1/oauth/providers", Handler: "listOAuthProviders", Description: "List connected OAuth providers", Response: "ListOAuthProvidersResponse"},

		// Models/Provider routes
		{Method: "GET", Path: "/api/v1/models", Handler: "listModels", Description: "List all available models from YAML cache", Response: "ListModelsResponse"},
		{Method: "PUT", Path: "/api/v1/models/{provider}/{modelId}", Handler: "updateModel", Description: "Update model settings (active, kind, preferred)", Request: "UpdateModelRequest", Response: "MessageResponse", PathParams: []string{"provider", "modelId"}},
		{Method: "PUT", Path: "/api/v1/models/task-routing", Handler: "updateTaskRouting", Description: "Update task routing configuration", Request: "UpdateTaskRoutingRequest", Response: "MessageResponse"},
		{Method: "GET", Path: "/api/v1/providers", Handler: "listAuthProfiles", Description: "List all auth profiles (API keys)", Response: "ListAuthProfilesResponse"},
		{Method: "POST", Path: "/api/v1/providers", Handler: "createAuthProfile", Description: "Create a new auth profile", Request: "CreateAuthProfileRequest", Response: "CreateAuthProfileResponse"},
		{Method: "GET", Path: "/api/v1/providers/{id}", Handler: "getAuthProfile", Description: "Get auth profile by ID", Request: "GetAuthProfileRequest", Response: "GetAuthProfileResponse", PathParams: []string{"id"}},
		{Method: "PUT", Path: "/api/v1/providers/{id}", Handler: "updateAuthProfile", Description: "Update auth profile", Request: "UpdateAuthProfileRequest", Response: "GetAuthProfileResponse", PathParams: []string{"id"}},
		{Method: "DELETE", Path: "/api/v1/providers/{id}", Handler: "deleteAuthProfile", Description: "Delete auth profile", Request: "DeleteAuthProfileRequest", Response: "MessageResponse", PathParams: []string{"id"}},
		{Method: "POST", Path: "/api/v1/providers/{id}/test", Handler: "testAuthProfile", Description: "Test auth profile (verify API key works)", Request: "TestAuthProfileRequest", Response: "TestAuthProfileResponse", PathParams: []string{"id"}},

		// Setup routes
		{Method: "POST", Path: "/api/v1/setup/admin", Handler: "createAdmin", Description: "Create the first admin user (only works when no admin exists)", Request: "CreateAdminRequest", Response: "CreateAdminResponse"},
		{Method: "POST", Path: "/api/v1/setup/complete", Handler: "completeSetup", Description: "Mark initial setup as complete", Response: "CompleteSetupResponse"},
		{Method: "GET", Path: "/api/v1/setup/personality", Handler: "getPersonality", Description: "Get AI personality configuration", Response: "GetPersonalityResponse"},
		{Method: "PUT", Path: "/api/v1/setup/personality", Handler: "updatePersonality", Description: "Update AI personality configuration", Request: "UpdatePersonalityRequest", Response: "UpdatePersonalityResponse"},
		{Method: "GET", Path: "/api/v1/setup/status", Handler: "setupStatus", Description: "Check if setup is required (no admin exists)", Response: "SetupStatusResponse"},

		// User routes
		{Method: "GET", Path: "/api/v1/user/me", Handler: "getCurrentUser", Description: "Get current user profile", Response: "GetUserResponse"},
		{Method: "PUT", Path: "/api/v1/user/me", Handler: "updateCurrentUser", Description: "Update current user profile", Request: "UpdateUserRequest", Response: "GetUserResponse"},
		{Method: "DELETE", Path: "/api/v1/user/me", Handler: "deleteAccount", Description: "Delete current user account", Request: "DeleteAccountRequest", Response: "MessageResponse"},
		{Method: "POST", Path: "/api/v1/user/me/change-password", Handler: "changePassword", Description: "Change password for authenticated user", Request: "ChangePasswordRequest", Response: "MessageResponse"},
		{Method: "GET", Path: "/api/v1/user/me/preferences", Handler: "getPreferences", Description: "Get user preferences", Response: "GetPreferencesResponse"},
		{Method: "PUT", Path: "/api/v1/user/me/preferences", Handler: "updatePreferences", Description: "Update user preferences", Request: "UpdatePreferencesRequest", Response: "GetPreferencesResponse"},
	}
}
