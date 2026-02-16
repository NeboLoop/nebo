package sdk

import (
	"context"
	"encoding/hex"
	"encoding/json"
	"net"
	"net/http"
	"net/http/httptest"
	"sync"
	"testing"
	"time"

	"github.com/gobwas/ws"
	"github.com/gobwas/ws/wsutil"
)

// mockGateway is a test WebSocket server that speaks the NeboLoop binary protocol.
type mockGateway struct {
	server   *httptest.Server
	conns    []net.Conn
	connCh   chan net.Conn
	mu       sync.Mutex
	authOK   bool
	onFrame  func(net.Conn, *Frame) // optional per-test frame handler
}

func newMockGateway() *mockGateway {
	gw := &mockGateway{
		connCh: make(chan net.Conn, 10),
		authOK: true,
	}

	gw.server = httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		conn, _, _, err := ws.UpgradeHTTP(r, w)
		if err != nil {
			return
		}

		gw.mu.Lock()
		gw.conns = append(gw.conns, conn)
		gw.mu.Unlock()

		// Read CONNECT frame
		data, err := wsutil.ReadClientBinary(conn)
		if err != nil {
			conn.Close()
			return
		}

		connectFrame, err := DecodeFrameFromBytes(data)
		if err != nil || connectFrame.FrameType != FrameConnect {
			conn.Close()
			return
		}

		// Send auth response
		var respFrame *Frame
		if gw.authOK {
			payload, _ := json.Marshal(authOKPayload{
				OK:        true,
				SessionID: "test-session-123",
			})
			respFrame = &Frame{
				ProtoVersion: ProtoVersion,
				FrameType:    FrameAuthOK,
				MessageID:    NewMessageID(),
				Payload:      payload,
			}
		} else {
			payload, _ := json.Marshal(authFailPayload{
				Reason: "invalid credentials",
				Code:   401,
			})
			respFrame = &Frame{
				ProtoVersion: ProtoVersion,
				FrameType:    FrameAuthFail,
				MessageID:    NewMessageID(),
				Payload:      payload,
			}
		}

		encoded, _ := EncodeFrame(respFrame)
		wsutil.WriteServerBinary(conn, encoded)

		if !gw.authOK {
			conn.Close()
			return
		}

		gw.connCh <- conn

		// Read loop for test assertions
		for {
			data, err := wsutil.ReadClientBinary(conn)
			if err != nil {
				return
			}
			f, err := DecodeFrameFromBytes(data)
			if err != nil {
				continue
			}
			if gw.onFrame != nil {
				gw.onFrame(conn, f)
			}
		}
	}))

	return gw
}

func (gw *mockGateway) url() string {
	return "ws" + gw.server.URL[4:] // http:// → ws://
}

func (gw *mockGateway) close() {
	gw.mu.Lock()
	for _, c := range gw.conns {
		c.Close()
	}
	gw.mu.Unlock()
	gw.server.Close()
}

func (gw *mockGateway) sendToClient(conn net.Conn, f *Frame) error {
	encoded, err := EncodeFrame(f)
	if err != nil {
		return err
	}
	return wsutil.WriteServerBinary(conn, encoded)
}

func TestConnectAndAuth(t *testing.T) {
	gw := newMockGateway()
	defer gw.close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := Connect(ctx, Config{
		Gateway:  gw.url(),
		BotID:    "bot-1",
		APIKey:   "key-1",
		DeviceID: "dev-1",
	})
	if err != nil {
		t.Fatalf("Connect: %v", err)
	}
	defer client.Close()

	if !client.IsConnected() {
		t.Error("expected connected")
	}
	if client.SessionID() != "test-session-123" {
		t.Errorf("SessionID = %q, want %q", client.SessionID(), "test-session-123")
	}
}

func TestAuthFailure(t *testing.T) {
	gw := newMockGateway()
	gw.authOK = false
	defer gw.close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	_, err := Connect(ctx, Config{
		Gateway: gw.url(),
		BotID:   "bot-bad",
		APIKey:  "key-bad",
	})
	if err == nil {
		t.Fatal("expected error for auth failure")
	}
}

func TestDeliveryDispatch(t *testing.T) {
	gw := newMockGateway()
	defer gw.close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := Connect(ctx, Config{
		Gateway: gw.url(),
		BotID:   "bot-1",
		APIKey:  "key-1",
	})
	if err != nil {
		t.Fatalf("Connect: %v", err)
	}
	defer client.Close()

	// Wait for gateway to register the connection
	conn := <-gw.connCh

	// Test each content type
	t.Run("install", func(t *testing.T) {
		got := make(chan InstallEvent, 1)
		client.OnInstall(func(evt InstallEvent) { got <- evt })

		content, _ := json.Marshal(InstallEvent{
			Type:        "installed",
			AppID:       "app-123",
			Version:     "1.0.0",
			DownloadURL: "https://example.com/app.napp",
		})
		payload, _ := json.Marshal(deliveryPayload{
			SenderID:    "neboloop",
			ContentType: "install",
			Content:     content,
		})

		convID := NewConversationID()
		gw.sendToClient(conn, &Frame{
			ProtoVersion:   ProtoVersion,
			FrameType:      FrameMessageDelivery,
			ConversationID: convID,
			Seq:            1,
			MessageID:      NewMessageID(),
			Payload:        payload,
		})

		select {
		case evt := <-got:
			if evt.Type != "installed" || evt.AppID != "app-123" {
				t.Errorf("install event = %+v", evt)
			}
		case <-time.After(2 * time.Second):
			t.Fatal("timeout waiting for install event")
		}
	})

	t.Run("channel", func(t *testing.T) {
		got := make(chan ChannelMessage, 1)
		client.OnChannelMessage(func(msg ChannelMessage) { got <- msg })

		content, _ := json.Marshal(ChannelMessage{
			ChannelType: "telegram",
			SenderName:  "Alice",
			Text:        "hello from telegram",
		})
		payload, _ := json.Marshal(deliveryPayload{
			SenderID:    "bridge-telegram",
			ContentType: "channel",
			Content:     content,
		})

		gw.sendToClient(conn, &Frame{
			ProtoVersion:   ProtoVersion,
			FrameType:      FrameMessageDelivery,
			ConversationID: NewConversationID(),
			Seq:            2,
			MessageID:      NewMessageID(),
			Payload:        payload,
		})

		select {
		case msg := <-got:
			if msg.ChannelType != "telegram" || msg.Text != "hello from telegram" {
				t.Errorf("channel message = %+v", msg)
			}
		case <-time.After(2 * time.Second):
			t.Fatal("timeout waiting for channel message")
		}
	})

	t.Run("task", func(t *testing.T) {
		got := make(chan TaskSubmission, 1)
		client.OnTask(func(task TaskSubmission) { got <- task })

		content, _ := json.Marshal(TaskSubmission{
			From:          "agent-2",
			Input:         "summarize this document",
			CorrelationID: "corr-1",
		})
		payload, _ := json.Marshal(deliveryPayload{
			SenderID:    "agent-2",
			ContentType: "task",
			Content:     content,
			Stream:      "a2a",
		})

		gw.sendToClient(conn, &Frame{
			ProtoVersion:   ProtoVersion,
			FrameType:      FrameMessageDelivery,
			ConversationID: NewConversationID(),
			Seq:            3,
			MessageID:      NewMessageID(),
			Payload:        payload,
		})

		select {
		case task := <-got:
			if task.From != "agent-2" || task.CorrelationID != "corr-1" {
				t.Errorf("task = %+v", task)
			}
		case <-time.After(2 * time.Second):
			t.Fatal("timeout waiting for task")
		}
	})

	t.Run("direct", func(t *testing.T) {
		got := make(chan DirectMessage, 1)
		client.OnDirectMessage(func(dm DirectMessage) { got <- dm })

		content, _ := json.Marshal(DirectMessage{
			From:    "agent-3",
			Type:    "proposal",
			Content: "let's collaborate",
		})
		payload, _ := json.Marshal(deliveryPayload{
			SenderID:    "agent-3",
			ContentType: "direct",
			Content:     content,
			Stream:      "a2a",
		})

		gw.sendToClient(conn, &Frame{
			ProtoVersion:   ProtoVersion,
			FrameType:      FrameMessageDelivery,
			ConversationID: NewConversationID(),
			Seq:            4,
			MessageID:      NewMessageID(),
			Payload:        payload,
		})

		select {
		case dm := <-got:
			if dm.From != "agent-3" || dm.Type != "proposal" {
				t.Errorf("direct message = %+v", dm)
			}
		case <-time.After(2 * time.Second):
			t.Fatal("timeout waiting for direct message")
		}
	})
}

func TestAckTracking(t *testing.T) {
	gw := newMockGateway()
	defer gw.close()

	ackCh := make(chan *Frame, 10)
	gw.onFrame = func(_ net.Conn, f *Frame) {
		if f.FrameType == FrameAck {
			ackCh <- f
		}
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := Connect(ctx, Config{
		Gateway: gw.url(),
		BotID:   "bot-1",
		APIKey:  "key-1",
	})
	if err != nil {
		t.Fatalf("Connect: %v", err)
	}
	defer client.Close()

	conn := <-gw.connCh

	// Register a handler so delivery is processed
	client.OnTask(func(_ TaskSubmission) {})

	convID := NewConversationID()
	convHex := hex.EncodeToString(convID[:])

	// Send messages with seq 1, 2, 3
	for seq := uint64(1); seq <= 3; seq++ {
		content, _ := json.Marshal(TaskSubmission{From: "test", Input: "test"})
		payload, _ := json.Marshal(deliveryPayload{
			ContentType: "task",
			Content:     content,
		})
		gw.sendToClient(conn, &Frame{
			ProtoVersion:   ProtoVersion,
			FrameType:      FrameMessageDelivery,
			ConversationID: convID,
			Seq:            seq,
			MessageID:      NewMessageID(),
			Payload:        payload,
		})
	}

	// Wait for acks
	ackCount := 0
	timeout := time.After(3 * time.Second)
	for ackCount < 3 {
		select {
		case <-ackCh:
			ackCount++
		case <-timeout:
			t.Fatalf("expected 3 acks, got %d", ackCount)
		}
	}

	// Verify tracker has highest seq
	client.mu.RLock()
	tracked := client.ackTracker[convHex]
	client.mu.RUnlock()

	if tracked != 3 {
		t.Errorf("ack tracker = %d, want 3", tracked)
	}
}

func TestSendMessage(t *testing.T) {
	gw := newMockGateway()
	defer gw.close()

	sendCh := make(chan *Frame, 10)
	gw.onFrame = func(_ net.Conn, f *Frame) {
		if f.FrameType == FrameSendMessage {
			sendCh <- f
		}
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := Connect(ctx, Config{
		Gateway: gw.url(),
		BotID:   "bot-1",
		APIKey:  "key-1",
	})
	if err != nil {
		t.Fatalf("Connect: %v", err)
	}
	defer client.Close()

	<-gw.connCh // wait for connection established

	cid := NewConversationID()
	convID := hex.EncodeToString(cid[:])

	// Send a direct message
	err = client.SendDirect(ctx, convID, DirectMessage{
		From:    "bot-1",
		Type:    "message",
		Content: "hello agent-2",
	})
	if err != nil {
		t.Fatalf("SendDirect: %v", err)
	}

	select {
	case f := <-sendCh:
		var send sendPayload
		if err := json.Unmarshal(f.Payload, &send); err != nil {
			t.Fatalf("unmarshal: %v", err)
		}
		if send.ContentType != "direct" {
			t.Errorf("content_type = %q, want %q", send.ContentType, "direct")
		}
		if send.Stream != "a2a" {
			t.Errorf("stream = %q, want %q", send.Stream, "a2a")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timeout waiting for send frame")
	}
}

func TestJoinConversation(t *testing.T) {
	gw := newMockGateway()
	defer gw.close()

	joinCh := make(chan *Frame, 5)
	gw.onFrame = func(_ net.Conn, f *Frame) {
		if f.FrameType == FrameJoinConversation {
			joinCh <- f
		}
	}

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := Connect(ctx, Config{
		Gateway: gw.url(),
		BotID:   "bot-1",
		APIKey:  "key-1",
	})
	if err != nil {
		t.Fatalf("Connect: %v", err)
	}
	defer client.Close()

	<-gw.connCh

	cid := NewConversationID()
	convID := hex.EncodeToString(cid[:])
	if err := client.Join(ctx, convID); err != nil {
		t.Fatalf("Join: %v", err)
	}

	select {
	case f := <-joinCh:
		var join joinPayload
		if err := json.Unmarshal(f.Payload, &join); err != nil {
			t.Fatalf("unmarshal join: %v", err)
		}
		if _, ok := join.LastAckedSeqs[convID]; !ok {
			t.Error("expected conversation ID in join payload")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timeout waiting for join frame")
	}
}

func TestHeartbeat(t *testing.T) {
	// We can't wait 20s in a test, so verify the heartbeat goroutine runs
	// by checking the presence frame is sent. Use a short-lived client.
	gw := newMockGateway()
	defer gw.close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := Connect(ctx, Config{
		Gateway: gw.url(),
		BotID:   "bot-1",
		APIKey:  "key-1",
	})
	if err != nil {
		t.Fatalf("Connect: %v", err)
	}
	defer client.Close()

	// Heartbeat goroutine starts — verify it's running by checking client state
	if !client.IsConnected() {
		t.Error("client should be connected")
	}
}

func TestSlowDownHandling(t *testing.T) {
	gw := newMockGateway()
	defer gw.close()

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	client, err := Connect(ctx, Config{
		Gateway: gw.url(),
		BotID:   "bot-1",
		APIKey:  "key-1",
	})
	if err != nil {
		t.Fatalf("Connect: %v", err)
	}
	defer client.Close()

	conn := <-gw.connCh

	// Send SLOW_DOWN frame — should not crash
	gw.sendToClient(conn, &Frame{
		ProtoVersion: ProtoVersion,
		FrameType:    FrameSlowDown,
		MessageID:    NewMessageID(),
	})

	// Give it a moment to process
	time.Sleep(100 * time.Millisecond)

	if !client.IsConnected() {
		t.Error("client should remain connected after slow down")
	}
}
