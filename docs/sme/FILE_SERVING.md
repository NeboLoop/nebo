# File Serving & Image Rendering — Internal Reference

Nebo's file serving system connects agent-produced artifacts (screenshots, downloads, generated images) to the web UI chat. This document is the SME-level reference — read it to become immediately expert on how files flow from tool execution to rendered pixels in the browser.

**No public documentation exists.** Everything below is derived from source code.

---

## Architecture Overview

```
Tool Execution (screenshot, file write, etc.)
  │
  ├─ Saves file to <data_dir>/files/<name>.png
  │
  └─ Returns ToolResult { Content, ImageURL: "/api/v1/files/<name>.png" }
       │
       ▼
Agent Hub (agenthub/hub.go) receives tool result frame
  │  payload includes "image_url" key
  │
  ▼
ChatContext (realtime/chat.go) captures image_url
  │
  ├─ Appends contentBlock{Type:"image", ImageURL} to pendingRequest
  │
  ├─ Streams "image" WebSocket event to browser (real-time)
  │
  └─ On message complete: serializes contentBlocks into metadata JSON
       │
       ▼
Database (chat_messages.metadata)
  │  {"contentBlocks": [{"type":"image","imageURL":"/api/v1/files/..."}]}
  │
  ▼
Chat History API (GET /api/v1/chat/history)
  │  Returns metadata as JSON string in ChatMessage
  │
  ▼
Frontend (+page.svelte) parses metadata → extracts contentBlocks
  │
  ▼
MessageGroup.svelte renders <img src={block.imageURL}>
  │
  ▼
Browser requests /api/v1/files/<name>.png
  │
  ▼
ServeFileHandler (handler/files/filehandler.go)
  │  Resolves to <data_dir>/files/<name>.png
  │  Path traversal checks → Content-Type → http.ServeFile
  │
  ▼
Image rendered in chat bubble
```

---

## Storage Location

Files are served from a single directory:

```
<data_dir>/files/
```

Where `<data_dir>` is platform-specific:
- **macOS:** `~/Library/Application Support/Nebo/files/`
- **Linux:** `~/.config/nebo/files/`
- **Windows:** `%AppData%\Nebo\files\`
- **Override:** `NEBO_DATA_DIR` env var

The directory is created on demand via `os.MkdirAll` in both the file handler and the screenshot tool.

**Key function** (`filehandler.go:18-26`):
```go
func filesDir() (string, error) {
    dataDir, err := defaults.DataDir()
    if err != nil { return "", err }
    dir := filepath.Join(dataDir, "files")
    os.MkdirAll(dir, 0755)
    return dir, nil
}
```

---

## HTTP Endpoints

### Route Registration (`server.go:469-471`)

```go
r.Post("/files/browse", files.BrowseFilesHandler(svcCtx))
r.Get("/files/*", files.ServeFileHandler(svcCtx))
```

Both routes are **protected** (require JWT auth).

### GET /api/v1/files/* — Serve Files

Serves any file from `<data_dir>/files/` with security checks.

**Security layers:**
1. `filepath.Clean` rejects `..` traversal
2. `strings.HasPrefix(fullPath, baseDir)` ensures resolved path stays within the files dir
3. Only serves files, not directories

**Content-Type mapping** (`filehandler.go:69-84`):

| Extension | Content-Type |
|-----------|-------------|
| `.png` | `image/png` |
| `.jpg`, `.jpeg` | `image/jpeg` |
| `.gif` | `image/gif` |
| `.webp` | `image/webp` |
| `.svg` | `image/svg+xml` |
| `.pdf` | `application/pdf` |
| (other) | `application/octet-stream` |

**Caching:** `Cache-Control: public, max-age=3600` (1 hour).

### POST /api/v1/files/browse — Native File Picker

Desktop-mode only. Opens OS native file picker dialog and returns selected paths.

```go
type BrowseFilesResponse struct {
    Paths []string `json:"paths"`
}
```

Returns `501 Not Implemented` in headless mode (when `svcCtx.BrowseFiles()` is nil). The callback is installed via `svcCtx.SetBrowseFiles(fn)` during Wails desktop initialization.

---

## ToolResult ImageURL Contract

All tools that produce visual output use the `ImageURL` field on `ToolResult`:

```go
// registry.go:13-18
type ToolResult struct {
    Content  string `json:"content"`
    IsError  bool   `json:"is_error,omitempty"`
    ImageURL string `json:"image_url,omitempty"`
}
```

**Contract:** `ImageURL` must be a path that the file handler can resolve — typically `/api/v1/files/<filename>`. The file must exist in `<data_dir>/files/` for the URL to work.

**Tools that set ImageURL:**
- `screenshot` (capture action) — `screenshot.go:324`
- `screenshot` (see action) — `screenshot.go:305`

---

## Screenshot Tool File Saving

### Default Path (no output param)

```go
// screenshot.go:345-350
dataDir, _ := defaults.DataDir()
filesDir := filepath.Join(dataDir, "files")
os.MkdirAll(filesDir, 0755)
fileName = fmt.Sprintf("screenshot_%s.png", time.Now().Format("20060102_150405"))
outputPath = filepath.Join(filesDir, fileName)
```

File name format: `screenshot_YYYYMMDD_HHMMSS.png`

ImageURL: `/api/v1/files/screenshot_YYYYMMDD_HHMMSS.png` — resolves correctly.

### Custom Output Path (output param set)

```go
// screenshot.go:351-353
} else {
    fileName = filepath.Base(outputPath)
}
```

**KNOWN BUG:** When the agent specifies a custom `output` path, the file is saved to that arbitrary location, but `ImageURL` is still constructed as `/api/v1/files/<basename>`. The file handler looks in `<data_dir>/files/` where the file does NOT exist, resulting in a 404 and a broken image in the chat UI.

### See Action (annotated snapshots)

```go
// screenshot.go:276-306
fileName := fmt.Sprintf("screenshot_see_%s.png", time.Now().Format("20060102_150405"))
filePath := filepath.Join(filesDir, fileName)
os.WriteFile(filePath, snap.AnnotatedPNG, 0644)
```

Always saves to `<data_dir>/files/` — no bug here.

---

## Real-Time Streaming Pipeline

### Step 1: Tool Result → Agent Hub Frame

The runner sends tool results as frames through the agent WebSocket. The `image_url` field is included in the payload.

### Step 2: ChatContext Captures Image (`chat.go:307-314`)

```go
if imageURL, ok := payload["image_url"].(string); ok && imageURL != "" {
    if req, ok := c.pending[frame.ID]; ok {
        req.contentBlocks = append(req.contentBlocks, contentBlock{
            Type:     "image",
            ImageURL: imageURL,
        })
    }
}
```

### Step 3: WebSocket Event to Browser

A real-time `"image"` event is sent to the browser immediately:

```go
msg := &Message{
    Type: "image",
    Data: map[string]interface{}{
        "session_id": sessionID,
        "image_url":  imageURL,
    },
}
```

### Step 4: Frontend Handles Image Event (`+page.svelte:877-893`)

```typescript
function handleImage(data: Record<string, unknown>) {
    const imageURL = (data?.image_url as string) || '';
    if (!imageURL) return;
    if (currentStreamingMessage) {
        if (!currentStreamingMessage.contentBlocks) {
            currentStreamingMessage.contentBlocks = [];
        }
        currentStreamingMessage.contentBlocks = [
            ...currentStreamingMessage.contentBlocks,
            { type: 'image' as const, imageURL }
        ];
        replaceMessageById({ ...currentStreamingMessage });
    }
}
```

---

## Database Persistence

### ContentBlock Struct (`chat.go:50-55`)

```go
type contentBlock struct {
    Type          string `json:"type"`                    // "text", "tool", "image"
    Text          string `json:"text,omitempty"`
    ToolCallIndex *int   `json:"toolCallIndex,omitempty"`
    ImageURL      string `json:"imageURL,omitempty"`
}
```

### Metadata Serialization (`chat.go:992-1012`)

On message completion, contentBlocks are serialized into the `chat_messages.metadata` column:

```go
func (c *ChatContext) buildMetadata(req *pendingRequest) sql.NullString {
    metaMap := make(map[string]interface{})
    if len(req.toolCalls) > 0 {
        metaMap["toolCalls"] = req.toolCalls
    }
    if req.thinking != "" {
        metaMap["thinking"] = req.thinking
    }
    if len(req.contentBlocks) > 0 {
        metaMap["contentBlocks"] = req.contentBlocks
    }
    metaJSON, _ := json.Marshal(metaMap)
    return sql.NullString{String: string(metaJSON), Valid: true}
}
```

### Stored JSON Shape

```json
{
  "contentBlocks": [
    { "type": "text", "text": "Here is the screenshot..." },
    { "type": "tool", "toolCallIndex": 0 },
    { "type": "image", "imageURL": "/api/v1/files/screenshot_20260222_150405.png" }
  ],
  "toolCalls": [...],
  "thinking": "..."
}
```

---

## Frontend Rendering

### Metadata Parsing (`+page.svelte:287-326`)

```typescript
function parseMetadata(metadata: string | undefined): ParsedMetadata {
    if (!metadata) return {};
    const parsed = JSON.parse(metadata);
    const result: ParsedMetadata = {};
    if (parsed.contentBlocks && Array.isArray(parsed.contentBlocks)) {
        result.contentBlocks = parsed.contentBlocks;
    }
    return result;
}
```

### ContentBlock TypeScript Interface (`MessageGroup.svelte:15-22`)

```typescript
interface ContentBlock {
    type: 'text' | 'tool' | 'image';
    text?: string;
    toolCallIndex?: number;
    imageData?: string;
    imageMimeType?: string;
    imageURL?: string;
}
```

### Image Rendering (`MessageGroup.svelte:184-193`)

```svelte
{:else if block.type === 'image' && (block.imageData || block.imageURL)}
    <div class="rounded-xl overflow-hidden mb-1 max-w-sm">
        <img
            src={block.imageData
                ? `data:${block.imageMimeType || 'image/png'};base64,${block.imageData}`
                : block.imageURL}
            alt="Shared content"
            class="max-w-full h-auto rounded-xl"
        />
    </div>
```

Two rendering modes:
1. **Base64 inline** — `data:image/png;base64,...` (for images sent directly in the stream)
2. **URL-based** — `/api/v1/files/...` (standard path, browser fetches from file handler)

---

## Key Files

| File | Lines | Responsibility |
|------|-------|----------------|
| `internal/handler/files/filehandler.go` | 122 | HTTP handler: serve files + native file picker |
| `internal/agent/tools/screenshot.go` | 394 | Screenshot capture, file save, ImageURL construction |
| `internal/agent/tools/registry.go` | 13-18 | ToolResult struct with ImageURL field |
| `internal/realtime/chat.go` | 50-55, 307-314, 992-1012 | contentBlock struct, image capture, metadata serialization |
| `internal/server/server.go` | 469-471 | Route registration for /files/* |
| `app/src/routes/(app)/agent/+page.svelte` | 287-326, 877-893 | Metadata parsing, real-time image handling |
| `app/src/lib/components/chat/MessageGroup.svelte` | 15-22, 184-193 | ContentBlock interface, image rendering |
| `internal/types/types.go` | 109-117 | ChatMessage type (metadata field) |
| `internal/svc/servicecontext.go` | 214-226 | BrowseFiles callback (desktop mode) |

---

## Known Issues

### Custom Output Path Bug (screenshot tool)

**Symptom:** Agent specifies `screenshot(action: "capture", output: "nebo_desktop_shot.png")` → file is saved to CWD, but ImageURL points to `/api/v1/files/nebo_desktop_shot.png` → file handler looks in `<data_dir>/files/` → 404 → broken image in chat.

**Root cause:** `saveImageToFile` (`screenshot.go:342-371`) — when `outputPath != ""`, it saves to the custom path but constructs ImageURL from `filepath.Base(outputPath)`, assuming the file is in the files directory.

**Fix needed:** Always save a copy to `<data_dir>/files/` for web serving, regardless of custom output path.

### No Cleanup/Rotation

Files accumulate in `<data_dir>/files/` indefinitely. No TTL, no size limit, no cleanup mechanism. Screenshots from months ago persist.

### No Deduplication

Same-second screenshots overwrite each other (timestamp-based naming: `screenshot_YYYYMMDD_HHMMSS.png`).
