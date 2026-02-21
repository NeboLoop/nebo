package tools

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"testing"
)

func TestVisionTool_Name(t *testing.T) {
	tool := NewVisionTool(VisionConfig{})
	if tool.Name() != "vision" {
		t.Errorf("got %q, want 'vision'", tool.Name())
	}
}

func TestVisionTool_RequiresApproval(t *testing.T) {
	tool := NewVisionTool(VisionConfig{})
	if tool.RequiresApproval() {
		t.Error("vision tool should not require approval")
	}
}

func TestVisionTool_NoProvider(t *testing.T) {
	tool := NewVisionTool(VisionConfig{})

	input, _ := json.Marshal(visionInput{
		Image:  "/tmp/test.png",
		Prompt: "Describe this",
	})

	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !result.IsError {
		t.Error("expected error when no provider configured")
	}
	if result.Content == "" {
		t.Error("expected error message")
	}
}

func TestVisionTool_EmptyImage(t *testing.T) {
	tool := NewVisionTool(VisionConfig{})

	input, _ := json.Marshal(visionInput{
		Image: "",
	})

	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !result.IsError {
		t.Error("expected error for empty image")
	}
}

func TestVisionTool_InvalidInput(t *testing.T) {
	tool := NewVisionTool(VisionConfig{})

	result, err := tool.Execute(context.Background(), json.RawMessage(`{invalid json}`))
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !result.IsError {
		t.Error("expected error for invalid JSON")
	}
}

func TestVisionTool_DefaultPrompt(t *testing.T) {
	var capturedPrompt string
	tool := NewVisionTool(VisionConfig{
		AnalyzeFunc: func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
			capturedPrompt = prompt
			return "analysis result", nil
		},
	})

	// Create a temp PNG file
	tmpDir := t.TempDir()
	imgPath := filepath.Join(tmpDir, "test.png")
	writeDummyPNG(t, imgPath)

	input, _ := json.Marshal(visionInput{
		Image: imgPath,
		// No prompt — should use default
	})

	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.IsError {
		t.Fatalf("unexpected tool error: %s", result.Content)
	}
	if capturedPrompt != "Describe this image in detail." {
		t.Errorf("got prompt=%q, want default", capturedPrompt)
	}
}

func TestVisionTool_WithAnalyzeFunc(t *testing.T) {
	var capturedBase64 string
	var capturedMediaType string
	var capturedPrompt string

	tool := NewVisionTool(VisionConfig{
		AnalyzeFunc: func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
			capturedBase64 = imageBase64
			capturedMediaType = mediaType
			capturedPrompt = prompt
			return "A red button", nil
		},
	})

	// Create a temp PNG file
	tmpDir := t.TempDir()
	imgPath := filepath.Join(tmpDir, "test.png")
	writeDummyPNG(t, imgPath)

	input, _ := json.Marshal(visionInput{
		Image:  imgPath,
		Prompt: "What is this?",
	})

	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.IsError {
		t.Fatalf("unexpected tool error: %s", result.Content)
	}
	if result.Content != "A red button" {
		t.Errorf("got content=%q, want 'A red button'", result.Content)
	}
	if capturedMediaType != "image/png" {
		t.Errorf("got mediaType=%q, want image/png", capturedMediaType)
	}
	if capturedPrompt != "What is this?" {
		t.Errorf("got prompt=%q, want 'What is this?'", capturedPrompt)
	}
	if capturedBase64 == "" {
		t.Error("expected non-empty base64 data")
	}
}

func TestVisionTool_SetAnalyzeFunc(t *testing.T) {
	tool := NewVisionTool(VisionConfig{})

	// Initially nil — should error
	tmpDir := t.TempDir()
	imgPath := filepath.Join(tmpDir, "test.png")
	writeDummyPNG(t, imgPath)

	input, _ := json.Marshal(visionInput{Image: imgPath})
	result, _ := tool.Execute(context.Background(), input)
	if !result.IsError {
		t.Error("expected error when AnalyzeFunc is nil")
	}

	// Set the func
	tool.SetAnalyzeFunc(func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
		return "now it works", nil
	})

	result, _ = tool.Execute(context.Background(), input)
	if result.IsError {
		t.Fatalf("unexpected error after SetAnalyzeFunc: %s", result.Content)
	}
	if result.Content != "now it works" {
		t.Errorf("got %q, want 'now it works'", result.Content)
	}
}

func TestVisionTool_AnalyzeFuncError(t *testing.T) {
	tool := NewVisionTool(VisionConfig{
		AnalyzeFunc: func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
			return "", fmt.Errorf("provider unavailable")
		},
	})

	tmpDir := t.TempDir()
	imgPath := filepath.Join(tmpDir, "test.png")
	writeDummyPNG(t, imgPath)

	input, _ := json.Marshal(visionInput{Image: imgPath, Prompt: "test"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !result.IsError {
		t.Error("expected tool error when AnalyzeFunc returns error")
	}
	if result.Content == "" {
		t.Error("expected error message in content")
	}
}

func TestVisionTool_FileNotFound(t *testing.T) {
	tool := NewVisionTool(VisionConfig{
		AnalyzeFunc: func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
			return "ok", nil
		},
	})

	input, _ := json.Marshal(visionInput{Image: "/nonexistent/path/image.png"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !result.IsError {
		t.Error("expected error for nonexistent file")
	}
}

func TestVisionTool_UnsupportedFormat(t *testing.T) {
	tool := NewVisionTool(VisionConfig{
		AnalyzeFunc: func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
			return "ok", nil
		},
	})

	tmpDir := t.TempDir()
	badFile := filepath.Join(tmpDir, "image.bmp")
	os.WriteFile(badFile, []byte("fake bmp"), 0644)

	input, _ := json.Marshal(visionInput{Image: badFile})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !result.IsError {
		t.Error("expected error for unsupported format")
	}
}

func TestVisionTool_Base64DataURL(t *testing.T) {
	var capturedBase64 string
	var capturedMediaType string

	tool := NewVisionTool(VisionConfig{
		AnalyzeFunc: func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
			capturedBase64 = imageBase64
			capturedMediaType = mediaType
			return "decoded", nil
		},
	})

	// Construct a data URL
	data := base64.StdEncoding.EncodeToString([]byte("fake-png-data"))
	dataURL := fmt.Sprintf("data:image/png;base64,%s", data)

	input, _ := json.Marshal(visionInput{Image: dataURL, Prompt: "test"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if result.IsError {
		t.Fatalf("unexpected tool error: %s", result.Content)
	}
	if capturedBase64 != data {
		t.Errorf("base64 data mismatch")
	}
	if capturedMediaType != "image/png" {
		t.Errorf("got mediaType=%q, want image/png", capturedMediaType)
	}
}

func TestVisionTool_InvalidDataURL(t *testing.T) {
	tool := NewVisionTool(VisionConfig{
		AnalyzeFunc: func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
			return "ok", nil
		},
	})

	// Data URL without comma separator
	input, _ := json.Marshal(visionInput{Image: "data:image/png;base64_no_comma"})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}
	if !result.IsError {
		t.Error("expected error for invalid data URL format")
	}
}

func TestVisionTool_JpegFile(t *testing.T) {
	var capturedMediaType string

	tool := NewVisionTool(VisionConfig{
		AnalyzeFunc: func(ctx context.Context, imageBase64, mediaType, prompt string) (string, error) {
			capturedMediaType = mediaType
			return "jpeg analysis", nil
		},
	})

	tmpDir := t.TempDir()
	for _, ext := range []string{".jpg", ".jpeg"} {
		imgPath := filepath.Join(tmpDir, "test"+ext)
		os.WriteFile(imgPath, []byte("fake-jpeg"), 0644)

		input, _ := json.Marshal(visionInput{Image: imgPath})
		result, _ := tool.Execute(context.Background(), input)
		if result.IsError {
			t.Fatalf("unexpected error for %s: %s", ext, result.Content)
		}
		if capturedMediaType != "image/jpeg" {
			t.Errorf("for %s: got mediaType=%q, want image/jpeg", ext, capturedMediaType)
		}
	}
}

// =============================================================================
// mediaTypeFromExt tests
// =============================================================================

func TestMediaTypeFromExt(t *testing.T) {
	tests := []struct {
		ext  string
		want string
	}{
		{".png", "image/png"},
		{".PNG", "image/png"},
		{".jpg", "image/jpeg"},
		{".jpeg", "image/jpeg"},
		{".JPEG", "image/jpeg"},
		{".gif", "image/gif"},
		{".webp", "image/webp"},
		{".bmp", ""},
		{".tiff", ""},
		{".svg", ""},
		{"", ""},
	}

	for _, tt := range tests {
		t.Run(tt.ext, func(t *testing.T) {
			got := mediaTypeFromExt(tt.ext)
			if got != tt.want {
				t.Errorf("mediaTypeFromExt(%q) = %q, want %q", tt.ext, got, tt.want)
			}
		})
	}
}

// =============================================================================
// Registry GetVisionTool tests
// =============================================================================

func TestRegistryGetVisionTool(t *testing.T) {
	registry := NewRegistry(nil)
	registry.RegisterDefaults()

	vt := registry.GetVisionTool()
	if vt == nil {
		t.Fatal("expected vision tool to be registered by default")
	}
	if vt.Name() != "vision" {
		t.Errorf("got name=%q, want 'vision'", vt.Name())
	}
}

func TestRegistryGetVisionTool_Empty(t *testing.T) {
	registry := NewRegistry(nil)
	// Don't register defaults

	vt := registry.GetVisionTool()
	if vt != nil {
		t.Error("expected nil when vision tool not registered")
	}
}

// =============================================================================
// helpers
// =============================================================================

// writeDummyPNG writes a minimal valid-ish PNG file (actually just bytes with .png extension)
// The vision tool reads the raw bytes and base64-encodes them — it doesn't validate PNG structure.
func writeDummyPNG(t *testing.T, path string) {
	t.Helper()
	// Minimal PNG: 8-byte signature + minimal IHDR + IEND
	// The tool only needs to read bytes and detect extension, not decode the PNG
	data := []byte{
		0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // PNG signature
		0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
		0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1 pixel
		0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xde, // bit depth, color type, CRC
		0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, // IEND
		0xae, 0x42, 0x60, 0x82, // IEND CRC
	}
	if err := os.WriteFile(path, data, 0644); err != nil {
		t.Fatalf("failed to write dummy PNG: %v", err)
	}
}
