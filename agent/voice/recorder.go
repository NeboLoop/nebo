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
	"time"
)

// Recorder handles audio recording and transcription
type Recorder struct {
	apiKey string
}

// NewRecorder creates a new voice recorder
func NewRecorder() *Recorder {
	return &Recorder{
		apiKey: os.Getenv("OPENAI_API_KEY"),
	}
}

// Record records audio from the microphone and returns the transcribed text
func (r *Recorder) Record() (string, error) {
	if r.apiKey == "" {
		return "", fmt.Errorf("OPENAI_API_KEY not set - required for voice transcription")
	}

	// Create temp file for recording
	tempDir := os.TempDir()
	audioFile := filepath.Join(tempDir, fmt.Sprintf("gobot_voice_%d.wav", time.Now().UnixNano()))
	defer os.Remove(audioFile)

	// Record audio using platform-specific command
	fmt.Println("\n🎤 Recording... Press Ctrl+C to stop")
	if err := r.recordAudio(audioFile); err != nil {
		return "", fmt.Errorf("recording failed: %w", err)
	}

	// Check if file was created
	if _, err := os.Stat(audioFile); os.IsNotExist(err) {
		return "", fmt.Errorf("no audio recorded")
	}

	fmt.Println("🔄 Transcribing...")

	// Transcribe with Whisper
	text, err := r.transcribe(audioFile)
	if err != nil {
		return "", fmt.Errorf("transcription failed: %w", err)
	}

	return text, nil
}

// recordAudio records audio using platform-specific tools
func (r *Recorder) recordAudio(outputFile string) error {
	var cmd *exec.Cmd

	switch runtime.GOOS {
	case "darwin":
		// macOS: Use sox (brew install sox) or ffmpeg
		// Try sox first, fallback to ffmpeg
		if _, err := exec.LookPath("sox"); err == nil {
			cmd = exec.Command("sox", "-d", "-r", "16000", "-c", "1", "-b", "16", outputFile)
		} else if _, err := exec.LookPath("ffmpeg"); err == nil {
			cmd = exec.Command("ffmpeg", "-f", "avfoundation", "-i", ":0", "-ar", "16000", "-ac", "1", "-y", outputFile)
		} else {
			return fmt.Errorf("install sox (brew install sox) or ffmpeg for voice recording")
		}

	case "linux":
		// Linux: Use arecord (ALSA) or sox
		if _, err := exec.LookPath("arecord"); err == nil {
			cmd = exec.Command("arecord", "-f", "S16_LE", "-r", "16000", "-c", "1", outputFile)
		} else if _, err := exec.LookPath("sox"); err == nil {
			cmd = exec.Command("sox", "-d", "-r", "16000", "-c", "1", "-b", "16", outputFile)
		} else {
			return fmt.Errorf("install arecord (alsa-utils) or sox for voice recording")
		}

	case "windows":
		// Windows: Use ffmpeg or PowerShell
		if _, err := exec.LookPath("ffmpeg"); err == nil {
			cmd = exec.Command("ffmpeg", "-f", "dshow", "-i", "audio=Microphone", "-ar", "16000", "-ac", "1", "-y", outputFile)
		} else {
			// Fallback to PowerShell with .NET
			psScript := fmt.Sprintf(`
Add-Type -AssemblyName System.Speech
$rec = New-Object System.Speech.Recognition.SpeechRecognitionEngine
$rec.SetInputToDefaultAudioDevice()
$grammar = New-Object System.Speech.Recognition.DictationGrammar
$rec.LoadGrammar($grammar)
$result = $rec.Recognize()
if ($result) { $result.Text } else { "" }
`)
			cmd = exec.Command("powershell", "-Command", psScript)
			output, err := cmd.Output()
			if err != nil {
				return fmt.Errorf("PowerShell speech recognition failed: %w", err)
			}
			// For Windows PowerShell, we get text directly, not audio
			// Write it to a text file instead
			return os.WriteFile(outputFile+".txt", output, 0644)
		}

	default:
		return fmt.Errorf("unsupported platform: %s", runtime.GOOS)
	}

	// Run the recording command
	cmd.Stdin = os.Stdin
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	// Start recording
	if err := cmd.Start(); err != nil {
		return err
	}

	// Wait for Ctrl+C or command to finish
	done := make(chan error, 1)
	go func() {
		done <- cmd.Wait()
	}()

	// Handle interrupt
	sigChan := make(chan os.Signal, 1)
	signalNotify(sigChan, os.Interrupt)

	select {
	case <-sigChan:
		// User pressed Ctrl+C, stop recording
		if cmd.Process != nil {
			cmd.Process.Signal(os.Interrupt)
			time.Sleep(100 * time.Millisecond)
			cmd.Process.Kill()
		}
		fmt.Println("\n✓ Recording stopped")
	case err := <-done:
		if err != nil {
			return err
		}
	}

	return nil
}

// signalNotify is a wrapper to allow testing
var signalNotify = func(c chan<- os.Signal, sig ...os.Signal) {
	// Use signal.Notify at runtime
	// This is a simple wrapper for testing purposes
	go func() {
		sigChan := make(chan os.Signal, 1)
		for s := range sigChan {
			c <- s
		}
	}()
}

// transcribe sends audio to OpenAI Whisper API
func (r *Recorder) transcribe(audioFile string) (string, error) {
	// Check for Windows PowerShell result
	if _, err := os.Stat(audioFile + ".txt"); err == nil {
		text, err := os.ReadFile(audioFile + ".txt")
		os.Remove(audioFile + ".txt")
		return string(text), err
	}

	// Read audio file
	audioData, err := os.ReadFile(audioFile)
	if err != nil {
		return "", err
	}

	// Create multipart form
	var buf bytes.Buffer
	writer := multipart.NewWriter(&buf)

	part, err := writer.CreateFormFile("file", filepath.Base(audioFile))
	if err != nil {
		return "", err
	}
	if _, err := part.Write(audioData); err != nil {
		return "", err
	}

	if err := writer.WriteField("model", "whisper-1"); err != nil {
		return "", err
	}
	writer.Close()

	// Send request
	req, err := http.NewRequest("POST", "https://api.openai.com/v1/audio/transcriptions", &buf)
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "Bearer "+r.apiKey)
	req.Header.Set("Content-Type", writer.FormDataContentType())

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()

	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return "", err
	}

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("API error (%d): %s", resp.StatusCode, string(body))
	}

	var result struct {
		Text string `json:"text"`
	}
	if err := json.Unmarshal(body, &result); err != nil {
		return "", err
	}

	return result.Text, nil
}
