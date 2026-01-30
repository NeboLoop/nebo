package voice

import (
	"bytes"
	"encoding/json"
	"io"
	"mime/multipart"
	"net/http"
	"os"
	"strings"
)

// Available ElevenLabs voices
var elevenLabsVoices = map[string]string{
	"rachel": "21m00Tcm4TlvDq8ikWAM",
	"domi":   "AZnzlk1XvdvUeBnXmlld",
	"bella":  "EXAVITQu4vr4xnSDxMaL",
	"antoni": "ErXwobaYiN019PkySvjV",
	"elli":   "MF3mGyEYCl7XYWbV9V6O",
	"josh":   "TxGEqnHWrfWFTfGW9XjX",
	"arnold": "VR6AewLTigWG4xSOukaG",
	"adam":   "pNInz6obpgDQGcFmaJgB",
	"sam":    "yoZ06aMxZJJ28mfd3POQ",
}

// TTSRequest represents a text-to-speech request
type TTSRequest struct {
	Text  string  `json:"text"`
	Voice string  `json:"voice"`
	Speed float64 `json:"speed"`
}

// TTSHandler handles text-to-speech requests using ElevenLabs
var TTSHandler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	apiKey := os.Getenv("ELEVENLABS_API_KEY")
	if apiKey == "" {
		http.Error(w, `{"error":"ELEVENLABS_API_KEY not configured"}`, http.StatusServiceUnavailable)
		return
	}

	var req TTSRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, `{"error":"Invalid request body"}`, http.StatusBadRequest)
		return
	}

	if req.Text == "" {
		http.Error(w, `{"error":"text is required"}`, http.StatusBadRequest)
		return
	}

	// Resolve voice ID
	voiceID := elevenLabsVoices["rachel"] // default
	if req.Voice != "" {
		voiceLower := strings.ToLower(req.Voice)
		if id, ok := elevenLabsVoices[voiceLower]; ok {
			voiceID = id
		} else {
			// Assume it's a direct voice ID
			voiceID = req.Voice
		}
	}

	speed := req.Speed
	if speed == 0 {
		speed = 1.0
	}

	// Build ElevenLabs request
	requestBody := map[string]any{
		"text":     req.Text,
		"model_id": "eleven_turbo_v2_5",
		"voice_settings": map[string]any{
			"stability":        0.5,
			"similarity_boost": 0.75,
			"speed":            speed,
		},
	}

	jsonBody, _ := json.Marshal(requestBody)
	apiReq, err := http.NewRequest("POST",
		"https://api.elevenlabs.io/v1/text-to-speech/"+voiceID,
		bytes.NewReader(jsonBody))
	if err != nil {
		http.Error(w, `{"error":"Failed to create request"}`, http.StatusInternalServerError)
		return
	}

	apiReq.Header.Set("Content-Type", "application/json")
	apiReq.Header.Set("xi-api-key", apiKey)
	apiReq.Header.Set("Accept", "audio/mpeg")

	client := &http.Client{}
	resp, err := client.Do(apiReq)
	if err != nil {
		http.Error(w, `{"error":"ElevenLabs API request failed"}`, http.StatusBadGateway)
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		body, _ := io.ReadAll(resp.Body)
		http.Error(w, string(body), resp.StatusCode)
		return
	}

	// Stream audio back to client
	w.Header().Set("Content-Type", "audio/mpeg")
	w.Header().Set("Cache-Control", "no-cache")
	io.Copy(w, resp.Body)
})

// VoicesHandler returns available TTS voices
var VoicesHandler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	voices := make([]map[string]string, 0, len(elevenLabsVoices))
	for name, id := range elevenLabsVoices {
		voices = append(voices, map[string]string{
			"name": name,
			"id":   id,
		})
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]any{
		"voices": voices,
	})
})

// TranscribeHandler handles voice transcription requests
var TranscribeHandler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	apiKey := os.Getenv("OPENAI_API_KEY")
	if apiKey == "" {
		http.Error(w, "OPENAI_API_KEY not configured", http.StatusServiceUnavailable)
		return
	}

	// Parse multipart form (max 25MB for audio)
	if err := r.ParseMultipartForm(25 << 20); err != nil {
		http.Error(w, "Failed to parse form: "+err.Error(), http.StatusBadRequest)
		return
	}

	file, header, err := r.FormFile("audio")
	if err != nil {
		http.Error(w, "No audio file provided", http.StatusBadRequest)
		return
	}
	defer file.Close()

	// Read audio data
	audioData, err := io.ReadAll(file)
	if err != nil {
		http.Error(w, "Failed to read audio", http.StatusInternalServerError)
		return
	}

	// Create multipart form for OpenAI
	var buf bytes.Buffer
	writer := multipart.NewWriter(&buf)

	part, err := writer.CreateFormFile("file", header.Filename)
	if err != nil {
		http.Error(w, "Failed to create form", http.StatusInternalServerError)
		return
	}
	if _, err := part.Write(audioData); err != nil {
		http.Error(w, "Failed to write audio", http.StatusInternalServerError)
		return
	}

	if err := writer.WriteField("model", "whisper-1"); err != nil {
		http.Error(w, "Failed to add model field", http.StatusInternalServerError)
		return
	}
	writer.Close()

	// Send to OpenAI
	req, err := http.NewRequest("POST", "https://api.openai.com/v1/audio/transcriptions", &buf)
	if err != nil {
		http.Error(w, "Failed to create request", http.StatusInternalServerError)
		return
	}
	req.Header.Set("Authorization", "Bearer "+apiKey)
	req.Header.Set("Content-Type", writer.FormDataContentType())

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		http.Error(w, "Failed to call OpenAI API", http.StatusBadGateway)
		return
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		http.Error(w, "Failed to read API response", http.StatusInternalServerError)
		return
	}

	if resp.StatusCode != http.StatusOK {
		http.Error(w, "OpenAI API error: "+string(body), resp.StatusCode)
		return
	}

	// Return the transcription result
	w.Header().Set("Content-Type", "application/json")
	w.Write(body)
})
