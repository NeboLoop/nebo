package voice

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"
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

// TTSHandler handles text-to-speech requests.
// Uses ElevenLabs if ELEVENLABS_API_KEY is set, otherwise falls back to macOS `say` command.
var TTSHandler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
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

	apiKey := os.Getenv("ELEVENLABS_API_KEY")
	if apiKey != "" {
		if serveElevenLabsTTS(w, req, apiKey) {
			return
		}
		// ElevenLabs failed (quota, network, etc.) — fall through to macOS
	}

	// Fallback: macOS `say` command
	if runtime.GOOS == "darwin" {
		serveMacTTS(w, req)
		return
	}

	http.Error(w, `{"error":"No TTS provider configured. Set ELEVENLABS_API_KEY or use macOS."}`, http.StatusServiceUnavailable)
})

// macTTS uses the macOS `say` command to generate audio, returning raw AIFF bytes.
func macTTS(text, voice string, speed float64) ([]byte, error) {
	tmpFile, err := os.CreateTemp("", "nebo-tts-*.aiff")
	if err != nil {
		return nil, fmt.Errorf("failed to create temp file: %w", err)
	}
	tmpPath := tmpFile.Name()
	tmpFile.Close()
	defer os.Remove(tmpPath)

	// Pick voice — default to Shelley (modern Siri-era voice)
	if voice == "" {
		voice = "Shelley (English (US))"
	}

	// Build rate arg — `say` uses words per minute, default ~175
	args := []string{"-v", voice, "-o", tmpPath}
	if speed > 0 && speed != 1.0 {
		rate := int(175 * speed)
		args = append(args, "-r", fmt.Sprintf("%d", rate))
	}
	args = append(args, text)

	cmd := exec.Command("say", args...)
	if output, err := cmd.CombinedOutput(); err != nil {
		return nil, fmt.Errorf("say command failed: %s", string(output))
	}

	return os.ReadFile(tmpPath)
}

// elevenLabsTTS calls the ElevenLabs API, returning raw MP3 bytes.
// Returns nil, nil if the API call fails (caller should fall back).
func elevenLabsTTS(text, voice string, speed float64, apiKey string) ([]byte, error) {
	// Resolve voice ID
	voiceID := elevenLabsVoices["rachel"] // default
	if voice != "" {
		voiceLower := strings.ToLower(voice)
		if id, ok := elevenLabsVoices[voiceLower]; ok {
			voiceID = id
		} else {
			voiceID = voice
		}
	}

	if speed == 0 {
		speed = 1.0
	}

	requestBody := map[string]any{
		"text":     text,
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
		return nil, nil // silent fallback
	}

	apiReq.Header.Set("Content-Type", "application/json")
	apiReq.Header.Set("xi-api-key", apiKey)
	apiReq.Header.Set("Accept", "audio/mpeg")

	client := &http.Client{}
	resp, err := client.Do(apiReq)
	if err != nil {
		return nil, nil // silent fallback
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		io.ReadAll(resp.Body) // drain body
		return nil, nil       // quota exceeded, auth error → silent fallback
	}

	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, nil
	}
	return data, nil
}

// SynthesizeSpeech generates TTS audio. Tries ElevenLabs first (if API key set),
// then falls back to macOS `say`. Returns audio bytes and content type.
func SynthesizeSpeech(text, voice string, speed float64) ([]byte, string, error) {
	apiKey := os.Getenv("ELEVENLABS_API_KEY")
	if apiKey != "" {
		data, err := elevenLabsTTS(text, voice, speed, apiKey)
		if err == nil && data != nil {
			return data, "audio/mpeg", nil
		}
		// ElevenLabs failed — fall through to macOS
	}

	if runtime.GOOS == "darwin" {
		data, err := macTTS(text, voice, speed)
		if err != nil {
			return nil, "", err
		}
		return data, "audio/aiff", nil
	}

	return nil, "", fmt.Errorf("no TTS provider configured. Set ELEVENLABS_API_KEY or use macOS")
}

// serveMacTTS uses the macOS `say` command to generate audio (HTTP handler wrapper).
func serveMacTTS(w http.ResponseWriter, req TTSRequest) {
	data, err := macTTS(req.Text, req.Voice, req.Speed)
	if err != nil {
		http.Error(w, fmt.Sprintf(`{"error":"%s"}`, err.Error()), http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "audio/aiff")
	w.Header().Set("Cache-Control", "no-cache")
	w.Write(data)
}

// serveElevenLabsTTS uses the ElevenLabs API for high-quality TTS (HTTP handler wrapper).
// Returns true if it successfully served audio, false if caller should fall back.
func serveElevenLabsTTS(w http.ResponseWriter, req TTSRequest, apiKey string) bool {
	data, err := elevenLabsTTS(req.Text, req.Voice, req.Speed, apiKey)
	if err != nil || data == nil {
		return false
	}
	w.Header().Set("Content-Type", "audio/mpeg")
	w.Header().Set("Cache-Control", "no-cache")
	w.Write(data)
	return true
}

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

// transcribeLocal runs whisper-cli on an audio file and returns the transcribed text.
func transcribeLocal(audioPath, modelPath string) (string, error) {
	whisperPath, err := exec.LookPath("whisper-cli")
	if err != nil {
		return "", fmt.Errorf("whisper-cli not found in PATH")
	}

	cmd := exec.Command(whisperPath,
		"--model", modelPath,
		"--file", audioPath,
		"--no-timestamps",
		"--language", "en",
		"--threads", "4",
	)

	output, err := cmd.CombinedOutput()
	if err != nil {
		return "", fmt.Errorf("whisper-cli error: %w\nOutput: %s", err, string(output))
	}

	// Parse stdout — skip whisper log lines, keep only transcription text
	lines := strings.Split(string(output), "\n")
	var textLines []string
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" ||
			strings.HasPrefix(line, "whisper_") ||
			strings.HasPrefix(line, "main:") ||
			strings.HasPrefix(line, "ggml_") ||
			strings.HasPrefix(line, "system_info:") ||
			strings.HasPrefix(line, "output_") {
			continue
		}
		// Strip timestamp brackets like [00:00:00.000 --> 00:00:05.000]
		if idx := strings.Index(line, "]"); idx > 0 && strings.HasPrefix(line, "[") {
			line = strings.TrimSpace(line[idx+1:])
		}
		if line != "" {
			textLines = append(textLines, line)
		}
	}

	return strings.Join(textLines, " "), nil
}

// convertToWav converts audio from webm/ogg (MediaRecorder output) to 16kHz mono WAV
// using ffmpeg. Returns the path to the WAV file.
func convertToWav(inputPath string) (string, error) {
	ffmpegPath, err := exec.LookPath("ffmpeg")
	if err != nil {
		return "", fmt.Errorf("ffmpeg not found in PATH (needed for audio conversion)")
	}

	wavPath := inputPath + ".wav"
	cmd := exec.Command(ffmpegPath,
		"-i", inputPath,
		"-ar", "16000",
		"-ac", "1",
		"-y",
		wavPath,
	)
	if output, err := cmd.CombinedOutput(); err != nil {
		return "", fmt.Errorf("ffmpeg conversion error: %w\nOutput: %s", err, string(output))
	}

	return wavPath, nil
}

// TranscribeHandler handles voice transcription requests.
// Uses local whisper-cli when available, falls back to OpenAI Whisper API.
// Accepts multipart form with "audio" file field.
var TranscribeHandler = http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "Method not allowed", http.StatusMethodNotAllowed)
		return
	}

	// Parse multipart form (max 25MB for audio)
	if err := r.ParseMultipartForm(25 << 20); err != nil {
		http.Error(w, `{"error":"Failed to parse form: `+err.Error()+`"}`, http.StatusBadRequest)
		return
	}

	file, header, err := r.FormFile("audio")
	if err != nil {
		http.Error(w, `{"error":"No audio file provided"}`, http.StatusBadRequest)
		return
	}
	defer file.Close()

	// Read audio data to a temp file
	audioData, err := io.ReadAll(file)
	if err != nil {
		http.Error(w, `{"error":"Failed to read audio"}`, http.StatusInternalServerError)
		return
	}

	// Determine file extension from Content-Type or filename
	ext := filepath.Ext(header.Filename)
	if ext == "" {
		contentType := header.Header.Get("Content-Type")
		switch {
		case strings.Contains(contentType, "webm"):
			ext = ".webm"
		case strings.Contains(contentType, "ogg"):
			ext = ".ogg"
		case strings.Contains(contentType, "wav"):
			ext = ".wav"
		case strings.Contains(contentType, "mp4"), strings.Contains(contentType, "m4a"):
			ext = ".m4a"
		default:
			ext = ".webm" // MediaRecorder default
		}
	}

	// Write to temp file
	tmpFile, err := os.CreateTemp("", fmt.Sprintf("nebo_voice_%d_*%s", time.Now().UnixNano(), ext))
	if err != nil {
		http.Error(w, `{"error":"Failed to create temp file"}`, http.StatusInternalServerError)
		return
	}
	tmpPath := tmpFile.Name()
	defer os.Remove(tmpPath)

	if _, err := tmpFile.Write(audioData); err != nil {
		tmpFile.Close()
		http.Error(w, `{"error":"Failed to write temp file"}`, http.StatusInternalServerError)
		return
	}
	tmpFile.Close()

	// Try local transcription (embedded whisper on desktop, whisper-cli on headless)
	{
		wavPath := tmpPath
		if ext != ".wav" {
			converted, convErr := convertToWav(tmpPath)
			if convErr != nil {
				goto openai
			}
			wavPath = converted
			defer os.Remove(wavPath)
		}

		text, transcribeErr := TranscribeFile(wavPath)
		if transcribeErr == nil {
			text = strings.TrimSpace(text)
			if text != "" && text != "[BLANK_AUDIO]" && text != "(silence)" {
				w.Header().Set("Content-Type", "application/json")
				json.NewEncoder(w).Encode(map[string]string{"text": text})
				return
			}
			// Empty/silence — return empty
			w.Header().Set("Content-Type", "application/json")
			json.NewEncoder(w).Encode(map[string]string{"text": ""})
			return
		}
		// Local transcription failed, fall through to OpenAI
	}

openai:
	// Fallback: OpenAI Whisper API
	apiKey := os.Getenv("OPENAI_API_KEY")
	if apiKey == "" {
		http.Error(w, `{"error":"No transcription backend available. Install whisper-cli or set OPENAI_API_KEY."}`, http.StatusServiceUnavailable)
		return
	}

	// Create multipart form for OpenAI
	var buf bytes.Buffer
	writer := multipart.NewWriter(&buf)

	part, err := writer.CreateFormFile("file", header.Filename)
	if err != nil {
		http.Error(w, `{"error":"Failed to create form"}`, http.StatusInternalServerError)
		return
	}
	if _, err := part.Write(audioData); err != nil {
		http.Error(w, `{"error":"Failed to write audio"}`, http.StatusInternalServerError)
		return
	}

	if err := writer.WriteField("model", "whisper-1"); err != nil {
		http.Error(w, `{"error":"Failed to add model field"}`, http.StatusInternalServerError)
		return
	}
	writer.Close()

	// Send to OpenAI
	apiReq, err := http.NewRequest("POST", "https://api.openai.com/v1/audio/transcriptions", &buf)
	if err != nil {
		http.Error(w, `{"error":"Failed to create request"}`, http.StatusInternalServerError)
		return
	}
	apiReq.Header.Set("Authorization", "Bearer "+apiKey)
	apiReq.Header.Set("Content-Type", writer.FormDataContentType())

	resp, err := http.DefaultClient.Do(apiReq)
	if err != nil {
		http.Error(w, `{"error":"Failed to call OpenAI API"}`, http.StatusBadGateway)
		return
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		http.Error(w, `{"error":"Failed to read API response"}`, http.StatusInternalServerError)
		return
	}

	if resp.StatusCode != http.StatusOK {
		http.Error(w, `{"error":"OpenAI API error: `+string(body)+`"}`, resp.StatusCode)
		return
	}

	// Return the transcription result
	w.Header().Set("Content-Type", "application/json")
	w.Write(body)
})
