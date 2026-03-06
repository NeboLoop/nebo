//! Zstd compression/decompression for frame payloads.

const COMPRESSION_THRESHOLD: usize = 1024; // Only compress payloads > 1 KB

/// Compress payload with zstd if it exceeds the threshold.
/// Returns `(data, was_compressed)`.
pub fn compress(payload: &[u8]) -> (Vec<u8>, bool) {
    if payload.len() <= COMPRESSION_THRESHOLD {
        return (payload.to_vec(), false);
    }

    match zstd::encode_all(payload, 1) {
        Ok(compressed) if compressed.len() < payload.len() => (compressed, true),
        _ => (payload.to_vec(), false),
    }
}

/// Decompress a zstd-compressed payload.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    zstd::decode_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_payload_not_compressed() {
        let small = b"hello";
        let (result, was_compressed) = compress(small);
        assert!(!was_compressed);
        assert_eq!(result, small);
    }

    #[test]
    fn test_large_payload_compressed() {
        // Repeating data compresses well
        let large = "a".repeat(2048);
        let (compressed, was_compressed) = compress(large.as_bytes());
        assert!(was_compressed);
        assert!(compressed.len() < large.len());
    }

    #[test]
    fn test_roundtrip() {
        let original = "hello world! ".repeat(200);
        let (compressed, was_compressed) = compress(original.as_bytes());
        assert!(was_compressed);
        let decompressed = decompress(&compressed).unwrap();
        assert_eq!(decompressed, original.as_bytes());
    }
}
