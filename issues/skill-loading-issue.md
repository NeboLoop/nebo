# Skill Loading Bug Report

## Issue
Skills can be created successfully but fail to load. The skill file is written to disk correctly, but the skill system doesn't recognize it.

## Reproduction Steps
1. Create skill with `skill(action: "create", name: "lvt-sdr", content: "...")`
2. Skill reports success: `Created skill 'lvt-sdr' at /Users/almatuck/Library/Application Support/Nebo/skills/lvt-sdr/SKILL.md`
3. Attempt to load with `skill(action: "load", name: "lvt-sdr")`
4. System responds: `Skill 'lvt-sdr' not found. Use skill(action: "catalog") to list available skills.`
5. `skill(action: "catalog")` shows: `No skills installed. Create one with skill(action: "create", name: "my-skill", content: "...")`
6. `skill(action: "help", name: "lvt-sdr")` also reports not found

## Evidence

### File exists on disk
```
/Users/almatuck/Library/Application Support/Nebo/skills/lvt-sdr/SKILL.md
```

### File content is valid YAML
- Has proper frontmatter with `name: lvt-sdr` and `description`
- Contains full skill documentation
- Verified via `system(resource: "file", action: "read")`

### Behavior pattern
- `create` → Success (file written)
- `load` → Fail ("not found")
- `catalog` → Shows empty ("No skills installed")
- `help` → Fail ("not found")
- `delete` → Reports success
- `create` again → Reports success again

## Hypothesis
The skill creation and skill loading are using different mechanisms. Possible causes:
1. **Cache issue** — Skill registry isn't being updated after file creation
2. **Path mismatch** — Creating to one path, searching in another
3. **Permission issue** — Can write but not read the skill index
4. **Async issue** — File written but index not flushed before load attempt
5. **Validation issue** — Skill content passes write validation but fails load validation silently

## Expected Behavior
After `skill(action: "create")` succeeds:
- `skill(action: "catalog")` should show the skill
- `skill(action: "load", name: "lvt-sdr")` should succeed
- `skill(action: "help", name: "lvt-sdr")` should return full content

## Actual Behavior
- `catalog` shows empty list
- `load` reports "not found" even though file exists
- `help` reports "not found"

## Impact
Users cannot use custom skills. All skill creation is effectively broken. This blocks workflows that depend on specialized capabilities.

## Environment
- OS: macOS (aarch64)
- Skill directory: `/Users/almatuck/Library/Application Support/Nebo/skills/`
- Skill file written: `/Users/almatuck/Library/Application Support/Nebo/skills/lvt-sdr/SKILL.md`

## Suggested Debug Steps
1. Check if skill index file exists and is being updated
2. Verify file permissions on skill directory
3. Check if there's a race condition between write and read
4. Review skill loading code path vs creation code path
5. Add logging to see where "not found" is determined

## Priority
High — skills are a core capability and this blocks all custom skill functionality.