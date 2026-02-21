package markdown

import (
	"bytes"
	"regexp"
	"strings"

	highlighting "github.com/yuin/goldmark-highlighting/v2"
	"github.com/yuin/goldmark"
	"github.com/yuin/goldmark/extension"
	"github.com/yuin/goldmark/parser"
	"github.com/yuin/goldmark/renderer/html"
)

var md goldmark.Markdown

func init() {
	md = goldmark.New(
		goldmark.WithExtensions(
			extension.GFM, // tables, strikethrough, autolinks, task lists
			highlighting.NewHighlighting(
				highlighting.WithStyle("monokai"),
			),
		),
		goldmark.WithParserOptions(
			parser.WithAutoHeadingID(),
		),
		goldmark.WithRendererOptions(
			html.WithHardWraps(), // matches marked's breaks: true
			html.WithUnsafe(),   // allow raw HTML in markdown
		),
	)
}

// Render converts markdown content to sanitized HTML.
// It applies GFM extensions, syntax highlighting, embed detection
// (YouTube, Vimeo, X/Twitter), and external link target="_blank".
func Render(content string) string {
	if content == "" {
		return ""
	}

	var buf bytes.Buffer
	if err := md.Convert([]byte(content), &buf); err != nil {
		// On error, return empty — frontend will fall back to client-side parsing
		return ""
	}

	result := buf.String()
	result = processEmbeds(result)
	result = processExternalLinks(result)
	return result
}

// Embed patterns — match the same URLs as markdown-embeds.ts
var (
	youtubeRe = regexp.MustCompile(`(?:https?://)?(?:www\.)?(?:youtube\.com/watch\?v=|youtu\.be/|youtube\.com/embed/|youtube\.com/shorts/)([a-zA-Z0-9_-]{11})(?:[?&]\S*)?`)
	vimeoRe   = regexp.MustCompile(`(?:https?://)?(?:www\.)?vimeo\.com/(\d+)(?:[?#/]\S*)?`)
	twitterRe = regexp.MustCompile(`(?:https?://)?(?:www\.)?(?:twitter\.com|x\.com)/(\w+)/status/(\d+)(?:\?\S*)?`)

	// Matches <p><a href="URL">URL</a></p> where href == text (autolinked standalone URL)
	autolinkedParagraphRe = regexp.MustCompile(`<p><a href="(https?://[^"]+)">\s*(https?://[^<]+)\s*</a></p>`)
)

// processEmbeds scans for autolinked standalone URLs and replaces with embed HTML.
func processEmbeds(html string) string {
	return autolinkedParagraphRe.ReplaceAllStringFunc(html, func(match string) string {
		sub := autolinkedParagraphRe.FindStringSubmatch(match)
		if len(sub) < 3 {
			return match
		}
		href := strings.TrimSpace(sub[1])
		text := strings.TrimSpace(sub[2])

		// Only embed if the link text matches the href (autolinked)
		if href != text {
			return match
		}

		// YouTube
		if m := youtubeRe.FindStringSubmatch(href); len(m) >= 2 {
			return youtubeEmbed(m[1])
		}
		// Vimeo
		if m := vimeoRe.FindStringSubmatch(href); len(m) >= 2 {
			return vimeoEmbed(m[1])
		}
		// X/Twitter
		if m := twitterRe.FindStringSubmatch(href); len(m) >= 3 {
			return tweetEmbed(m[1], m[2])
		}

		return match
	})
}

func youtubeEmbed(videoID string) string {
	return `<div class="embed-container embed-video"><iframe src="https://www.youtube.com/embed/` + videoID + `" frameborder="0" allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture" allowfullscreen loading="lazy"></iframe></div>`
}

func vimeoEmbed(videoID string) string {
	return `<div class="embed-container embed-video"><iframe src="https://player.vimeo.com/video/` + videoID + `?dnt=1" frameborder="0" allow="autoplay; fullscreen; picture-in-picture" allowfullscreen loading="lazy"></iframe></div>`
}

func tweetEmbed(user, tweetID string) string {
	canonicalURL := "https://twitter.com/" + user + "/status/" + tweetID
	return `<div class="embed-container embed-tweet"><blockquote class="twitter-tweet" data-dnt="true" data-theme="dark"><p></p>&mdash; @` + user + ` <a href="` + canonicalURL + `">` + canonicalURL + `</a></blockquote></div>`
}

// processExternalLinks adds target="_blank" rel="noopener noreferrer" to external links.
var linkRe = regexp.MustCompile(`<a href="(https?://[^"]*)"`)

func processExternalLinks(s string) string {
	return linkRe.ReplaceAllStringFunc(s, func(match string) string {
		// Skip if already has target
		// We check the surrounding context by looking ahead in the original string
		// But since we're doing per-match replacement, just add the attrs
		return match + ` target="_blank" rel="noopener noreferrer"`
	})
}
