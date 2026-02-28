package voice

import "context"

// VoiceTransport abstracts the I/O layer for a duplex voice connection.
// Two implementations: wsTransport (direct WebSocket) and CommsTransport (NeboLoop relay).
type VoiceTransport interface {
	// ReadPump reads audio and control messages from the client.
	// Audio frames are pushed to audioCh. Control messages are passed to onControl.
	// Blocks until ctx done, connection closed, or error. Calls cancel on exit.
	ReadPump(ctx context.Context, cancel context.CancelFunc, audioCh chan<- []byte, onControl func([]byte))

	// WritePump sends audio from audioOutCh to the client.
	// Blocks until ctx done or audioOutCh closed.
	WritePump(ctx context.Context, audioOutCh <-chan []byte)

	// SendControl sends a JSON control message to the client.
	// Thread-safe — can be called from any goroutine.
	SendControl(msg ControlMessage) error

	// Close cleans up transport resources.
	Close() error
}
