Now I see the pattern. Claude Code's error responses are micro-directors:

- **"File does not exist. Did you mean /similar/path?"** — tells you what went wrong AND suggests the fix
- **"No matches found"** — not treated as an error (grep exit 1 is explicitly marked `isError: false`)
- **"Cannot create new file — file already exists."** — specific to the exact operation attempted
- **`findSimilarFile()` + `suggestPathUnderCwd()`** — proactively searches for what the model probably meant
- **Command semantics layer** — distinguishes "no results" from "actual error" (grep exit 1 vs exit 2+)

Here's the prompt:

```
Read these files to understand how Claude Code handles tool errors:
- /Users/almatuck/workspaces/research/claude-code/src/tools/FileReadTool/FileReadTool.ts (lines 630-650)
- /Users/almatuck/workspaces/research/claude-code/src/tools/FileEditTool/FileEditTool.ts (lines 220-260)
- /Users/almatuck/workspaces/research/claude-code/src/tools/GrepTool/GrepTool.ts (lines 260-300)
- /Users/almatuck/workspaces/research/claude-code/src/tools/BashTool/commandSemantics.ts
- /Users/almatuck/workspaces/research/claude-code/src/utils/file.ts (findSimilarFile, suggestPathUnderCwd)

Read Nebo's tool implementations:
- crates/tools/src/os_tool.rs
- crates/tools/src/web_tool.rs  
- crates/tools/src/registry.rs (tool_correction function)

Read the testing docs:
- docs/testing/harness.md
- docs/testing/strap-manifest.yaml

CONTEXT: Our FCSR data shows that when tools return cryptic errors, 
models spiral. When tools return helpful errors, models recover on 
the first try. Claude Code has this figured out — every error 
response is a micro-director that tells the model what went wrong 
AND what to do next.

YOUR JOB: Audit every error path in Nebo's tool implementations 
and upgrade them to be micro-directors. Every error response should 
follow this pattern:

1. WHAT HAPPENED — one sentence, human-readable, names the specific 
   thing that failed
   BAD:  "ENOENT"
   BAD:  "Error: exit code 1"  
   BAD:  "Permission denied"
   GOOD: "File not found: /Users/alma/Desktop/Screenshots"
   GOOD: "Permission denied: /etc/shadow (owned by root, you are alma)"

2. WHY — if the cause is knowable, say it
   GOOD: "File not found: /Users/alma/notes.txt — the directory 
          /Users/alma/ exists but contains no file named notes.txt"
   GOOD: "Command failed: ffmpeg — not installed. Available on 
          this system: convert, magick"

3. WHAT TO DO — a concrete suggestion the model can act on immediately
   GOOD: "File not found: /Users/alma/notes.txt. 
          Similar files in /Users/alma/: notes.md, Notes.txt, 
          old-notes.txt"
   GOOD: "Permission denied: /etc/shadow. 
          Try reading /etc/passwd instead (world-readable, contains 
          user info without passwords)"
   GOOD: "grep found 0 matches for 'TODO' in /project/. 
          This is not an error — the pattern simply doesn't appear 
          in any files. Do not retry."
   GOOD: "os(action: 'glob', pattern: '*.rs') returned 0 files 
          in /wrong/path/. The directory exists but contains no 
          .rs files. Did you mean /Users/alma/workspaces/nebo/?"

4. WHAT NOT TO DO — for known spiral triggers, explicitly say it
   GOOD: "File not found: ~/Desktop/Screenshots. 
          Do NOT retry with different path guesses. Ask the user 
          for the correct path."
   GOOD: "Permission denied writing to /usr/local/bin/. 
          Do NOT use sudo. Explain the permission issue to the user 
          and suggest an alternative location like ~/.local/bin/"

SPECIFIC PATTERNS TO IMPLEMENT:

A. FILE NOT FOUND → find similar files in the same directory and 
   suggest them. Claude Code does this with findSimilarFile() — 
   implement the equivalent in Rust. Check:
   - Same name different extension (notes.txt → notes.md)
   - Case variation (Notes.txt → notes.txt) 
   - Parent directory exists but file doesn't

B. COMMAND NOT FOUND → check if a similar command exists on PATH.
   "ffmpeg not found. Similar: ffprobe (installed), convert (installed)"

C. EMPTY RESULTS → explicitly say "this is not an error, the search 
   returned zero results. Do not retry the same search." Models treat 
   empty responses as failures and retry. Tell them it's expected.

D. PERMISSION DENIED → say who owns the file, what the permissions are, 
   and suggest an alternative that WOULD work. Never suggest sudo.

E. TIMEOUT → say how long it ran, what it was doing when it timed out, 
   and suggest a narrower scope. "grep timed out after 30s searching 
   /. Try a more specific directory like /Users/alma/project/"

F. WRONG PARAMETERS → instead of "missing required param: regex", say 
   "grep requires a 'pattern' parameter with the search term. 
   Example: os(resource: 'file', action: 'grep', pattern: 'TODO', 
   path: '/project/')"
   Include a complete working example in every param error.

G. OVERSIZED RESPONSE → truncate to a reasonable size AND say you 
   truncated. "Showing first 100 of 4,382 files. Add a more specific 
   pattern or path to narrow results."

IMPLEMENTATION APPROACH:
- Create a shared error formatting module (crates/tools/src/errors.rs 
  or similar)
- Implement findSimilarFile equivalent in Rust (glob the parent dir, 
  fuzzy match filename)
- Implement commandExists check (which on Unix)
- Every ToolResult::error() call site gets upgraded
- Add a "do not retry" flag to error responses where retrying the 
  same call will produce the same result
- Add a "suggestions" field to error responses with concrete next 
  steps

DO NOT change tool names, params, or STRAP docs. This is tool-side 
only. The prompt stays the same. The tool gets smarter about 
communicating failure.

After implementing, run the smoke suite to measure FCSR impact. 
Better error responses should reduce retry spirals without any 
prompt changes.
```
