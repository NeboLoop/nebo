package embeddings

import (
	"strings"
	"testing"
)

func TestSplitText_ShortText(t *testing.T) {
	text := "Hello world. This is a short text."
	chunks := SplitText(text)
	if len(chunks) != 1 {
		t.Fatalf("expected 1 chunk for short text, got %d", len(chunks))
	}
	if chunks[0].Text != text {
		t.Fatalf("expected chunk text to match input")
	}
	if chunks[0].StartChar != 0 || chunks[0].EndChar != len(text) {
		t.Fatalf("expected StartChar=0, EndChar=%d, got %d, %d", len(text), chunks[0].StartChar, chunks[0].EndChar)
	}
}

func TestSplitText_Empty(t *testing.T) {
	chunks := SplitText("")
	if len(chunks) != 0 {
		t.Fatalf("expected 0 chunks for empty text, got %d", len(chunks))
	}
	chunks = SplitText("   ")
	if len(chunks) != 0 {
		t.Fatalf("expected 0 chunks for whitespace text, got %d", len(chunks))
	}
}

func TestSplitText_LongText(t *testing.T) {
	// Build a text that's well over the threshold (~1800 chars)
	var sentences []string
	for i := 0; i < 50; i++ {
		sentences = append(sentences, "This is sentence number one that contains some meaningful content about testing.")
	}
	text := strings.Join(sentences, " ")

	chunks := SplitText(text)
	if len(chunks) < 2 {
		t.Fatalf("expected multiple chunks for long text (%d chars), got %d chunks", len(text), len(chunks))
	}

	// Verify all text is covered
	if chunks[0].StartChar != 0 {
		t.Fatalf("first chunk should start at 0, got %d", chunks[0].StartChar)
	}
	lastChunk := chunks[len(chunks)-1]
	if lastChunk.EndChar != len(text) {
		t.Fatalf("last chunk should end at %d, got %d", len(text), lastChunk.EndChar)
	}

	// Verify chunks are in order
	for i := 1; i < len(chunks); i++ {
		if chunks[i].Index != i {
			t.Fatalf("chunk %d has index %d", i, chunks[i].Index)
		}
		// Overlap: next chunk should start before previous chunk ends
		if chunks[i].StartChar >= chunks[i-1].EndChar {
			t.Fatalf("expected overlap between chunk %d (end=%d) and chunk %d (start=%d)",
				i-1, chunks[i-1].EndChar, i, chunks[i].StartChar)
		}
	}

	// Verify each chunk is under the max size (with some tolerance for sentence boundaries)
	for i, c := range chunks {
		if len(c.Text) > 2*defaultMaxChars {
			t.Fatalf("chunk %d is too large: %d chars", i, len(c.Text))
		}
	}
}

func TestSplitText_CustomOptions(t *testing.T) {
	// Build text > 500 chars
	var sentences []string
	for i := 0; i < 20; i++ {
		sentences = append(sentences, "This is a test sentence with some content.")
	}
	text := strings.Join(sentences, " ")

	// Use small chunk size to force splitting
	chunks := SplitText(text, WithMaxChars(200), WithOverlapChars(50))
	if len(chunks) < 2 {
		t.Fatalf("expected multiple chunks with small max, got %d", len(chunks))
	}
}

func TestSplitSentences(t *testing.T) {
	text := "First sentence. Second sentence. Third sentence."
	sentences := splitSentences(text)
	if len(sentences) != 3 {
		t.Fatalf("expected 3 sentences, got %d: %+v", len(sentences), sentences)
	}
	if sentences[0].text != "First sentence. " {
		t.Fatalf("unexpected first sentence: %q", sentences[0].text)
	}

	// Double newline boundary
	text2 := "First paragraph.\n\nSecond paragraph."
	sentences2 := splitSentences(text2)
	if len(sentences2) != 2 {
		t.Fatalf("expected 2 sentences for paragraph split, got %d: %+v", len(sentences2), sentences2)
	}
}
