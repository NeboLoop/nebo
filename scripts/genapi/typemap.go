package main

import (
	"regexp"
	"strings"
	"unicode"
)

// rustTypeToTS converts a Rust type string to its TypeScript equivalent.
func rustTypeToTS(rustType string, structs map[string]*RustStruct) string {
	rustType = strings.TrimSpace(rustType)

	// Option<T> → T | undefined  (for interface fields, caller adds ?)
	if strings.HasPrefix(rustType, "Option<") && strings.HasSuffix(rustType, ">") {
		inner := rustType[7 : len(rustType)-1]
		return rustTypeToTS(inner, structs)
	}

	// Vec<T> → T[]
	if strings.HasPrefix(rustType, "Vec<") && strings.HasSuffix(rustType, ">") {
		inner := rustType[4 : len(rustType)-1]
		return rustTypeToTS(inner, structs) + "[]"
	}

	// HashMap<K, V> / BTreeMap<K, V> → Record<K, V>
	for _, prefix := range []string{"HashMap<", "BTreeMap<"} {
		if strings.HasPrefix(rustType, prefix) && strings.HasSuffix(rustType, ">") {
			inner := rustType[len(prefix) : len(rustType)-1]
			parts := splitGenericArgs(inner)
			if len(parts) == 2 {
				k := rustTypeToTS(parts[0], structs)
				v := rustTypeToTS(parts[1], structs)
				return "Record<" + k + ", " + v + ">"
			}
		}
	}

	// Box<T>, Arc<T>, etc. → unwrap
	for _, wrapper := range []string{"Box<", "Arc<", "Rc<", "Mutex<", "RwLock<"} {
		if strings.HasPrefix(rustType, wrapper) && strings.HasSuffix(rustType, ">") {
			inner := rustType[len(wrapper) : len(rustType)-1]
			return rustTypeToTS(inner, structs)
		}
	}

	// Primitives.
	switch rustType {
	case "String", "&str", "Cow<'_, str>", "Cow<str>":
		return "string"
	case "bool":
		return "boolean"
	case "i8", "i16", "i32", "i64", "i128", "isize",
		"u8", "u16", "u32", "u64", "u128", "usize",
		"f32", "f64":
		return "number"
	case "()", "":
		return "void"
	case "serde_json::Value", "Value":
		return "unknown"
	}

	// If we know this struct, use its name.
	if _, ok := structs[rustType]; ok {
		return rustType
	}

	// Unknown type — return as-is (will be caught as missing).
	return rustType
}

// splitGenericArgs splits "A, B" respecting nested angle brackets.
func splitGenericArgs(s string) []string {
	var parts []string
	depth := 0
	start := 0
	for i, ch := range s {
		switch ch {
		case '<':
			depth++
		case '>':
			depth--
		case ',':
			if depth == 0 {
				parts = append(parts, strings.TrimSpace(s[start:i]))
				start = i + 1
			}
		}
	}
	parts = append(parts, strings.TrimSpace(s[start:]))
	return parts
}

// toTSFieldName converts a Rust field name to its TypeScript name,
// applying serde rename rules.
func toTSFieldName(field RustField, renameAll string) string {
	if field.Rename != "" {
		return field.Rename
	}
	name := field.Name
	switch renameAll {
	case "camelCase":
		return snakeToCamel(name)
	case "PascalCase":
		return snakeToPascal(name)
	case "SCREAMING_SNAKE_CASE":
		return strings.ToUpper(name)
	case "kebab-case":
		return strings.ReplaceAll(name, "_", "-")
	default:
		return name // snake_case as-is
	}
}

func snakeToCamel(s string) string {
	parts := strings.Split(s, "_")
	for i := 1; i < len(parts); i++ {
		if len(parts[i]) > 0 {
			parts[i] = strings.ToUpper(parts[i][:1]) + parts[i][1:]
		}
	}
	return strings.Join(parts, "")
}

func snakeToPascal(s string) string {
	parts := strings.Split(s, "_")
	for i := range parts {
		if len(parts[i]) > 0 {
			parts[i] = strings.ToUpper(parts[i][:1]) + parts[i][1:]
		}
	}
	return strings.Join(parts, "")
}

// Modules whose name is prepended to the function name to avoid collisions.
// e.g. handlers::neboai::account_status → neboAIAccountStatus
var prefixedModules = map[string]string{
	"neboai": "nebo_loop",
	"user":     "user",
}

// handlerToFuncName converts a Rust handler function name to a TS function name.
// e.g. list_agent_chats → listAgentChats
// For prefixed modules: handlers::neboai::account_status → neboAIAccountStatus
func handlerToFuncName(handler string) string {
	parts := strings.Split(handler, "::")
	name := parts[len(parts)-1]
	if len(parts) >= 2 {
		module := parts[len(parts)-2]
		if prefix, ok := prefixedModules[module]; ok {
			name = prefix + "_" + name
		}
	}
	return snakeToCamel(name)
}

// handlerToResponseTypeName generates a response type name from a handler name.
// e.g. list_agents → ListAgentsResponse
func handlerToResponseTypeName(funcName string) string {
	return snakeToPascal(funcName) + "Response"
}

// resolveSerializeWith maps custom serializer names to TS types.
func resolveSerializeWith(name string) string {
	switch {
	case strings.Contains(name, "i64_as_bool"), strings.Contains(name, "as_bool"):
		return "boolean"
	case strings.Contains(name, "json_string_as_array"):
		return "string[]"
	default:
		return ""
	}
}

// extractPathParams extracts {param} segments from a route path.
func extractPathParams(path string) []string {
	re := regexp.MustCompile(`\{(\*?\w+)\}`)
	matches := re.FindAllStringSubmatch(path, -1)
	var params []string
	for _, m := range matches {
		name := strings.TrimPrefix(m[1], "*")
		params = append(params, name)
	}
	return params
}

// pathParamToTSName converts a path param name to a TS param name.
func pathParamToTSName(name string) string {
	// Common renames.
	switch name {
	case "id":
		return "id"
	}
	return snakeToCamel(name)
}

// deriveDescription generates a human-readable description from a camelCase function name.
func deriveDescription(name string) string {
	var words []string
	start := 0
	for i, r := range name {
		if unicode.IsUpper(r) && i > 0 {
			words = append(words, strings.ToLower(name[start:i]))
			start = i
		}
	}
	words = append(words, strings.ToLower(name[start:]))
	if len(words) > 0 {
		words[0] = strings.ToUpper(words[0][:1]) + words[0][1:]
	}
	return strings.Join(words, " ")
}
