package main

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strings"
)

// ── Rust Enum Parsing ───────────────────────────────────────────────────
//
// A Serialize enum a generated struct refers to is emitted as a TypeScript
// union in the shape serde writes it: unit variants as string literals,
// internally tagged variants as objects carrying the tag, adjacently tagged
// ones with their content under the content key, externally tagged ones as
// a one-key object.

// RustEnum is a parsed Rust enum with its serde representation.
type RustEnum struct {
	Name      string
	RenameAll string
	Tag       string // #[serde(tag = "...")]
	Content   string // #[serde(content = "...")]
	Untagged  bool
	Variants  []RustVariant
}

// RustVariant is one variant: unit, a single tuple type, or struct fields.
type RustVariant struct {
	Name   string
	Rename string
	Tuple  string      // the newtype's Rust type, for `Name(T)`
	Fields []RustField // for `Name { a: T }`
	Struct bool
}

var (
	reEnumHead    = regexp.MustCompile(`pub\s+enum\s+(\w+)\s*\{`)
	reVariantHead = regexp.MustCompile(`^(\w+)\s*(.*)$`)
	reEnumField   = regexp.MustCompile(`^(?:pub\s+)?(\w+)\s*:\s*(.+)$`)
	reTypeName    = regexp.MustCompile(`\b[A-Z]\w*\b`)
)

// scanEnums reads all .rs files in dir (non-recursive) and extracts enums
// that derive Serialize.
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
		data, err := os.ReadFile(filepath.Join(dir, e.Name()))
		if err != nil {
			continue
		}
		result = append(result, parseEnums(string(data))...)
	}
	return result
}

func parseEnums(src string) []*RustEnum {
	lines := strings.Split(src, "\n")
	var result []*RustEnum
	for i := 0; i < len(lines); i++ {
		if !reDerive.MatchString(strings.TrimSpace(lines[i])) {
			continue
		}
		e := &RustEnum{}
		j := i + 1
		for ; j < len(lines); j++ {
			t := strings.TrimSpace(lines[j])
			if m := reSerdeCont.FindStringSubmatch(t); m != nil {
				applySerdeEnumAttr(e, m[1])
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
		e.Name = m[1]
		// The body up to the closing brace at depth zero, as one string, so
		// variants are split the same way whether they span lines or not.
		var body strings.Builder
		depth := 1
		rest := lines[j][strings.Index(lines[j], "{")+1:]
		for k := j; depth > 0 && k < len(lines); k++ {
			line := rest
			if k > j {
				line = lines[k]
			}
			if c := strings.Index(line, "//"); c >= 0 {
				line = line[:c]
			}
			for _, ch := range line {
				if ch == '{' {
					depth++
				} else if ch == '}' {
					depth--
					if depth == 0 {
						break
					}
				}
				body.WriteRune(ch)
			}
			body.WriteRune('\n')
			i = k
		}
		e.Variants = parseVariants(body.String())
		result = append(result, e)
	}
	return result
}

func applySerdeEnumAttr(e *RustEnum, attr string) {
	for _, kv := range []struct {
		key string
		dst *string
	}{{"rename_all", &e.RenameAll}, {"tag", &e.Tag}, {"content", &e.Content}} {
		re := regexp.MustCompile(`\b` + kv.key + `\s*=\s*"([^"]+)"`)
		if m := re.FindStringSubmatch(attr); m != nil {
			*kv.dst = m[1]
		}
	}
	if regexp.MustCompile(`\buntagged\b`).MatchString(attr) {
		e.Untagged = true
	}
}

// parseVariants splits an enum body into variants at top-level commas.
func parseVariants(body string) []RustVariant {
	var variants []RustVariant
	rename := ""
	for _, part := range splitTopLevel(body, ',') {
		part = strings.TrimSpace(part)
		// Leading attributes belong to the variant after them.
		for strings.HasPrefix(part, "#[") {
			end := strings.Index(part, "]")
			if end < 0 {
				break
			}
			attr := part[:end+1]
			if m := regexp.MustCompile(`rename\s*=\s*"([^"]+)"`).FindStringSubmatch(attr); m != nil && !strings.Contains(attr, "rename_all") {
				rename = m[1]
			}
			part = strings.TrimSpace(part[end+1:])
		}
		if part == "" {
			continue
		}
		m := reVariantHead.FindStringSubmatch(part)
		if m == nil {
			continue
		}
		v := RustVariant{Name: m[1], Rename: rename}
		rename = ""
		tail := strings.TrimSpace(m[2])
		switch {
		case strings.HasPrefix(tail, "("):
			v.Tuple = strings.TrimSpace(strings.TrimSuffix(strings.TrimPrefix(tail, "("), ")"))
		case strings.HasPrefix(tail, "{"):
			v.Struct = true
			inner := strings.TrimSuffix(strings.TrimPrefix(tail, "{"), "}")
			for _, f := range splitTopLevel(inner, ',') {
				f = strings.TrimSpace(f)
				if fm := reEnumField.FindStringSubmatch(f); fm != nil {
					field := RustField{Name: fm[1], RustType: strings.TrimSpace(fm[2])}
					field.Optional = strings.HasPrefix(field.RustType, "Option<")
					v.Fields = append(v.Fields, field)
				}
			}
		}
		variants = append(variants, v)
	}
	return variants
}

// splitTopLevel splits s at sep outside <>, (), {} and [].
func splitTopLevel(s string, sep rune) []string {
	var parts []string
	depth, start := 0, 0
	for i, ch := range s {
		switch ch {
		case '<', '(', '{', '[':
			depth++
		case '>', ')', '}', ']':
			depth--
		case sep:
			if depth == 0 {
				parts = append(parts, s[start:i])
				start = i + len(string(sep))
			}
		}
	}
	return append(parts, s[start:])
}

// variantName is the variant's name as serde writes it.
func variantName(v RustVariant, renameAll string) string {
	if v.Rename != "" {
		return v.Rename
	}
	switch renameAll {
	case "snake_case":
		return pascalToSnake(v.Name)
	case "SCREAMING_SNAKE_CASE":
		return strings.ToUpper(pascalToSnake(v.Name))
	case "kebab-case":
		return strings.ReplaceAll(pascalToSnake(v.Name), "_", "-")
	case "lowercase":
		return strings.ToLower(v.Name)
	case "camelCase":
		return strings.ToLower(v.Name[:1]) + v.Name[1:]
	}
	return v.Name
}

func pascalToSnake(s string) string {
	var b strings.Builder
	for i, ch := range s {
		if ch >= 'A' && ch <= 'Z' {
			if i > 0 {
				b.WriteByte('_')
			}
			b.WriteRune(ch + ('a' - 'A'))
		} else {
			b.WriteRune(ch)
		}
	}
	return b.String()
}

// referencedEnums returns the enums the structs' fields reach, following
// the enums' own variants, by name.
func referencedEnums(structs map[string]*RustStruct, enums map[string]*RustEnum) []string {
	seen := map[string]bool{}
	var queue []string
	visit := func(rustType string) {
		for _, name := range reTypeName.FindAllString(rustType, -1) {
			if _, ok := enums[name]; ok && !seen[name] {
				seen[name] = true
				queue = append(queue, name)
			}
		}
	}
	for _, s := range structs {
		for _, f := range s.Fields {
			if !f.Skip {
				visit(f.RustType)
			}
		}
	}
	for len(queue) > 0 {
		e := enums[queue[0]]
		queue = queue[1:]
		for _, v := range e.Variants {
			visit(v.Tuple)
			for _, f := range v.Fields {
				visit(f.RustType)
			}
		}
	}
	names := make([]string, 0, len(seen))
	for n := range seen {
		names = append(names, n)
	}
	sort.Strings(names)
	return names
}

func emitEnum(b *strings.Builder, e *RustEnum, structs map[string]*RustStruct) {
	fields := func(fs []RustField) string {
		var parts []string
		for _, f := range fs {
			opt := ""
			if f.Optional {
				opt = "?"
			}
			parts = append(parts, fmt.Sprintf("%s%s: %s", f.Name, opt, rustTypeToTS(f.RustType, structs)))
		}
		return "{ " + strings.Join(parts, "; ") + " }"
	}
	var arms []string
	for _, v := range e.Variants {
		name := variantName(v, e.RenameAll)
		lit := fmt.Sprintf("'%s'", name)
		var payload string
		switch {
		case v.Tuple != "":
			payload = rustTypeToTS(v.Tuple, structs)
		case v.Struct:
			payload = fields(v.Fields)
		}
		switch {
		case e.Untagged:
			if payload == "" {
				arms = append(arms, "null")
			} else {
				arms = append(arms, payload)
			}
		case e.Tag != "" && e.Content != "":
			if payload == "" {
				arms = append(arms, fmt.Sprintf("{ %s: %s }", e.Tag, lit))
			} else {
				arms = append(arms, fmt.Sprintf("{ %s: %s; %s: %s }", e.Tag, lit, e.Content, payload))
			}
		case e.Tag != "":
			switch {
			case payload == "":
				arms = append(arms, fmt.Sprintf("{ %s: %s }", e.Tag, lit))
			case v.Struct:
				arms = append(arms, fmt.Sprintf("{ %s: %s; %s", e.Tag, lit, strings.TrimPrefix(payload, "{ ")))
			default:
				arms = append(arms, fmt.Sprintf("({ %s: %s } & %s)", e.Tag, lit, payload))
			}
		default:
			if payload == "" {
				arms = append(arms, lit)
			} else {
				arms = append(arms, fmt.Sprintf("{ %s: %s }", name, payload))
			}
		}
	}
	fmt.Fprintf(b, "export type %s =\n\t| %s\n\n", e.Name, strings.Join(arms, "\n\t| "))
}
