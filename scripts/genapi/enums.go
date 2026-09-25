package main

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

// ── Rust Enum Parsing ───────────────────────────────────────────────────
//
// A serialized struct can hold an enum field (`mode: Mode`). The enum is
// emitted as a TypeScript type in the shape serde writes it, so the field's
// type is defined wherever the struct is. Only enums a struct (or another
// emitted enum) refers to are emitted.

// RustEnum is a parsed Rust enum that derives Serialize.
type RustEnum struct {
	Name      string
	RenameAll string // variant renaming, e.g. "snake_case"
	Tag       string // #[serde(tag = "...")]
	Content   string // #[serde(content = "...")]
	Untagged  bool
	Variants  []RustVariant
}

// RustVariant is one variant: a unit, a newtype/tuple, or a struct variant.
type RustVariant struct {
	Name   string
	Rename string
	Tuple  []string    // newtype/tuple payload types
	Fields []RustField // struct variant fields
	IsUnit bool
}

var (
	reEnumHead  = regexp.MustCompile(`pub\s+enum\s+(\w+)\s*\{`)
	reVariant   = regexp.MustCompile(`^(\w+)\s*(\(|\{|,|$)`)
	reEnumField = regexp.MustCompile(`^(?:pub\s+)?(\w+)\s*:\s*(.+?)\s*,?$`)
	reAttrStr   = func(name string) *regexp.Regexp {
		return regexp.MustCompile(name + `\s*=\s*"([^"]+)"`)
	}
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
		en := &RustEnum{}
		j := i + 1
		for ; j < len(lines); j++ {
			t := strings.TrimSpace(lines[j])
			if m := reSerdeCont.FindStringSubmatch(t); m != nil {
				applyEnumContainerAttr(en, m[1])
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
		en.Name = m[1]
		j = parseVariants(lines, j+1, en)
		result = append(result, en)
		i = j
	}
	return result
}

func applyEnumContainerAttr(en *RustEnum, attr string) {
	if m := reAttrStr("rename_all").FindStringSubmatch(attr); m != nil {
		en.RenameAll = m[1]
	}
	if m := reAttrStr(`\btag`).FindStringSubmatch(attr); m != nil {
		en.Tag = m[1]
	}
	if m := reAttrStr("content").FindStringSubmatch(attr); m != nil {
		en.Content = m[1]
	}
	if regexp.MustCompile(`\buntagged\b`).MatchString(attr) {
		en.Untagged = true
	}
}

// parseVariants reads variants from line j until the enum's closing brace
// and returns the index after it.
func parseVariants(lines []string, j int, en *RustEnum) int {
	pendingRename := ""
	for j < len(lines) {
		t := strings.TrimSpace(lines[j])
		if t == "}" || t == "};" {
			return j + 1
		}
		if sm := reSerdeField.FindStringSubmatch(t); sm != nil {
			if m := reAttrStr("rename").FindStringSubmatch(sm[1]); m != nil {
				pendingRename = m[1]
			}
			j++
			continue
		}
		if t == "" || strings.HasPrefix(t, "//") || strings.HasPrefix(t, "#[") {
			j++
			continue
		}
		m := reVariant.FindStringSubmatch(t)
		if m == nil {
			j++
			continue
		}
		v := RustVariant{Name: m[1], Rename: pendingRename}
		pendingRename = ""
		switch m[2] {
		case "(":
			// Tuple payload, on one line: `Name(A, B),`
			open := strings.Index(t, "(")
			close := strings.LastIndex(t, ")")
			if close > open {
				for _, p := range splitGenericArgs(t[open+1 : close]) {
					if p != "" {
						v.Tuple = append(v.Tuple, p)
					}
				}
			}
			j++
		case "{":
			// Struct payload: on one line, or across lines until `},`.
			open := strings.Index(t, "{")
			if close := strings.LastIndex(t, "}"); close > open {
				v.Fields = parseInlineFields(t[open+1 : close])
				j++
			} else {
				j++
				var pending []string
				for j < len(lines) {
					ft := strings.TrimSpace(lines[j])
					j++
					if strings.HasPrefix(ft, "}") {
						break
					}
					if sm := reSerdeField.FindStringSubmatch(ft); sm != nil {
						pending = append(pending, sm[1])
						continue
					}
					if ft == "" || strings.HasPrefix(ft, "//") || strings.HasPrefix(ft, "#[") {
						continue
					}
					if fm := reEnumField.FindStringSubmatch(ft); fm != nil {
						f := RustField{Name: fm[1], RustType: strings.TrimSpace(fm[2])}
						for _, a := range pending {
							applySerdeFieldAttr(&f, a)
						}
						pending = nil
						if strings.HasPrefix(f.RustType, "Option<") {
							f.Optional = true
						}
						v.Fields = append(v.Fields, f)
					}
				}
			}
		default:
			v.IsUnit = true
			j++
		}
		en.Variants = append(en.Variants, v)
	}
	return j
}

func parseInlineFields(s string) []RustField {
	var fields []RustField
	for _, p := range splitGenericArgs(s) {
		if fm := reEnumField.FindStringSubmatch(strings.TrimSpace(p)); fm != nil {
			f := RustField{Name: fm[1], RustType: strings.TrimSpace(fm[2])}
			if strings.HasPrefix(f.RustType, "Option<") {
				f.Optional = true
			}
			fields = append(fields, f)
		}
	}
	return fields
}

// variantTag is the variant's name as serde writes it.
func variantTag(v RustVariant, renameAll string) string {
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
	case "camelCase":
		return strings.ToLower(v.Name[:1]) + v.Name[1:]
	case "lowercase":
		return strings.ToLower(v.Name)
	case "UPPERCASE":
		return strings.ToUpper(v.Name)
	default:
		return v.Name
	}
}

func pascalToSnake(s string) string {
	var b strings.Builder
	for i, r := range s {
		if r >= 'A' && r <= 'Z' {
			if i > 0 {
				b.WriteByte('_')
			}
			b.WriteRune(r + ('a' - 'A'))
		} else {
			b.WriteRune(r)
		}
	}
	return b.String()
}

// enumTS renders the enum as the TypeScript type of its serde encoding.
func enumTS(en *RustEnum, structs map[string]*RustStruct) string {
	fieldsTS := func(fields []RustField) string {
		var parts []string
		for _, f := range fields {
			if f.Skip {
				continue
			}
			opt := ""
			if f.Optional {
				opt = "?"
			}
			parts = append(parts, fmt.Sprintf("%s%s: %s", toTSFieldName(f, ""), opt, rustTypeToTS(f.RustType, structs)))
		}
		return strings.Join(parts, "; ")
	}
	payload := func(v RustVariant) string {
		switch {
		case len(v.Fields) > 0:
			return "{ " + fieldsTS(v.Fields) + " }"
		case len(v.Tuple) == 1:
			return rustTypeToTS(v.Tuple[0], structs)
		case len(v.Tuple) > 1:
			var ts []string
			for _, t := range v.Tuple {
				ts = append(ts, rustTypeToTS(t, structs))
			}
			return "[" + strings.Join(ts, ", ") + "]"
		default:
			return "{}"
		}
	}
	var arms []string
	for _, v := range en.Variants {
		tag := variantTag(v, en.RenameAll)
		switch {
		case en.Untagged:
			if v.IsUnit {
				arms = append(arms, "null")
			} else {
				arms = append(arms, payload(v))
			}
		case en.Tag != "" && en.Content != "":
			if v.IsUnit {
				arms = append(arms, fmt.Sprintf("{ %s: %q }", en.Tag, tag))
			} else {
				arms = append(arms, fmt.Sprintf("{ %s: %q; %s: %s }", en.Tag, tag, en.Content, payload(v)))
			}
		case en.Tag != "":
			switch {
			case len(v.Fields) > 0:
				arms = append(arms, fmt.Sprintf("{ %s: %q; %s }", en.Tag, tag, fieldsTS(v.Fields)))
			case len(v.Tuple) == 1:
				arms = append(arms, fmt.Sprintf("({ %s: %q } & %s)", en.Tag, tag, payload(v)))
			default:
				arms = append(arms, fmt.Sprintf("{ %s: %q }", en.Tag, tag))
			}
		default:
			if v.IsUnit {
				arms = append(arms, fmt.Sprintf("%q", tag))
			} else {
				arms = append(arms, fmt.Sprintf("{ %s: %s }", quoteKey(tag), payload(v)))
			}
		}
	}
	if len(arms) == 0 {
		return "never"
	}
	return strings.Join(arms, "\n\t| ")
}

func quoteKey(k string) string {
	if regexp.MustCompile(`^[A-Za-z_$][A-Za-z0-9_$]*$`).MatchString(k) {
		return k
	}
	return fmt.Sprintf("%q", k)
}

// referencedEnums returns the enums that the emitted structs refer to,
// directly or through another emitted enum, by name.
func referencedEnums(structs map[string]*RustStruct, enums map[string]*RustEnum) []string {
	reIdent := regexp.MustCompile(`\b[A-Z]\w*\b`)
	seen := map[string]bool{}
	var queue []string
	visit := func(rustType string) {
		for _, id := range reIdent.FindAllString(rustType, -1) {
			if _, isStruct := structs[id]; isStruct {
				continue
			}
			if _, ok := enums[id]; ok && !seen[id] {
				seen[id] = true
				queue = append(queue, id)
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
		en := enums[queue[0]]
		queue = queue[1:]
		for _, v := range en.Variants {
			for _, t := range v.Tuple {
				visit(t)
			}
			for _, f := range v.Fields {
				visit(f.RustType)
			}
		}
	}
	return sortedKeys(seen)
}
