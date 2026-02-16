package embeddings

import (
	"strings"
	"unicode/utf8"
)

// Chunk represents a segment of text with position information.
type Chunk struct {
	Text      string
	StartChar int
	EndChar   int
	Index     int
}

// Default chunking parameters (~400 tokens ≈ 1600 chars, ~80 tokens ≈ 320 chars overlap)
const (
	defaultMaxChars     = 1600
	defaultOverlapChars = 320
)

// ChunkOption configures text chunking behavior.
type ChunkOption func(*chunkConfig)

type chunkConfig struct {
	maxChars     int
	overlapChars int
}

// WithMaxChars sets the maximum characters per chunk.
func WithMaxChars(n int) ChunkOption {
	return func(c *chunkConfig) { c.maxChars = n }
}

// WithOverlapChars sets the overlap characters between chunks.
func WithOverlapChars(n int) ChunkOption {
	return func(c *chunkConfig) { c.overlapChars = n }
}

// SplitText splits text into overlapping chunks for embedding.
// Uses sentence-boundary splitting with configurable size/overlap.
// Short texts (< ~450 tokens) stay as a single chunk.
func SplitText(text string, opts ...ChunkOption) []Chunk {
	cfg := chunkConfig{
		maxChars:     defaultMaxChars,
		overlapChars: defaultOverlapChars,
	}
	for _, o := range opts {
		o(&cfg)
	}

	text = strings.TrimSpace(text)
	if text == "" {
		return nil
	}

	// Short text → single chunk (not worth splitting if it barely exceeds one chunk)
	minSplitLen := cfg.maxChars + cfg.overlapChars
	if utf8.RuneCountInString(text) < minSplitLen {
		return []Chunk{{
			Text:      text,
			StartChar: 0,
			EndChar:   len(text),
			Index:     0,
		}}
	}

	// Split into sentences
	sentences := splitSentences(text)
	if len(sentences) <= 1 {
		return []Chunk{{
			Text:      text,
			StartChar: 0,
			EndChar:   len(text),
			Index:     0,
		}}
	}

	var chunks []Chunk
	chunkIdx := 0
	sentIdx := 0

	for sentIdx < len(sentences) {
		// Accumulate sentences up to maxChars
		var buf strings.Builder
		startSent := sentIdx
		startChar := sentences[sentIdx].start

		for sentIdx < len(sentences) {
			s := sentences[sentIdx]
			if buf.Len()+len(s.text) > cfg.maxChars && buf.Len() > 0 {
				break
			}
			buf.WriteString(s.text)
			sentIdx++
		}

		chunkText := buf.String()
		endChar := startChar + len(chunkText)

		chunks = append(chunks, Chunk{
			Text:      chunkText,
			StartChar: startChar,
			EndChar:   endChar,
			Index:     chunkIdx,
		})
		chunkIdx++

		if sentIdx >= len(sentences) {
			break
		}

		// Rewind for overlap: walk backwards from current position
		// until we've accumulated ~overlapChars of text
		overlapLen := 0
		rewindTo := sentIdx
		for i := sentIdx - 1; i >= startSent+1; i-- {
			overlapLen += len(sentences[i].text)
			if overlapLen >= cfg.overlapChars {
				rewindTo = i
				break
			}
			rewindTo = i
		}
		sentIdx = rewindTo
	}

	return chunks
}

type sentence struct {
	text  string
	start int
}

// splitSentences splits text into sentences at natural boundaries.
// Preserves the delimiter with the sentence that precedes it.
func splitSentences(text string) []sentence {
	var sentences []sentence
	start := 0

	for i := 0; i < len(text); i++ {
		// Double newline is always a boundary
		if i < len(text)-1 && text[i] == '\n' && text[i+1] == '\n' {
			end := i + 2
			sentences = append(sentences, sentence{text: text[start:end], start: start})
			start = end
			i++ // skip the second newline
			continue
		}

		// Sentence-ending punctuation followed by space or newline
		if (text[i] == '.' || text[i] == '!' || text[i] == '?') && i+1 < len(text) {
			next := text[i+1]
			if next == ' ' || next == '\n' || next == '\t' {
				end := i + 2
				if end > len(text) {
					end = len(text)
				}
				sentences = append(sentences, sentence{text: text[start:end], start: start})
				start = end
				i++ // skip the delimiter
				continue
			}
		}
	}

	// Remaining text
	if start < len(text) {
		sentences = append(sentences, sentence{text: text[start:], start: start})
	}

	return sentences
}
