# Nebo Video

Probe, trim, and assemble videos from a declarative project JSON — ffmpeg-backed primitives for agent-driven video creation.

## When to use

Call this plugin whenever an agent needs to inspect, cut, or compose video files on disk. It is the editing/assembly layer that pairs with `generate_media` (AI generation) and `elevenlabs` (voice + music) to let an agent create a full video end-to-end.

Three actions in v0.1.0:

- **`video.probe`** — read metadata (duration, codec, dimensions, fps) from a file. Always call this first; downstream actions need to know what they're working with.
- **`video.trim`** — cut a `[start..end]` window out of a file into a new MP4.
- **`video.render`** — assemble a full video from a project JSON (shots, audio, overlays) into a single MP4.

## Self-contained

ffmpeg and ffprobe are built into the plugin. Nothing needs to be installed and a system ffmpeg is never used. On first run the plugin writes its copies under its data directory and reuses them after that. Output is always H.264 video with AAC audio in an MP4.

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
| `codec` | string | `"h264"` | Only `h264`; anything else is rejected |
| `video_bitrate` | string | `"5M"` | ffmpeg syntax (`5M`, `800k`) |
| `audio_bitrate` | string | `"128k"` | |
| `pixel_format` | string | `"yuv420p"` | For broadest player compatibility |

### Tracks

Each track has `type` and `clips[]`. The clip timeline is composed into a single `-filter_complex` graph; one ffmpeg process produces the final output.

**Video track** (`type: "video"`) — clips play back-to-back in list order. Each clip is scaled to fit the output size, with black bars when the aspect ratio differs. Keep each clip's `start` equal to the previous clip's end: gaps are not filled in v0.1.0.

```json
{ "type": "video", "clips": [
  { "source": "/abs/a.mp4", "start": 0.0, "duration": 4.0, "trim_start": 0.0 }
]}
```

- `source` — absolute path to the source file
- `start` — where the clip begins on the output timeline (seconds)
- `duration` — length of this clip on the output timeline
- `trim_start` — where in the source to start reading (seconds, default 0)

**Audio track** (`type: "audio"`) — every audio clip is placed at its `start` and mixed together. A video clip's own sound is not carried over in v0.1.0. To keep it, add an audio clip whose `source` is that video file, with the same `start`, `trim_start` and `duration`.

```json
{ "type": "audio", "clips": [
  { "source": "/abs/vo.wav", "start": 4.0, "volume": 1.0, "filter": "loudnorm=I=-16:TP=-1.5" }
]}
```

- `trim_start`, `duration` — the window to read from the source (default: from 0 to the end)
- `volume` — linear multiplier, default 1.0
- `filter` — raw ffmpeg audio filter string, applied before mixing

**Text track** (`type: "text"`) — a text overlay in the built-in Inter font. Any characters are fine (quotes, `%`, `:`, brackets); the text is shown exactly as written. Richer styling arrives in v0.2.0.

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
# → {"input":"/tmp/raw.mp4","duration":42.0,"width":1920,"height":1080,"fps":30.0,"video_codec":"h264","audio_codec":"aac",...}

# Trim
echo '{"input":"/tmp/raw.mp4","start":5.0,"end":15.0,"output":"/tmp/cut.mp4"}' | nebo-video trim
# → {"output":"/tmp/cut.mp4","duration":10.0,"size_bytes":1234567,"mode":"stream_copy"}

# Render
echo '{"project":"/tmp/project.json","output":"/tmp/final.mp4"}' | nebo-video render
# → {"output":"/tmp/final.mp4","size_bytes":9876543,"input_count":3}
```

See `examples/simple-project.json` for a working project.

## Errors

Every action returns a JSON object. On failure:

```json
{ "error": "trim: end (2) must be greater than start (5)" }
```

The process exits 0 on success and 1 on error. When ffmpeg itself fails, the error includes its message.

## Status

v0.1.0 — the minimum useful set. Planned for v0.2.0: carrying a video clip's own sound, transitions, and the `thumbnail`, `overlay`, `silence` (detect + remove) and `reframe` (vertical/square) actions.

## License

GPL-3.0-or-later, because the built-in ffmpeg includes the x264 encoder. See `LICENSE` and `NOTICE`.
