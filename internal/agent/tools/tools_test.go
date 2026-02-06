package tools

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestReadTool(t *testing.T) {
	// Create a temp file
	tmpDir := t.TempDir()
	testFile := filepath.Join(tmpDir, "test.txt")
	content := "line 1\nline 2\nline 3"
	if err := os.WriteFile(testFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	tool := NewReadTool()

	// Test reading the file
	input, _ := json.Marshal(ReadInput{Path: testFile})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}

	if result.IsError {
		t.Errorf("unexpected error: %s", result.Content)
	}

	if result.Content == "" {
		t.Error("expected content, got empty string")
	}
}

func TestWriteTool(t *testing.T) {
	tmpDir := t.TempDir()
	testFile := filepath.Join(tmpDir, "output.txt")

	tool := NewWriteTool()

	// Test writing a file
	input, _ := json.Marshal(WriteInput{
		Path:    testFile,
		Content: "hello world",
	})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}

	if result.IsError {
		t.Errorf("unexpected error: %s", result.Content)
	}

	// Verify file was written
	data, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatal(err)
	}

	if string(data) != "hello world" {
		t.Errorf("expected 'hello world', got %q", string(data))
	}
}

func TestEditTool(t *testing.T) {
	tmpDir := t.TempDir()
	testFile := filepath.Join(tmpDir, "edit.txt")
	if err := os.WriteFile(testFile, []byte("hello world"), 0644); err != nil {
		t.Fatal(err)
	}

	tool := NewEditTool()

	// Test editing the file
	input, _ := json.Marshal(EditInput{
		Path:      testFile,
		OldString: "world",
		NewString: "universe",
	})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}

	if result.IsError {
		t.Errorf("unexpected error: %s", result.Content)
	}

	// Verify file was edited
	data, err := os.ReadFile(testFile)
	if err != nil {
		t.Fatal(err)
	}

	if string(data) != "hello universe" {
		t.Errorf("expected 'hello universe', got %q", string(data))
	}
}

func TestGlobTool(t *testing.T) {
	tmpDir := t.TempDir()

	// Create test files
	os.WriteFile(filepath.Join(tmpDir, "file1.txt"), []byte(""), 0644)
	os.WriteFile(filepath.Join(tmpDir, "file2.txt"), []byte(""), 0644)
	os.WriteFile(filepath.Join(tmpDir, "file3.go"), []byte(""), 0644)

	tool := NewGlobTool()

	// Test globbing .txt files
	input, _ := json.Marshal(GlobInput{
		Pattern: "*.txt",
		Path:    tmpDir,
	})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}

	if result.IsError {
		t.Errorf("unexpected error: %s", result.Content)
	}

	// Should find 2 txt files
	if result.Content == "" {
		t.Error("expected to find files")
	}
}

func TestGrepTool(t *testing.T) {
	tmpDir := t.TempDir()
	testFile := filepath.Join(tmpDir, "search.txt")
	content := "line 1 foo\nline 2 bar\nline 3 foo bar"
	if err := os.WriteFile(testFile, []byte(content), 0644); err != nil {
		t.Fatal(err)
	}

	tool := NewGrepTool()

	// Test searching for "foo"
	input, _ := json.Marshal(GrepInput{
		Pattern: "foo",
		Path:    testFile,
	})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}

	if result.IsError {
		t.Errorf("unexpected error: %s", result.Content)
	}

	// Should find 2 matches
	if result.Content == "" {
		t.Error("expected to find matches")
	}
}

func TestBashTool(t *testing.T) {
	policy := NewPolicy()
	policy.Level = PolicyFull // Allow all for testing
	tool := NewBashTool(policy, nil) // nil registry for non-background tests

	// Test echo command
	input, _ := json.Marshal(BashInput{
		Command: "echo hello",
	})
	result, err := tool.Execute(context.Background(), input)
	if err != nil {
		t.Fatal(err)
	}

	if result.IsError {
		t.Errorf("unexpected error: %s", result.Content)
	}

	if result.Content != "hello\n" && result.Content != "hello" {
		t.Errorf("expected 'hello', got %q", result.Content)
	}
}

func TestPolicyAllowlist(t *testing.T) {
	policy := NewPolicy()

	// Safe commands should not require approval
	if policy.RequiresApproval("ls -la") {
		t.Error("ls should not require approval")
	}

	// git status should not require approval
	if policy.RequiresApproval("git status") {
		t.Error("git status should not require approval")
	}

	// rm should require approval
	if !policy.RequiresApproval("rm -rf /") {
		t.Error("rm should require approval")
	}
}
