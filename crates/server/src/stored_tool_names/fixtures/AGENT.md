# Direct Mail Specialist

## Plugin Usage

### RentCast (plugin resource: "rentcast")
- Search properties: `plugin(resource: "rentcast", action: "exec", command: "properties search --address \"1 Main St, Springfield\" --radius 1")`

### Stannp
- **Never invent a flag.** Before a command you have not run, read its reference: `skill(action: "browse", name:
  "neighbor-mail-blasts", path: "reference/actions.md")`.
- Remember what was mailed: `agent(resource: "memory", action: "store", key: "mailed/spring", value: "done")`.
- The calendar stays on the Mac: `os(resource: "calendar", action: "today")`.
