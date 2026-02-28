package voice

import (
	"context"
	"encoding/json"
	"fmt"
	"sync"
	"time"

	"github.com/gorilla/websocket"
)

// wsTransport implements VoiceTransport over a direct WebSocket connection.
// Used when the browser connects directly to Nebo's /ws/voice endpoint.
type wsTransport struct {
	conn    *websocket.Conn
	writeMu sync.Mutex // protects concurrent writes
}

func newWSTransport(conn *websocket.Conn) *wsTransport {
	return &wsTransport{conn: conn}
}

// ReadPump reads audio and control messages from the WebSocket.
// Binary frames are pushed to audioCh. Text frames are passed to onControl.
// Blocks until ctx done, connection closed, or error. Calls cancel on exit.
func (t *wsTransport) ReadPump(ctx context.Context, cancel context.CancelFunc, audioCh chan<- []byte, onControl func([]byte)) {
	defer cancel()

	t.conn.SetReadDeadline(time.Now().Add(voicePongWait))
	t.conn.SetPongHandler(func(string) error {
		t.conn.SetReadDeadline(time.Now().Add(voicePongWait))
		return nil
	})

	for {
		msgType, data, err := t.conn.ReadMessage()
		if err != nil {
			if websocket.IsUnexpectedCloseError(err, websocket.CloseGoingAway, websocket.CloseNormalClosure) {
				fmt.Printf("[voice-ws] Read error: %v\n", err)
			}
			return
		}

		switch msgType {
		case websocket.BinaryMessage:
			select {
			case audioCh <- data:
			default:
				// Drop frame if pipeline is backed up
			}
		case websocket.TextMessage:
			onControl(data)
		}

		select {
		case <-ctx.Done():
			return
		default:
		}
	}
}

// WritePump sends audio from audioOutCh to the WebSocket as binary frames.
// Also sends periodic pings to keep the connection alive.
// Blocks until ctx done or audioOutCh closed.
func (t *wsTransport) WritePump(ctx context.Context, audioOutCh <-chan []byte) {
	ticker := time.NewTicker(voicePingPeriod)
	defer ticker.Stop()

	for {
		select {
		case audio, ok := <-audioOutCh:
			if !ok {
				return
			}
			t.writeMu.Lock()
			t.conn.SetWriteDeadline(time.Now().Add(voiceWriteWait))
			err := t.conn.WriteMessage(websocket.BinaryMessage, audio)
			t.writeMu.Unlock()
			if err != nil {
				return
			}

		case <-ticker.C:
			t.writeMu.Lock()
			t.conn.SetWriteDeadline(time.Now().Add(voiceWriteWait))
			err := t.conn.WriteMessage(websocket.PingMessage, nil)
			t.writeMu.Unlock()
			if err != nil {
				return
			}

		case <-ctx.Done():
			return
		}
	}
}

// SendControl sends a JSON control message to the client as a WebSocket text frame.
// Thread-safe — can be called from any goroutine.
func (t *wsTransport) SendControl(msg ControlMessage) error {
	data, err := json.Marshal(msg)
	if err != nil {
		return err
	}
	t.writeMu.Lock()
	defer t.writeMu.Unlock()
	t.conn.SetWriteDeadline(time.Now().Add(voiceWriteWait))
	return t.conn.WriteMessage(websocket.TextMessage, data)
}

// Close closes the underlying WebSocket connection.
func (t *wsTransport) Close() error {
	return t.conn.Close()
}
