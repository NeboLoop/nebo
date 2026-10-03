# Nebo Video

Probe, trim, and assemble videos from a declarative project JSON — ffmpeg-backed primitives for agent-driven video creation.

## When to use

Call this plugin whenever an agent needs to inspect, cut, or compose video files on disk. It is the editing/assembly layer that pairs with `generate_media` (AI generation) and `elevenlabs` (voice + music) to let an agent create a full video end-to-end.

Three actions in v0.1.0:

- **`video.probe`** — read metadata (duration, codec, dimensions, fps) from a file. Always call this first; downstream actions need to know what they're working with.
- **`video.trim`** — cut a `[start..end]` window out of a file into a new MP4.
- **`video.render`** — assemble a full video from a project JSON (shots, audio, overlays) into a single MP4.

## Runtime requirement

The plugin shells out to `ffmpeg` and `ffprobe`. They must be on `PATH`:

- macOS: `brew install ffmpeg`
- Debian/Ubuntu: `apt-get install ffmpeg`
- Windows: `winget install Gyan.FFmpeg`

If `ffmpeg` is missing the plugin returns a clear error pointing at the install docs — it does not try to download it.

## Project JSON schema (for `video.render`)

The minimum project:

```json
{
  "output": { "width": 1920, "height": 1080, "fps": 30, "codec": "h264" },
  "tracks": [
    { "type": "video", "clips": [
      { "source": "/abs/path/a.mp4", "start": 0.0, "duration": 4.0 },
      { "source": "/abs/path/b.mp4", "start": 4.0, "duration": 8.0, "trim_start": 1.2 }
    ]}
  ]
}
```

### Output

| Field | Type | Default | Notes |
|---|---|---|---|
| `width` | int | 1920 | Output width in pixels |
| `height` | int | 1080 | Output height |
| `fps` | int | 30 | Frames per second |
| `codec` | string | `"h264"` | `h264`, `h265`, `vp9`, `av1` |
| `video_bitrate` | string | `"5M"` | ffmpeg syntax (`5M`, `800k`) |
| `audio_bitrate` | string | `"128k"` | |
| `pixel_format` | string | `"yuv420p"` | For broadest player compatibility |

### Tracks

Each track has `type` and `clips[]`. The clip timeline is composed into a single `-filter_complex` graph; one ffmpeg process produces the final output.

**Video track** (`type: "video"`) — clips are concatenated in order, placed on the output timeline at each clip's `start`.

```json
{ "type": "video", "clips": [
  { "source": "/abs/a.mp4", "start": 0.0, "duration": 4.0, "trim_start": 0.0 }
]}
```

- `source` — absolute path to the source file
- `start` — time on the output timeline this clip begins (seconds)
- `duration` — length of this clip on the output timeline
- `trim_start` — where in the source to start reading (seconds, default 0)

**Audio track** (`type: "audio"`) — mixed down with the video track's native audio.

```json
{ "type": "audio", "clips": [
  { "source": "/abs/vo.wav", "start": 4.0, "volume": 1.0, "filter": "loudnorm=I=-16:TP=-1.5" }
]}
```

- `volume` — linear multiplier, default 1.0
- `filter` — raw ffmpeg audio filter string, applied before mixing

**Text track** (`type: "text"`) — simple drawtext overlay. Rich styling is v0.2.0.

```json
{ "type": "text", "clips": [
  { "text": "Hello", "start": 0.5, "duration": 2.0, "size": 64, "x": 0.5, "y": 0.5, "color": "white" }
]}
```

- `x`, `y` — normalized 0..1 (0.5, 0.5 is center)
- `size` — point size
- `color` — ffmpeg color name or `#RRGGBB`

## Example

```bash
# Probe
echo '{"input":"/tmp/raw.mp4"}' | nebo-video probe
# → {"duration": 42.0, "width": 1920, "height": 1080, "fps": 30, "video_codec": "h264", "audio_codec": "aac"}

# Trim
echo '{"input":"/tmp/raw.mp4","start":5.0,"end":15.0,"output":"/tmp/cut.mp4"}' | nebo-video trim
# → {"output":"/tmp/cut.mp4","duration":10.0,"size_bytes":1234567}

# Render
echo '{"project":"/tmp/project.json","output":"/tmp/final.mp4"}' | nebo-video render
# → {"output":"/tmp/final.mp4","duration":42.0,"size_bytes":9876543}
```

See `examples/simple-project.json` for a working project.

## Errors

Every action returns a JSON object. On failure:

```json
{ "error": "ffmpeg not found on PATH. Install: brew install ffmpeg" }
```

The process exits 0 on success and 1 on error. Nebo's PluginTool surfaces the stderr/stdout to the agent.

## Status

v0.1.0 — minimum useful set. Follow-up actions scheduled for v0.2.0:
`concat`, `thumbnail`, `overlay`, `audio_mix`, `silence` (detect + remove), `reframe` (vertical/square).
