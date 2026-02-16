package sdk

import (
	"bytes"
	"encoding/binary"
	"strings"
	"testing"
)

func TestHeaderSize(t *testing.T) {
	// version(1) + type(1) + flags(1) + payload_len(4) + msg_id(16) + conv_id(16) + seq(8) = 47
	if HeaderSize != 47 {
		t.Errorf("HeaderSize = %d, want 47", HeaderSize)
	}
}

func TestEncodeDecodeRoundTrip(t *testing.T) {
	convID := NewConversationID()
	msgID := NewMessageID()

	original := &Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameSendMessage,
		Flags:          0,
		ConversationID: convID,
		Seq:            42,
		MessageID:      msgID,
		Payload:        []byte("hello world"),
	}

	encoded, err := EncodeFrame(original)
	if err != nil {
		t.Fatalf("EncodeFrame: %v", err)
	}

	decoded, err := DecodeFrame(bytes.NewReader(encoded))
	if err != nil {
		t.Fatalf("DecodeFrame: %v", err)
	}

	if decoded.ProtoVersion != original.ProtoVersion {
		t.Errorf("ProtoVersion = %d, want %d", decoded.ProtoVersion, original.ProtoVersion)
	}
	if decoded.FrameType != original.FrameType {
		t.Errorf("FrameType = %d, want %d", decoded.FrameType, original.FrameType)
	}
	if decoded.ConversationID != original.ConversationID {
		t.Errorf("ConversationID mismatch")
	}
	if decoded.Seq != original.Seq {
		t.Errorf("Seq = %d, want %d", decoded.Seq, original.Seq)
	}
	if decoded.MessageID != original.MessageID {
		t.Errorf("MessageID mismatch")
	}
	if !bytes.Equal(decoded.Payload, original.Payload) {
		t.Errorf("Payload = %q, want %q", decoded.Payload, original.Payload)
	}
}

func TestEncodeDecodeFromBytes(t *testing.T) {
	original := &Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameAck,
		ConversationID: NewConversationID(),
		MessageID:      NewMessageID(),
		Payload:        []byte("ack-data"),
	}

	encoded, err := EncodeFrame(original)
	if err != nil {
		t.Fatalf("EncodeFrame: %v", err)
	}

	decoded, err := DecodeFrameFromBytes(encoded)
	if err != nil {
		t.Fatalf("DecodeFrameFromBytes: %v", err)
	}

	if !bytes.Equal(decoded.Payload, original.Payload) {
		t.Errorf("Payload mismatch")
	}
}

func TestHeaderLayout(t *testing.T) {
	f := &Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameConnect,
		Flags:          FlagCompressed | FlagEncrypted,
		ConversationID: [16]byte{1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16},
		Seq:            0x0102030405060708,
		MessageID:      [16]byte{16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1},
		Payload:        nil,
	}

	encoded, err := EncodeFrame(f)
	if err != nil {
		t.Fatalf("EncodeFrame: %v", err)
	}

	if len(encoded) != HeaderSize {
		t.Fatalf("len = %d, want %d (no payload)", len(encoded), HeaderSize)
	}

	// Check byte layout: version(1) | type(1) | flags(1) | payload_len(4) | msg_id(16) | conv_id(16) | seq(8)
	if encoded[0] != ProtoVersion {
		t.Errorf("byte 0 (version) = %d", encoded[0])
	}
	if encoded[1] != FrameConnect {
		t.Errorf("byte 1 (type) = %d", encoded[1])
	}
	// Flags: encrypted flag should remain (no payload to compress)
	if encoded[2]&FlagEncrypted == 0 {
		t.Errorf("byte 2 (flags) should have encrypted bit set")
	}

	// payload_len at bytes 3-6 (big-endian)
	payloadLen := binary.BigEndian.Uint32(encoded[3:7])
	if payloadLen != 0 {
		t.Errorf("payload_len = %d, want 0", payloadLen)
	}

	// message_id at bytes 7-22
	for i := 0; i < 16; i++ {
		if encoded[7+i] != byte(16-i) {
			t.Errorf("message_id[%d] = %d, want %d", i, encoded[7+i], 16-i)
		}
	}

	// conversation_id at bytes 23-38
	for i := 0; i < 16; i++ {
		if encoded[23+i] != byte(i+1) {
			t.Errorf("conversation_id[%d] = %d, want %d", i, encoded[23+i], i+1)
		}
	}

	// seq at bytes 39-46 (big-endian)
	seq := binary.BigEndian.Uint64(encoded[39:47])
	if seq != 0x0102030405060708 {
		t.Errorf("seq = 0x%x, want 0x0102030405060708", seq)
	}
}

func TestCompressionAboveThreshold(t *testing.T) {
	// Create a compressible payload above 1KB
	payload := []byte(strings.Repeat("hello world ", 200)) // ~2.4KB

	original := &Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameSendMessage,
		ConversationID: NewConversationID(),
		MessageID:      NewMessageID(),
		Payload:        payload,
	}

	encoded, err := EncodeFrame(original)
	if err != nil {
		t.Fatalf("EncodeFrame: %v", err)
	}

	// Compressed should be smaller than uncompressed
	if len(encoded) >= HeaderSize+len(payload) {
		t.Errorf("encoded size %d should be less than uncompressed %d", len(encoded), HeaderSize+len(payload))
	}

	// Compressed flag should be set in the encoded bytes
	if encoded[2]&FlagCompressed == 0 {
		t.Error("compressed flag should be set in encoded frame")
	}

	// Decode should decompress automatically
	decoded, err := DecodeFrame(bytes.NewReader(encoded))
	if err != nil {
		t.Fatalf("DecodeFrame: %v", err)
	}

	if !bytes.Equal(decoded.Payload, payload) {
		t.Errorf("payload round-trip failed after compression")
	}

	// Decoded frame should have compression flag cleared
	if decoded.Flags&FlagCompressed != 0 {
		t.Error("compression flag should be cleared after decode")
	}
}

func TestNoCompressionBelowThreshold(t *testing.T) {
	payload := []byte("short")

	original := &Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameSendMessage,
		ConversationID: NewConversationID(),
		MessageID:      NewMessageID(),
		Payload:        payload,
	}

	encoded, err := EncodeFrame(original)
	if err != nil {
		t.Fatalf("EncodeFrame: %v", err)
	}

	// Should not be compressed
	if encoded[2]&FlagCompressed != 0 {
		t.Error("small payload should not be compressed")
	}

	if len(encoded) != HeaderSize+len(payload) {
		t.Errorf("encoded size = %d, want %d", len(encoded), HeaderSize+len(payload))
	}
}

func TestPayloadCapEnforcement(t *testing.T) {
	payload := make([]byte, MaxPayloadSize+1)

	f := &Frame{
		ProtoVersion:   ProtoVersion,
		FrameType:      FrameSendMessage,
		ConversationID: NewConversationID(),
		MessageID:      NewMessageID(),
		Payload:        payload,
	}

	_, err := EncodeFrame(f)
	if err == nil {
		t.Error("should reject payload exceeding MaxPayloadSize")
	}
}

func TestULIDUniqueness(t *testing.T) {
	seen := make(map[[16]byte]bool)
	for range 1000 {
		id := NewMessageID()
		if seen[id] {
			t.Fatal("ULID collision detected")
		}
		seen[id] = true
	}
}

func TestUnsupportedProtoVersion(t *testing.T) {
	data := make([]byte, HeaderSize)
	data[0] = 99 // Bad version

	_, err := DecodeFrameFromBytes(data)
	if err == nil {
		t.Error("should reject unsupported proto version")
	}
}

func TestAllFrameTypes(t *testing.T) {
	types := []uint8{
		FrameConnect, FrameAuthOK, FrameAuthFail,
		FrameJoinConversation, FrameLeaveConversation,
		FrameSendMessage, FrameMessageDelivery,
		FrameAck, FramePresence, FrameTyping,
		FrameSlowDown, FrameReplay, FrameClose,
	}

	for _, ft := range types {
		f := &Frame{
			ProtoVersion:   ProtoVersion,
			FrameType:      ft,
			ConversationID: NewConversationID(),
			MessageID:      NewMessageID(),
			Payload:        []byte("test"),
		}

		encoded, err := EncodeFrame(f)
		if err != nil {
			t.Fatalf("EncodeFrame(type=%d): %v", ft, err)
		}

		decoded, err := DecodeFrameFromBytes(encoded)
		if err != nil {
			t.Fatalf("DecodeFrameFromBytes(type=%d): %v", ft, err)
		}

		if decoded.FrameType != ft {
			t.Errorf("FrameType = %d, want %d", decoded.FrameType, ft)
		}
	}
}

func TestConversationIDIsUUIDv4(t *testing.T) {
	id := NewConversationID()

	// Version 4: byte 6 high nibble should be 0x4
	if id[6]>>4 != 4 {
		t.Errorf("UUID version = %d, want 4", id[6]>>4)
	}

	// Variant: byte 8 high 2 bits should be 10
	if id[8]>>6 != 2 {
		t.Errorf("UUID variant = %d, want 2", id[8]>>6)
	}
}
