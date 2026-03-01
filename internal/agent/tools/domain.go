// Package tools provides the STRAP (Single Tool Resource Action Pattern) implementation.
//
// The STRAP pattern consolidates multiple individual tools into domain-based tools
// with resource+action routing. This reduces context window overhead by ~80% and
// improves LLM tool comprehension.
//
// Example usage:
//
//	file(resource: file, action: read, path: "/tmp/test.txt")
//	shell(resource: bash, action: exec, command: "ls -la")
//	web(action: fetch, url: "https://example.com")
package tools

import (
	"encoding/json"
	"fmt"
	"strings"
)

// DomainTool extends Tool with STRAP metadata
type DomainTool interface {
	Tool

	// Domain returns the domain name (e.g., "file", "shell", "web")
	Domain() string

	// Resources returns available resources in this domain
	Resources() []string

	// ActionsFor returns available actions for a given resource
	ActionsFor(resource string) []string
}

// DomainInput is the base input structure for STRAP tools
type DomainInput struct {
	Resource string `json:"resource,omitempty"`
	Action   string `json:"action"`
}

// ResourceConfig defines a resource and its available actions
type ResourceConfig struct {
	Name        string
	Actions     []string
	Description string
}

// FieldConfig defines a field in the domain schema
type FieldConfig struct {
	Name        string
	Type        string   // "string", "integer", "boolean", "array", "object"
	Description string
	Required    bool
	RequiredFor []string // Actions that require this field
	Enum        []string // Allowed values (optional)
	Default     any      // Default value (optional)
	Items       string         // Item type for arrays (e.g., "string"). Defaults to "string" if omitted.
	ItemSchema  map[string]any // Full JSON Schema for array items (overrides Items if set).
}

// DomainSchemaConfig configures JSON schema generation for domain tools
type DomainSchemaConfig struct {
	Domain      string
	Description string
	Resources   map[string]ResourceConfig
	Fields      []FieldConfig
	Examples    []string
}

// ValidateResourceAction validates resource and action against allowed values
func ValidateResourceAction(resource, action string, resources map[string]ResourceConfig) error {
	if len(resources) == 0 {
		// No resources defined - only validate action
		return nil
	}

	rc, ok := resources[resource]
	if !ok {
		// Try empty resource (for single-resource domains)
		rc, ok = resources[""]
		if !ok {
			validResources := make([]string, 0, len(resources))
			for r := range resources {
				if r != "" {
					validResources = append(validResources, r)
				}
			}
			return fmt.Errorf("unknown resource: %s (valid: %s)", resource, strings.Join(validResources, ", "))
		}
	}

	for _, a := range rc.Actions {
		if a == action {
			return nil
		}
	}

	return fmt.Errorf("unknown action '%s' for resource '%s' (valid: %s)",
		action, resource, strings.Join(rc.Actions, ", "))
}

// BuildDomainSchema generates a JSON schema for a domain tool
func BuildDomainSchema(cfg DomainSchemaConfig) json.RawMessage {
	// Build properties
	properties := make(map[string]any)
	required := []string{"action"}

	// Add resource field if multiple resources
	if len(cfg.Resources) > 1 {
		resourceNames := make([]string, 0, len(cfg.Resources))
		for name := range cfg.Resources {
			if name != "" {
				resourceNames = append(resourceNames, name)
			}
		}
		properties["resource"] = map[string]any{
			"type":        "string",
			"description": fmt.Sprintf("Resource type: %s", strings.Join(resourceNames, ", ")),
			"enum":        resourceNames,
		}
		required = append(required, "resource")
	}

	// Collect all actions across all resources for the action enum
	actionSet := make(map[string]bool)
	for _, rc := range cfg.Resources {
		for _, a := range rc.Actions {
			actionSet[a] = true
		}
	}
	actions := make([]string, 0, len(actionSet))
	for a := range actionSet {
		actions = append(actions, a)
	}

	properties["action"] = map[string]any{
		"type":        "string",
		"description": fmt.Sprintf("Action to perform: %s", strings.Join(actions, ", ")),
		"enum":        actions,
	}

	// Add field definitions
	for _, f := range cfg.Fields {
		prop := map[string]any{
			"type":        f.Type,
			"description": f.Description,
		}

		if len(f.Enum) > 0 {
			prop["enum"] = f.Enum
		}

		if f.Default != nil {
			prop["default"] = f.Default
		}

		// Arrays require an "items" field for valid JSON Schema
		if f.Type == "array" {
			if f.ItemSchema != nil {
				prop["items"] = f.ItemSchema
			} else {
				itemType := f.Items
				if itemType == "" {
					itemType = "string"
				}
				prop["items"] = map[string]any{"type": itemType}
			}
		}

		properties[f.Name] = prop

		// Add to required if globally required
		if f.Required {
			required = append(required, f.Name)
		}
	}

	// Build description with examples
	var desc strings.Builder
	desc.WriteString(cfg.Description)

	if len(cfg.Examples) > 0 {
		desc.WriteString("\n\nExamples:\n")
		for _, ex := range cfg.Examples {
			desc.WriteString(fmt.Sprintf("  %s\n", ex))
		}
	}

	schema := map[string]any{
		"type":        "object",
		"description": desc.String(),
		"properties":  properties,
		"required":    required,
	}

	data, _ := json.MarshalIndent(schema, "", "  ")
	return json.RawMessage(data)
}

// BuildDomainDescription generates a description string for domain tools
func BuildDomainDescription(cfg DomainSchemaConfig) string {
	var desc strings.Builder
	desc.WriteString(cfg.Description)

	// Add resource/action documentation
	if len(cfg.Resources) > 0 {
		desc.WriteString("\n\nResources and Actions:")
		for name, rc := range cfg.Resources {
			if name == "" {
				continue
			}
			desc.WriteString(fmt.Sprintf("\n- %s: %s", name, strings.Join(rc.Actions, ", ")))
			if rc.Description != "" {
				desc.WriteString(fmt.Sprintf(" (%s)", rc.Description))
			}
		}
	}

	// Add examples
	if len(cfg.Examples) > 0 {
		desc.WriteString("\n\nExamples:\n")
		for _, ex := range cfg.Examples {
			desc.WriteString(fmt.Sprintf("  %s\n", ex))
		}
	}

	return desc.String()
}

// ActionRequiresApproval checks if an action requires user approval
// based on a configurable list of dangerous actions
func ActionRequiresApproval(action string, dangerousActions []string) bool {
	for _, da := range dangerousActions {
		if da == action {
			return true
		}
	}
	return false
}
