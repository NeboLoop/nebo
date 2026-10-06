# Transcript JSON

Nebo writes this file when an employee transcribes audio or video (`generate_media`, kind `transcript`). Plugins that work from what was said read it: captions, edit by transcript, search. The Nebo Media plugin is one of them.

The file sits beside the source as `<name>.transcript.json`, unless the call named another path.

## Shape

```json
{
  "language": "en",
  "duration": 12.48,
  "words": [
    { "text": "Hello", "start": 0.32, "end": 0.61, "speaker": "S1" },
    { "text": "there.", "start": 0.61, "end": 1.1, "speaker": "S1" }
  ],
  "segments": [
    { "text": "Hello there.", "start": 0.32, "end": 1.1, "speaker": "S1" }
  ]
}
```

| Field | Type | Meaning |
|-------|------|---------|
| `language` | string | The spoken language as the transcriber reported it, usually a code such as `en`. Empty when unknown. |
| `duration` | number | Length of the audio in seconds. `0` when unknown. |
| `words` | array | Every word, sorted by `start`. Empty when the source gave no word timings. |
| `segments` | array | Caption-sized runs of words, sorted by `start`. |
| `text` | string | A word with its punctuation attached and no surrounding spaces. A segment's text is its words joined by single spaces. |
| `start`, `end` | number | Seconds from the start of the file, to the millisecond. `end >= start`. |
| `speaker` | string, optional | A label (`S1`, `S2`, …) in the order each voice first speaks. Left out when the audio has a single voice or the labels are unavailable. |

## Rules

- Segments cover the words in order and do not overlap. Nebo starts a new segment at a sentence end (`.`, `?` or `!`), a change of speaker, a pause of a second or more, or after 16 words. A transcriber that supplies its own segments may break them elsewhere.
- When there are no word timings, `words` is empty and the segments carry the timing. A transcript with neither has one segment spanning the file.
- Readers ignore fields they do not know, so fields may be added later. Fields are never renamed or removed.
