// Package sdk implements the NeboLoop Bot SDK — a thin client for the NeboLoop
// comms protocol. WebSocket transport, 47-byte binary frame header, JSON payloads.
// Target: under 800 LOC total across the package.
package sdk

import (
	"encoding/binary"
	"fmt"
	"io"
	"math/rand/v2"
	"sync"
	"time"

	"github.com/klauspost/compress/zstd"
	"github.com/oklog/ulid/v2"
)

// HeaderSize is the fixed binary frame header size in bytes.
const HeaderSize = 47

// ProtoVersion is the current protocol version.
const ProtoVersion = 1

// Frame type constants.
const (
	FrameConnect           uint8 = 0x01
	FrameAuthOK            uint8 = 0x02
	FrameAuthFail          uint8 = 0x03
	FrameJoinConversation  uint8 = 0x04
	FrameLeaveConversation uint8 = 0x05
	FrameSendMessage       uint8 = 0x06
	FrameMessageDelivery   uint8 = 0x07
	FrameAck               uint8 = 0x08
	FramePresence          uint8 = 0x09
	FrameTyping            uint8 = 0x0A
	FrameSlowDown          uint8 = 0x0B
	FrameReplay            uint8 = 0x0C
	FrameClose             uint8 = 0x0D
)

// Flag bits (PRD §6.2).
const (
	FlagCompressed uint8 = 1 << 0
	FlagEncrypted  uint8 = 1 << 1
	FlagEphemeral  uint8 = 1 << 2
)

// MaxPayloadSize is the hard reject cap (PRD §8.1).
const MaxPayloadSize = 32 * 1024

// CompressionThreshold — only compress payloads above this size (PRD §6.2).
const CompressionThreshold = 1024

// Frame is a single protocol frame: 47-byte header + variable payload.
type Frame struct {
	ProtoVersion   uint8
	FrameType      uint8
	Flags          uint8
	ConversationID [16]byte // UUID
	Seq            uint64
	MessageID      [16]byte // ULID
	Payload        []byte
}

// EncodeFrame serializes a frame to bytes. Compresses payload if >1KB.
func EncodeFrame(f *Frame) ([]byte, error) {
	// Reject oversized payloads before compression
	if len(f.Payload) > MaxPayloadSize {
		return nil, fmt.Errorf("payload exceeds max size: %d > %d", len(f.Payload), MaxPayloadSize)
	}

	payload := f.Payload
	flags := f.Flags

	// Compress if above threshold and not already compressed
	if len(payload) > CompressionThreshold && flags&FlagCompressed == 0 {
		compressed, err := compressZstd(payload)
		if err == nil && len(compressed) < len(payload) {
			payload = compressed
			flags |= FlagCompressed
		}
	}

	buf := make([]byte, HeaderSize+len(payload))

	// Header layout: version(1) | type(1) | flags(1) | payload_len(4) | msg_id(16) | conv_id(16) | seq(8)
	buf[0] = f.ProtoVersion
	buf[1] = f.FrameType
	buf[2] = flags
	binary.BigEndian.PutUint32(buf[3:7], uint32(len(payload)))
	copy(buf[7:23], f.MessageID[:])
	copy(buf[23:39], f.ConversationID[:])
	binary.BigEndian.PutUint64(buf[39:47], f.Seq)

	// Payload
	copy(buf[47:], payload)

	return buf, nil
}

// DecodeFrame reads a frame from an io.Reader.
func DecodeFrame(r io.Reader) (*Frame, error) {
	header := make([]byte, HeaderSize)
	if _, err := io.ReadFull(r, header); err != nil {
		return nil, fmt.Errorf("read header: %w", err)
	}

	f := &Frame{
		ProtoVersion: header[0],
		FrameType:    header[1],
		Flags:        header[2],
	}

	if f.ProtoVersion != ProtoVersion {
		return nil, fmt.Errorf("unsupported proto version: %d", f.ProtoVersion)
	}

	payloadLen := binary.BigEndian.Uint32(header[3:7])
	copy(f.MessageID[:], header[7:23])
	copy(f.ConversationID[:], header[23:39])
	f.Seq = binary.BigEndian.Uint64(header[39:47])

	if payloadLen > MaxPayloadSize {
		return nil, fmt.Errorf("payload too large: %d > %d", payloadLen, MaxPayloadSize)
	}

	if payloadLen > 0 {
		f.Payload = make([]byte, payloadLen)
		if _, err := io.ReadFull(r, f.Payload); err != nil {
			return nil, fmt.Errorf("read payload: %w", err)
		}

		// Decompress if flagged
		if f.Flags&FlagCompressed != 0 {
			decompressed, err := decompressZstd(f.Payload)
			if err != nil {
				return nil, fmt.Errorf("decompress: %w", err)
			}
			f.Payload = decompressed
			f.Flags &^= FlagCompressed // Clear flag after decompression
		}
	}

	return f, nil
}

// DecodeFrameFromBytes decodes a frame from a byte slice.
func DecodeFrameFromBytes(data []byte) (*Frame, error) {
	if len(data) < HeaderSize {
		return nil, fmt.Errorf("data too short: %d < %d", len(data), HeaderSize)
	}

	f := &Frame{
		ProtoVersion: data[0],
		FrameType:    data[1],
		Flags:        data[2],
	}

	if f.ProtoVersion != ProtoVersion {
		return nil, fmt.Errorf("unsupported proto version: %d", f.ProtoVersion)
	}

	payloadLen := binary.BigEndian.Uint32(data[3:7])
	copy(f.MessageID[:], data[7:23])
	copy(f.ConversationID[:], data[23:39])
	f.Seq = binary.BigEndian.Uint64(data[39:47])

	if payloadLen > MaxPayloadSize {
		return nil, fmt.Errorf("payload too large: %d > %d", payloadLen, MaxPayloadSize)
	}

	expectedLen := HeaderSize + int(payloadLen)
	if len(data) < expectedLen {
		return nil, fmt.Errorf("data truncated: %d < %d", len(data), expectedLen)
	}

	if payloadLen > 0 {
		f.Payload = make([]byte, payloadLen)
		copy(f.Payload, data[HeaderSize:expectedLen])

		if f.Flags&FlagCompressed != 0 {
			decompressed, err := decompressZstd(f.Payload)
			if err != nil {
				return nil, fmt.Errorf("decompress: %w", err)
			}
			f.Payload = decompressed
			f.Flags &^= FlagCompressed
		}
	}

	return f, nil
}

// NewMessageID generates a new ULID for use as a message_id.
func NewMessageID() [16]byte {
	id := ulid.Make()
	var out [16]byte
	copy(out[:], id[:])
	return out
}

// --- zstd compression ---

var (
	zstdEncoder *zstd.Encoder
	zstdDecoder *zstd.Decoder
	zstdOnce    sync.Once
)

func initZstd() {
	zstdOnce.Do(func() {
		zstdEncoder, _ = zstd.NewWriter(nil, zstd.WithEncoderLevel(zstd.SpeedFastest))
		zstdDecoder, _ = zstd.NewReader(nil)
	})
}

func compressZstd(data []byte) ([]byte, error) {
	initZstd()
	return zstdEncoder.EncodeAll(data, nil), nil
}

func decompressZstd(data []byte) ([]byte, error) {
	initZstd()
	return zstdDecoder.DecodeAll(data, nil)
}

// --- UUID helpers ---

// NewConversationID generates a random UUID v4 for conversation_id.
func NewConversationID() [16]byte {
	var id [16]byte
	r := rand.New(rand.NewPCG(uint64(time.Now().UnixNano()), uint64(time.Now().UnixNano()>>1)))
	for i := range id {
		id[i] = byte(r.IntN(256))
	}
	id[6] = (id[6] & 0x0f) | 0x40 // Version 4
	id[8] = (id[8] & 0x3f) | 0x80 // Variant 10
	return id
}
