package voice

import (
	"bytes"
	"io"
	"mime/multipart"
	"net/http"
	"os"
)

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
