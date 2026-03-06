/// A chunk of text with character offsets.
#[derive(Debug, Clone)]
pub struct TextChunk {
    pub text: String,
    pub start_char: usize,
    pub end_char: usize,
}

/// Default max characters per chunk.
const DEFAULT_CHUNK_SIZE: usize = 1600;
/// Default overlap between chunks.
const DEFAULT_OVERLAP: usize = 320;
/// If text is shorter than this, return as single chunk.
const SHORT_CIRCUIT_SIZE: usize = 1920;

/// Split text into chunks at sentence boundaries with overlap.
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<TextChunk> {
    let chunk_size = if chunk_size == 0 { DEFAULT_CHUNK_SIZE } else { chunk_size };
    let overlap = if overlap == 0 { DEFAULT_OVERLAP } else { overlap };

    if text.len() <= SHORT_CIRCUIT_SIZE {
        return vec![TextChunk {
            text: text.to_string(),
            start_char: 0,
            end_char: text.len(),
        }];
    }

    // Find sentence boundaries
    let boundaries = find_sentence_boundaries(text);
    if boundaries.is_empty() {
        return vec![TextChunk {
            text: text.to_string(),
            start_char: 0,
            end_char: text.len(),
        }];
    }

    let mut chunks = Vec::new();
    let mut start = 0usize;

    while start < text.len() {
        let target_end = (start + chunk_size).min(text.len());

        // Find the best sentence boundary at or before target_end
        let end = boundaries
            .iter()
            .rev()
            .find(|&&b| b <= target_end && b > start)
            .copied()
            .unwrap_or(target_end);

        let chunk_text = text[start..end].trim();
        if !chunk_text.is_empty() {
            chunks.push(TextChunk {
                text: chunk_text.to_string(),
                start_char: start,
                end_char: end,
            });
        }

        if end >= text.len() {
            break;
        }

        // Advance with overlap
        let next_start = if end > overlap {
            // Find a sentence boundary near (end - overlap)
            let overlap_target = end - overlap;
            boundaries
                .iter()
                .find(|&&b| b >= overlap_target && b < end)
                .copied()
                .unwrap_or(end)
        } else {
            end
        };

        if next_start <= start {
            start = end; // avoid infinite loop
        } else {
            start = next_start;
        }
    }

    chunks
}

/// Convenience: chunk with default parameters.
pub fn chunk_text_default(text: &str) -> Vec<TextChunk> {
    chunk_text(text, DEFAULT_CHUNK_SIZE, DEFAULT_OVERLAP)
}

/// Find character positions of sentence boundaries (end of sentences).
fn find_sentence_boundaries(text: &str) -> Vec<usize> {
    let mut boundaries = Vec::new();
    let bytes = text.as_bytes();

    // Split on paragraph boundaries (\n\n)
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
            boundaries.push(i + 2);
            i += 2;
            continue;
        }

        // Sentence-ending punctuation followed by space or newline
        if (bytes[i] == b'.' || bytes[i] == b'!' || bytes[i] == b'?')
            && i + 1 < bytes.len()
            && (bytes[i + 1] == b' ' || bytes[i + 1] == b'\n')
        {
            boundaries.push(i + 1);
        }

        i += 1;
    }

    // Always include end of text
    if boundaries.last() != Some(&bytes.len()) {
        boundaries.push(bytes.len());
    }

    boundaries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_text_single_chunk() {
        let text = "Hello world. This is short.";
        let chunks = chunk_text_default(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, text);
        assert_eq!(chunks[0].start_char, 0);
    }

    #[test]
    fn test_long_text_multiple_chunks() {
        // Create text longer than SHORT_CIRCUIT_SIZE
        let sentence = "This is a test sentence. ";
        let text: String = sentence.repeat(100); // ~2500 chars
        let chunks = chunk_text_default(&text);
        assert!(chunks.len() > 1);

        // All chunks should be non-empty
        for chunk in &chunks {
            assert!(!chunk.text.is_empty());
        }
    }

    #[test]
    fn test_chunk_boundaries_are_sentence_aligned() {
        let text = "First sentence. Second sentence. Third sentence. ".repeat(50);
        let chunks = chunk_text(&text, 200, 50);
        // Most chunks should end at sentence boundaries
        for chunk in &chunks {
            let trimmed = chunk.text.trim();
            if !trimmed.is_empty() {
                let last_char = trimmed.chars().last().unwrap();
                assert!(
                    last_char == '.' || last_char == '!' || last_char == '?',
                    "chunk doesn't end at sentence boundary: ...{}",
                    &trimmed[trimmed.len().saturating_sub(20)..]
                );
            }
        }
    }

    #[test]
    fn test_chunk_offsets() {
        let text = "AAA. BBB. CCC. DDD. EEE. ".repeat(100);
        let chunks = chunk_text_default(&text);
        for chunk in &chunks {
            assert!(chunk.start_char <= chunk.end_char);
            assert!(chunk.end_char <= text.len());
        }
    }

    #[test]
    fn test_empty_text() {
        let chunks = chunk_text_default("");
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.is_empty());
    }

    #[test]
    fn test_paragraph_boundaries() {
        let text = "Paragraph one has content.\n\nParagraph two has content.\n\nParagraph three.".repeat(40);
        let chunks = chunk_text_default(&text);
        assert!(chunks.len() >= 1);
    }
}
