package voice

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"sync"
)

// CommsVoiceMessage is the wire format for voice frames over NeboLoop comms.
// Matches the protocol agreed between NeboLoop mobile and server teams.
type CommsVoiceMessage struct {
	Type       string `json:"type"`                    // "audio", "voice_start", "voice_end", "interrupt", "config"
	Data       string `json:"data,omitempty"`           // base64-encoded PCM for audio frames
	SampleRate int    `json:"sample_rate,omitempty"`    // e.g. 16000
	Channels   int    `json:"channels,omitempty"`       // e.g. 1
	Encoding   string `json:"encoding,omitempty"`       // e.g. "pcm_s16le"
	Text       string `json:"text,omitempty"`           // transcript text
	Final      bool   `json:"final,omitempty"`          // final transcript
	Speaking   bool   `json:"speaking,omitempty"`       // VAD state
	State      string `json:"state,omitempty"`          // voice state
	Voice      string `json:"voice,omitempty"`          // TTS voice name
	Error      string `json:"error,omitempty"`          // error message
}

// CommsTransportSendFunc sends a voice message back to the phone via NeboLoop comms.
// The implementation wraps the message in the comms envelope and sends via the SDK.
type CommsTransportSendFunc func(msg CommsVoiceMessage) error

// CommsTransport implements VoiceTransport over NeboLoop's comms WebSocket relay.
// Audio arrives as base64-encoded PCM in JSON text frames from the phone,
// relayed through NeboLoop's comms gateway.
type CommsTransport struct {
	sendFunc CommsTransportSendFunc
	inbound  chan CommsVoiceMessage // fed by the comms plugin's voice stream handler
	closeMu  sync.Mutex
	closed   bool
}

// NewCommsTransport creates a transport backed by NeboLoop comms.
// sendFunc is called to send voice messages back to the phone.
// The returned transport's Feed() method must be called by the comms plugin
// when voice stream messages arrive.
func NewCommsTransport(sendFunc CommsTransportSendFunc) *CommsTransport {
	return &CommsTransport{
		sendFunc: sendFunc,
		inbound:  make(chan CommsVoiceMessage, 100),
	}
}

// Feed pushes an inbound voice message from the comms plugin into the transport.
// Called by the comms plugin's voice stream handler.
func (t *CommsTransport) Feed(msg CommsVoiceMessage) {
	t.closeMu.Lock()
	if t.closed {
		t.closeMu.Unlock()
		return
	}
	t.closeMu.Unlock()

	select {
	case t.inbound <- msg:
	default:
		// Drop if backed up
	}
}

// ReadPump reads voice messages from the comms inbound channel.
// Audio frames are base64-decoded and pushed to audioCh.
// Control messages are converted to JSON and passed to onControl.
func (t *CommsTransport) ReadPump(ctx context.Context, cancel context.CancelFunc, audioCh chan<- []byte, onControl func([]byte)) {
	defer cancel()

	for {
		select {
		case <-ctx.Done():
			return
		case msg, ok := <-t.inbound:
			if !ok {
				return
			}

			switch msg.Type {
			case "audio":
				// Decode base64 PCM and push to audio channel
				pcm, err := base64.StdEncoding.DecodeString(msg.Data)
				if err != nil {
					fmt.Printf("[voice-comms] base64 decode error: %v\n", err)
					continue
				}
				select {
				case audioCh <- pcm:
				default:
					// Drop if pipeline is backed up
				}

			default:
				// Control message — convert to ControlMessage JSON for handleControl
				ctrl := ControlMessage{
					Type:  msg.Type,
					Voice: msg.Voice,
				}
				data, err := json.Marshal(ctrl)
				if err != nil {
					continue
				}
				onControl(data)
			}
		}
	}
}

// WritePump reads audio from audioOutCh, base64-encodes it, and sends via comms.
func (t *CommsTransport) WritePump(ctx context.Context, audioOutCh <-chan []byte) {
	for {
		select {
		case audio, ok := <-audioOutCh:
			if !ok {
				return
			}
			encoded := base64.StdEncoding.EncodeToString(audio)
			_ = t.sendFunc(CommsVoiceMessage{
				Type:       "audio",
				Data:       encoded,
				SampleRate: 16000,
				Channels:   1,
				Encoding:   "pcm_s16le",
			})

		case <-ctx.Done():
			return
		}
	}
}

// SendControl sends a control message to the phone via comms.
func (t *CommsTransport) SendControl(msg ControlMessage) error {
	return t.sendFunc(CommsVoiceMessage{
		Type:     msg.Type,
		State:    msg.State,
		Text:     msg.Text,
		Speaking: msg.IsSpeech,
		Voice:    msg.Voice,
		Error:    msg.Text, // errors carry text in the Text field
	})
}

// Close cleans up the comms transport.
func (t *CommsTransport) Close() error {
	t.closeMu.Lock()
	defer t.closeMu.Unlock()
	if !t.closed {
		t.closed = true
		close(t.inbound)
	}
	return nil
}
