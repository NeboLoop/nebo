package voice

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"sync"
	"time"

	"github.com/gorilla/websocket"
	"github.com/neboloop/nebo/internal/middleware"
)

// NewVoiceConn creates a VoiceConn with the given transport and dependencies.
// Used by both the direct WebSocket handler and the comms relay handler.
func NewVoiceConn(transport VoiceTransport, deps DuplexDeps) *VoiceConn {
	if deps.SampleRate == 0 {
		deps.SampleRate = 16000
	}
	return &VoiceConn{
		transport:  transport,
		deps:       deps,
		audioCh:    make(chan []byte, 100),
		textCh:     make(chan string, 10),
		ttsCh:      make(chan string, 20),
		audioOutCh: make(chan []byte, 200),
		gate:       NewNoiseGate(),
		vad:        NewDefaultVAD(),
		voice:      "rachel",
	}
}

// Serve launches the voice pipeline goroutines and blocks until done.
// Exported so the comms handler can call it after creating a VoiceConn.
func (vc *VoiceConn) Serve(ctx context.Context) {
	ctx, cancel := context.WithCancel(ctx)
	vc.cancel = cancel
	vc.setState(StateListening)
	vc.serve(ctx)
}

// VoiceState represents the current state of the voice connection.
type VoiceState int

const (
	StateIdle         VoiceState = iota // No active voice session
	StateListening                      // Receiving and processing user audio
	StateProcessing                     // ASR→LLM pipeline running
	StateSpeaking                       // TTS audio being sent to client
	StateInterrupting                   // User interrupted during speaking
)

func (s VoiceState) String() string {
	switch s {
	case StateIdle:
		return "idle"
	case StateListening:
		return "listening"
	case StateProcessing:
		return "processing"
	case StateSpeaking:
		return "speaking"
	case StateInterrupting:
		return "interrupting"
	default:
		return "unknown"
	}
}

// ControlMessage is a JSON text frame sent/received alongside binary audio.
type ControlMessage struct {
	Type       string `json:"type"`                  // "state", "transcript", "config", "vad_state", "error"
	State      string `json:"state,omitempty"`        // VoiceState as string
	Text       string `json:"text,omitempty"`         // Transcript or error text
	IsSpeech   bool   `json:"is_speech,omitempty"`    // VAD state
	SampleRate int    `json:"sample_rate,omitempty"`   // Audio sample rate
	Voice      string `json:"voice,omitempty"`         // TTS voice name
}

// DuplexDeps holds the dependencies for the duplex voice handler.
type DuplexDeps struct {
	// RunnerFunc runs a prompt through the agentic loop on LaneMain.
	// Returns a channel of text chunks and an error.
	RunnerFunc func(ctx context.Context, sessionKey, prompt, channel string) (<-chan string, error)

	// SendFrame sends a frame to the agent hub (for broadcasting to web UI).
	SendFrame func(frame map[string]any) error

	// SampleRate is the audio sample rate (default 16000).
	SampleRate int
}

// VoiceConn manages a single full-duplex voice connection.
type VoiceConn struct {
	transport VoiceTransport
	deps      DuplexDeps

	// Pipeline channels
	audioCh    chan []byte   // readPump → asrLoop: raw Int16LE PCM
	textCh     chan string   // asrLoop → llmLoop: transcribed text
	ttsCh      chan string   // llmLoop → ttsLoop: sentences to speak
	audioOutCh chan []byte   // ttsLoop → writePump: PCM audio to send

	// State
	state    VoiceState
	stateMu  sync.RWMutex
	cancel   context.CancelFunc
	gate     *NoiseGate
	vad      VAD
	voice    string // TTS voice name
}

const (
	voiceWriteWait  = 10 * time.Second
	voicePongWait   = 60 * time.Second
	voicePingPeriod = (voicePongWait * 9) / 10
)

var upgrader = websocket.Upgrader{
	ReadBufferSize:  4096,
	WriteBufferSize: 4096,
	CheckOrigin: func(r *http.Request) bool {
		origin := r.Header.Get("Origin")
		return origin == "" || middleware.IsLocalhostOrigin(origin)
	},
}

// DuplexHandler returns an http.HandlerFunc that upgrades to a full-duplex voice WebSocket.
func DuplexHandler(deps DuplexDeps) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			fmt.Printf("[voice-duplex] Upgrade failed: %v\n", err)
			return
		}

		transport := newWSTransport(conn)
		vc := NewVoiceConn(transport, deps)

		fmt.Printf("[voice-duplex] New connection from %s\n", r.RemoteAddr)
		vc.Serve(r.Context())
	}
}

// serve launches the pipeline goroutines and blocks until the connection closes.
func (vc *VoiceConn) serve(ctx context.Context) {
	var wg sync.WaitGroup
	wg.Add(5)

	go func() { defer wg.Done(); vc.transport.ReadPump(ctx, vc.cancel, vc.audioCh, vc.handleControl) }()
	go func() { defer wg.Done(); vc.transport.WritePump(ctx, vc.audioOutCh) }()
	go func() { defer wg.Done(); vc.asrLoop(ctx) }()
	go func() { defer wg.Done(); vc.llmLoop(ctx) }()
	go func() { defer wg.Done(); vc.ttsLoop(ctx) }()

	// Block until context cancelled (connection closed)
	<-ctx.Done()

	// Close channels to unblock goroutines
	close(vc.audioCh)
	close(vc.textCh)
	close(vc.ttsCh)
	close(vc.audioOutCh)

	vc.transport.Close()
	wg.Wait()
	fmt.Println("[voice-duplex] Connection closed")
}

// sendControl sends a JSON control message to the client via the transport.
func (vc *VoiceConn) sendControl(msg ControlMessage) {
	vc.transport.SendControl(msg)
}

// handleControl processes incoming JSON control messages.
func (vc *VoiceConn) handleControl(data []byte) {
	var msg ControlMessage
	if err := json.Unmarshal(data, &msg); err != nil {
		return
	}

	switch msg.Type {
	case "config":
		if msg.Voice != "" {
			vc.voice = msg.Voice
		}
	case "interrupt":
		vc.interrupt()
	}
}

// setState updates the voice state and notifies the client.
func (vc *VoiceConn) setState(state VoiceState) {
	vc.stateMu.Lock()
	vc.state = state
	vc.stateMu.Unlock()

	vc.sendControl(ControlMessage{
		Type:  "state",
		State: state.String(),
	})
}

// getState returns the current voice state.
func (vc *VoiceConn) getState() VoiceState {
	vc.stateMu.RLock()
	defer vc.stateMu.RUnlock()
	return vc.state
}

// interrupt handles user speech during StateSpeaking — drain queues and switch to listening.
func (vc *VoiceConn) interrupt() {
	if vc.getState() != StateSpeaking {
		return
	}

	vc.setState(StateInterrupting)

	// Drain pending TTS and audio output
	for {
		select {
		case <-vc.ttsCh:
		default:
			goto drainAudio
		}
	}
drainAudio:
	for {
		select {
		case <-vc.audioOutCh:
		default:
			goto done
		}
	}
done:
	vc.vad.Reset()
	vc.setState(StateListening)
}

// WakeWordHandler returns an http.HandlerFunc that listens for "Hey Nebo" over a WebSocket.
// When the wake word is detected, it sends a {"type":"wake"} control message to the client.
// The client then upgrades to the full duplex connection.
func WakeWordHandler() http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		conn, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			fmt.Printf("[voice-wake] Upgrade failed: %v\n", err)
			return
		}
		defer conn.Close()

		ctx, cancel := context.WithCancel(r.Context())
		defer cancel()

		detector := NewWakeWordDetector(func() {
			// Send wake signal to client
			msg, _ := json.Marshal(ControlMessage{Type: "wake"})
			conn.SetWriteDeadline(time.Now().Add(voiceWriteWait))
			conn.WriteMessage(websocket.TextMessage, msg)
		})

		conn.SetReadDeadline(time.Now().Add(voicePongWait))
		conn.SetPongHandler(func(string) error {
			conn.SetReadDeadline(time.Now().Add(voicePongWait))
			return nil
		})

		// Ping ticker
		go func() {
			ticker := time.NewTicker(voicePingPeriod)
			defer ticker.Stop()
			for {
				select {
				case <-ticker.C:
					conn.SetWriteDeadline(time.Now().Add(voiceWriteWait))
					if err := conn.WriteMessage(websocket.PingMessage, nil); err != nil {
						cancel()
						return
					}
				case <-ctx.Done():
					return
				}
			}
		}()

		fmt.Println("[voice-wake] Wake word listener started")
		for {
			msgType, data, err := conn.ReadMessage()
			if err != nil {
				return
			}

			if msgType == websocket.BinaryMessage {
				pcm := decodePCM(data)
				if detector.Feed(pcm) {
					fmt.Println("[voice-wake] Wake word detected!")
					// Keep connection open — client will disconnect and open full duplex
				}
			}

			select {
			case <-ctx.Done():
				return
			default:
			}
		}
	}
}
