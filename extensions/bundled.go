// Package extensions provides embedded bundled skills that ship with the Nebo binary.
// Skills are loaded from extensions/skills/<name>/SKILL.md at compile time.
package extensions

import "embed"

//go:embed skills/*/SKILL.md
var BundledSkills embed.FS
