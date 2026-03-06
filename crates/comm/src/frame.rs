//! 47-byte binary header codec for the NeboLoop comms protocol.
//!
//! Header layout (47 bytes, big-endian):
//!
//! ```text
//! [0]     proto_version   u8
//! [1]     frame_type      u8
//! [2]     flags           u8  (bit0=compressed, bit1=encrypted, bit2=ephemeral)
//! [3-6]   payload_len     u32
//! [7-22]  msg_id          16 bytes (ULID)
//! [23-38] conversation_id 16 bytes (UUID)
//! [39-46] seq             u64
//! ```

use thiserror::Error;

pub const HEADER_SIZE: usize = 47;
pub const PROTO_VERSION: u8 = 1;
pub const MAX_PAYLOAD_LEN: u32 = 32 * 1024; // 32 KB

// Frame types (u8, 1-13).
pub const TYPE_CONNECT: u8 = 1;
pub const TYPE_AUTH_OK: u8 = 2;
pub const TYPE_AUTH_FAIL: u8 = 3;
pub const TYPE_JOIN_CONVERSATION: u8 = 4;
pub const TYPE_LEAVE_CONVERSATION: u8 = 5;
pub const TYPE_SEND_MESSAGE: u8 = 6;
pub const TYPE_MESSAGE_DELIVERY: u8 = 7;
pub const TYPE_ACK: u8 = 8;
pub const TYPE_PRESENCE: u8 = 9;
pub const TYPE_TYPING: u8 = 10;
pub const TYPE_SLOW_DOWN: u8 = 11;
pub const TYPE_REPLAY: u8 = 12;
pub const TYPE_CLOSE: u8 = 13;

// Flag bits.
pub const FLAG_COMPRESSED: u8 = 1 << 0;
pub const FLAG_ENCRYPTED: u8 = 1 << 1;
pub const FLAG_EPHEMERAL: u8 = 1 << 2;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("unsupported protocol version: got {got}, want {PROTO_VERSION}")]
    BadVersion { got: u8 },
    #[error("payload exceeds maximum size ({len} > {MAX_PAYLOAD_LEN})")]
    PayloadTooLarge { len: u32 },
    #[error("short read: need {need} bytes, got {got}")]
    ShortRead { need: usize, got: usize },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Fixed 47-byte header preceding every frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Header {
    pub version: u8,
    pub frame_type: u8,
    pub flags: u8,
    pub payload_len: u32,
    pub msg_id: [u8; 16],
    pub conversation_id: [u8; 16],
    pub seq: u64,
}

impl Header {
    pub fn is_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }

    pub fn is_encrypted(&self) -> bool {
        self.flags & FLAG_ENCRYPTED != 0
    }

    pub fn is_ephemeral(&self) -> bool {
        self.flags & FLAG_EPHEMERAL != 0
    }
}

/// Encode a header and payload into a single byte vector.
pub fn encode(mut header: Header, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_PAYLOAD_LEN as usize {
        return Err(FrameError::PayloadTooLarge {
            len: payload.len() as u32,
        });
    }
    header.payload_len = payload.len() as u32;
    header.version = PROTO_VERSION;

    let mut out = vec![0u8; HEADER_SIZE + payload.len()];
    out[0] = header.version;
    out[1] = header.frame_type;
    out[2] = header.flags;
    out[3..7].copy_from_slice(&header.payload_len.to_be_bytes());
    out[7..23].copy_from_slice(&header.msg_id);
    out[23..39].copy_from_slice(&header.conversation_id);
    out[39..47].copy_from_slice(&header.seq.to_be_bytes());
    out[HEADER_SIZE..].copy_from_slice(payload);
    Ok(out)
}

/// Decode a byte slice into a header and payload.
pub fn decode(data: &[u8]) -> Result<(Header, &[u8]), FrameError> {
    if data.len() < HEADER_SIZE {
        return Err(FrameError::ShortRead {
            need: HEADER_SIZE,
            got: data.len(),
        });
    }

    let version = data[0];
    if version != PROTO_VERSION {
        return Err(FrameError::BadVersion { got: version });
    }

    let payload_len = u32::from_be_bytes([data[3], data[4], data[5], data[6]]);
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(FrameError::PayloadTooLarge { len: payload_len });
    }

    let end = HEADER_SIZE + payload_len as usize;
    if data.len() < end {
        return Err(FrameError::ShortRead {
            need: end,
            got: data.len(),
        });
    }

    let mut msg_id = [0u8; 16];
    msg_id.copy_from_slice(&data[7..23]);
    let mut conversation_id = [0u8; 16];
    conversation_id.copy_from_slice(&data[23..39]);

    let header = Header {
        version,
        frame_type: data[1],
        flags: data[2],
        payload_len,
        msg_id,
        conversation_id,
        seq: u64::from_be_bytes([
            data[39], data[40], data[41], data[42], data[43], data[44], data[45], data[46],
        ]),
    };

    Ok((header, &data[HEADER_SIZE..end]))
}

/// Read exactly one frame from a reader.
pub async fn read_frame<R: tokio::io::AsyncReadExt + Unpin>(
    r: &mut R,
) -> Result<(Header, Vec<u8>), FrameError> {
    let mut hdr_buf = [0u8; HEADER_SIZE];
    r.read_exact(&mut hdr_buf).await?;

    let version = hdr_buf[0];
    if version != PROTO_VERSION {
        return Err(FrameError::BadVersion { got: version });
    }

    let payload_len = u32::from_be_bytes([hdr_buf[3], hdr_buf[4], hdr_buf[5], hdr_buf[6]]);
    if payload_len > MAX_PAYLOAD_LEN {
        return Err(FrameError::PayloadTooLarge { len: payload_len });
    }

    let mut msg_id = [0u8; 16];
    msg_id.copy_from_slice(&hdr_buf[7..23]);
    let mut conversation_id = [0u8; 16];
    conversation_id.copy_from_slice(&hdr_buf[23..39]);

    let header = Header {
        version,
        frame_type: hdr_buf[1],
        flags: hdr_buf[2],
        payload_len,
        msg_id,
        conversation_id,
        seq: u64::from_be_bytes([
            hdr_buf[39], hdr_buf[40], hdr_buf[41], hdr_buf[42], hdr_buf[43], hdr_buf[44],
            hdr_buf[45], hdr_buf[46],
        ]),
    };

    let mut payload = vec![0u8; payload_len as usize];
    if payload_len > 0 {
        r.read_exact(&mut payload).await?;
    }

    Ok((header, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_roundtrip() {
        let header = Header {
            frame_type: TYPE_SEND_MESSAGE,
            flags: FLAG_COMPRESSED,
            msg_id: [1; 16],
            conversation_id: [2; 16],
            seq: 42,
            ..Default::default()
        };
        let payload = b"hello world";

        let encoded = encode(header, payload).unwrap();
        assert_eq!(encoded.len(), HEADER_SIZE + payload.len());

        let (decoded, decoded_payload) = decode(&encoded).unwrap();
        assert_eq!(decoded.version, PROTO_VERSION);
        assert_eq!(decoded.frame_type, TYPE_SEND_MESSAGE);
        assert!(decoded.is_compressed());
        assert!(!decoded.is_encrypted());
        assert_eq!(decoded.msg_id, [1; 16]);
        assert_eq!(decoded.conversation_id, [2; 16]);
        assert_eq!(decoded.seq, 42);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn test_payload_too_large() {
        let header = Header::default();
        let big = vec![0u8; MAX_PAYLOAD_LEN as usize + 1];
        assert!(encode(header, &big).is_err());
    }

    #[test]
    fn test_short_read() {
        assert!(decode(&[0u8; 10]).is_err());
    }

    #[test]
    fn test_bad_version() {
        let mut data = vec![0u8; HEADER_SIZE];
        data[0] = 99; // bad version
        assert!(decode(&data).is_err());
    }

    #[test]
    fn test_all_frame_types() {
        for ft in [
            TYPE_CONNECT,
            TYPE_AUTH_OK,
            TYPE_AUTH_FAIL,
            TYPE_JOIN_CONVERSATION,
            TYPE_LEAVE_CONVERSATION,
            TYPE_SEND_MESSAGE,
            TYPE_MESSAGE_DELIVERY,
            TYPE_ACK,
            TYPE_PRESENCE,
            TYPE_TYPING,
            TYPE_SLOW_DOWN,
            TYPE_REPLAY,
            TYPE_CLOSE,
        ] {
            let h = Header {
                frame_type: ft,
                ..Default::default()
            };
            let encoded = encode(h, b"").unwrap();
            let (decoded, _) = decode(&encoded).unwrap();
            assert_eq!(decoded.frame_type, ft);
        }
    }

    #[test]
    fn test_flags() {
        let h = Header {
            flags: FLAG_COMPRESSED | FLAG_ENCRYPTED | FLAG_EPHEMERAL,
            ..Default::default()
        };
        assert!(h.is_compressed());
        assert!(h.is_encrypted());
        assert!(h.is_ephemeral());

        let h2 = Header::default();
        assert!(!h2.is_compressed());
        assert!(!h2.is_encrypted());
        assert!(!h2.is_ephemeral());
    }
}
