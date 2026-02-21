package markdown

import (
	"strings"
	"testing"
)

func TestRenderEmpty(t *testing.T) {
	if got := Render(""); got != "" {
		t.Errorf("Render(\"\") = %q, want \"\"", got)
	}
}

func TestRenderBasicMarkdown(t *testing.T) {
	html := Render("**bold** and *italic*")
	if !strings.Contains(html, "<strong>bold</strong>") {
		t.Errorf("Expected <strong>bold</strong>, got: %s", html)
	}
	if !strings.Contains(html, "<em>italic</em>") {
		t.Errorf("Expected <em>italic</em>, got: %s", html)
	}
}

func TestRenderGFMTable(t *testing.T) {
	md := "| A | B |\n|---|---|\n| 1 | 2 |"
	html := Render(md)
	if !strings.Contains(html, "<table>") {
		t.Errorf("Expected table HTML, got: %s", html)
	}
}

func TestRenderGFMStrikethrough(t *testing.T) {
	html := Render("~~deleted~~")
	if !strings.Contains(html, "<del>deleted</del>") {
		t.Errorf("Expected <del>deleted</del>, got: %s", html)
	}
}

func TestRenderGFMTaskList(t *testing.T) {
	html := Render("- [x] done\n- [ ] todo")
	if !strings.Contains(html, "checked") {
		t.Errorf("Expected checked checkbox, got: %s", html)
	}
}

func TestRenderCodeBlock(t *testing.T) {
	html := Render("```go\nfunc main() {}\n```")
	if !strings.Contains(html, "<pre") {
		t.Errorf("Expected <pre> block, got: %s", html)
	}
}

func TestRenderExternalLinks(t *testing.T) {
	html := Render("[Google](https://google.com)")
	if !strings.Contains(html, `target="_blank"`) {
		t.Errorf("Expected target=_blank on external link, got: %s", html)
	}
	if !strings.Contains(html, `rel="noopener noreferrer"`) {
		t.Errorf("Expected rel=noopener on external link, got: %s", html)
	}
}

func TestRenderInternalLinks(t *testing.T) {
	html := Render("[page](/about)")
	if strings.Contains(html, `target="_blank"`) {
		t.Errorf("Internal link should NOT have target=_blank, got: %s", html)
	}
}

func TestRenderYouTubeEmbed(t *testing.T) {
	html := Render("https://www.youtube.com/watch?v=dQw4w9WgXcQ")
	if !strings.Contains(html, "youtube.com/embed/dQw4w9WgXcQ") {
		t.Errorf("Expected YouTube embed, got: %s", html)
	}
	if !strings.Contains(html, "embed-container") {
		t.Errorf("Expected embed-container class, got: %s", html)
	}
}

func TestRenderVimeoEmbed(t *testing.T) {
	html := Render("https://vimeo.com/123456789")
	if !strings.Contains(html, "player.vimeo.com/video/123456789") {
		t.Errorf("Expected Vimeo embed, got: %s", html)
	}
}

func TestRenderTwitterEmbed(t *testing.T) {
	html := Render("https://x.com/elonmusk/status/1234567890")
	if !strings.Contains(html, "twitter-tweet") {
		t.Errorf("Expected Twitter embed, got: %s", html)
	}
	if !strings.Contains(html, "twitter.com/elonmusk/status/1234567890") {
		t.Errorf("Expected canonical Twitter URL, got: %s", html)
	}
}

func TestRenderURLInsideParagraph(t *testing.T) {
	// URLs inside text should NOT become embeds
	html := Render("Check out https://www.youtube.com/watch?v=dQw4w9WgXcQ for more info")
	if strings.Contains(html, "embed-container") {
		t.Errorf("URL inside text should not become embed, got: %s", html)
	}
}

func TestRenderHardWraps(t *testing.T) {
	html := Render("line1\nline2")
	if !strings.Contains(html, "<br") {
		t.Errorf("Expected hard wrap <br>, got: %s", html)
	}
}
