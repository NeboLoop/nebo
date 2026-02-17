package tools

import (
	"bytes"
	"fmt"
	"regexp"
	"strings"
	"unicode"

	"golang.org/x/net/html"
	"golang.org/x/net/html/atom"
)

// defaultChunkSize is the default max characters per chunk returned to the LLM.
const defaultChunkSize = 50000

// skipElements are elements whose entire subtree should be discarded.
var skipElements = map[atom.Atom]bool{
	atom.Script:   true,
	atom.Style:    true,
	atom.Noscript: true,
	atom.Svg:      true,
	atom.Math:     true,
	atom.Template: true,
	atom.Iframe:   true,
	atom.Object:   true,
	atom.Embed:    true,
}

// hiddenStylePatterns are individual patterns that indicate hidden content.
// Checked independently against the style attribute value.
var hiddenStylePatterns = []*regexp.Regexp{
	regexp.MustCompile(`(?i)display\s*:\s*none`),
	regexp.MustCompile(`(?i)visibility\s*:\s*hidden`),
	regexp.MustCompile(`(?i)opacity\s*:\s*0(?:\s*[;"]|$)`),
	regexp.MustCompile(`(?i)font-size\s*:\s*0(?:px|em|rem|%)?(?:\s*[;"]|$)`),
	regexp.MustCompile(`(?i)(?:left|top)\s*:\s*-\d{4,}`),
}

// collapseSpaceRe collapses runs of whitespace into a single space.
var collapseSpaceRe = regexp.MustCompile(`[ \t]+`)

// multiNewlineRe collapses 3+ newlines into 2.
var multiNewlineRe = regexp.MustCompile(`\n{3,}`)

// ExtractVisibleText parses HTML and returns only the visible text content.
// It strips scripts, styles, hidden elements, and extracts what a human reader
// would see. Non-HTML content (JSON, plain text, etc.) passes through unchanged.
func ExtractVisibleText(raw []byte, contentType string) string {
	ct := strings.ToLower(contentType)

	// Only process HTML content — pass through everything else.
	if !strings.Contains(ct, "html") {
		return string(raw)
	}

	doc, err := html.Parse(bytes.NewReader(raw))
	if err != nil {
		// Parse failure — return raw as-is rather than losing content.
		return string(raw)
	}

	var buf strings.Builder
	buf.Grow(len(raw) / 3) // rough estimate: text is ~1/3 of HTML

	extractText(doc, &buf)

	text := buf.String()

	// Collapse whitespace runs into single spaces (per line).
	lines := strings.Split(text, "\n")
	for i, line := range lines {
		lines[i] = strings.TrimRightFunc(collapseSpaceRe.ReplaceAllString(line, " "), unicode.IsSpace)
	}
	text = strings.Join(lines, "\n")

	// Collapse excessive newlines.
	text = multiNewlineRe.ReplaceAllString(text, "\n\n")

	return strings.TrimSpace(text)
}

// extractText walks the HTML tree and writes visible text to buf.
func extractText(n *html.Node, buf *strings.Builder) {
	switch n.Type {
	case html.TextNode:
		buf.WriteString(n.Data)
		return

	case html.ElementNode:
		// Skip entire subtree for blacklisted elements.
		if skipElements[n.DataAtom] {
			return
		}

		// Skip elements with aria-hidden="true".
		if getAttr(n, "aria-hidden") == "true" {
			return
		}

		// Skip elements whose inline style hides them.
		if style := getAttr(n, "style"); style != "" && isHiddenStyle(style) {
			return
		}

		// Skip elements with hidden attribute.
		if hasAttr(n, "hidden") {
			return
		}

		// Block-level elements get newlines before/after.
		isBlock := isBlockElement(n.DataAtom)
		if isBlock {
			buf.WriteString("\n")
		}

		// Headings get a markdown-like prefix for structure.
		headingLevel := headingLevel(n.DataAtom)
		if headingLevel > 0 {
			buf.WriteString(strings.Repeat("#", headingLevel))
			buf.WriteString(" ")
		}

		// List items get a bullet.
		if n.DataAtom == atom.Li {
			buf.WriteString("• ")
		}

		// Walk children.
		for c := n.FirstChild; c != nil; c = c.NextSibling {
			extractText(c, buf)
		}

		// Line break elements.
		if n.DataAtom == atom.Br || n.DataAtom == atom.Hr {
			buf.WriteString("\n")
		}

		if isBlock {
			buf.WriteString("\n")
		}

	default:
		// Document, comment, doctype — walk children only.
		for c := n.FirstChild; c != nil; c = c.NextSibling {
			extractText(c, buf)
		}
	}
}

// ChunkText splits text into chunks of at most chunkSize characters,
// breaking at paragraph boundaries when possible.
// Returns the requested chunk (0-indexed) and total chunk count.
func ChunkText(text string, chunkSize, offset int) (chunk string, totalChunks int) {
	if chunkSize <= 0 {
		chunkSize = defaultChunkSize
	}

	totalLen := len(text)
	if totalLen <= chunkSize {
		return text, 1
	}

	// Build chunks by splitting at paragraph boundaries (\n\n).
	var chunks []string
	remaining := text

	for len(remaining) > 0 {
		if len(remaining) <= chunkSize {
			chunks = append(chunks, remaining)
			break
		}

		// Find the last paragraph break within chunkSize.
		cutPoint := chunkSize
		lastPara := strings.LastIndex(remaining[:chunkSize], "\n\n")
		if lastPara > chunkSize/4 { // don't cut too early
			cutPoint = lastPara + 2 // include the \n\n
		} else {
			// Fall back to last newline.
			lastNL := strings.LastIndex(remaining[:chunkSize], "\n")
			if lastNL > chunkSize/4 {
				cutPoint = lastNL + 1
			}
			// Otherwise hard-cut at chunkSize.
		}

		chunks = append(chunks, remaining[:cutPoint])
		remaining = remaining[cutPoint:]
	}

	totalChunks = len(chunks)
	if offset < 0 {
		offset = 0
	}
	if offset >= totalChunks {
		offset = totalChunks - 1
	}

	return chunks[offset], totalChunks
}

// FormatFetchResult formats a fetch result with HTTP header, chunk info, and the content chunk.
func FormatFetchResult(statusCode int, status, contentType string, totalBytes int, text string, chunkSize, offset int) string {
	if chunkSize <= 0 {
		chunkSize = defaultChunkSize
	}

	chunk, totalChunks := ChunkText(text, chunkSize, offset)

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("HTTP %d %s\nContent-Type: %s\nOriginal-Size: %d bytes\n", statusCode, status, contentType, totalBytes))

	if totalChunks > 1 {
		sb.WriteString(fmt.Sprintf("Chunk: %d/%d (use offset parameter to read other chunks)\n", offset+1, totalChunks))
	}

	sb.WriteString("\n")
	sb.WriteString(chunk)

	return sb.String()
}

// --- helpers ---

func isHiddenStyle(style string) bool {
	for _, re := range hiddenStylePatterns {
		if re.MatchString(style) {
			return true
		}
	}
	return false
}

func getAttr(n *html.Node, key string) string {
	for _, a := range n.Attr {
		if a.Key == key {
			return a.Val
		}
	}
	return ""
}

func hasAttr(n *html.Node, key string) bool {
	for _, a := range n.Attr {
		if a.Key == key {
			return true
		}
	}
	return false
}

func isBlockElement(a atom.Atom) bool {
	switch a {
	case atom.Div, atom.P, atom.Section, atom.Article, atom.Aside,
		atom.Header, atom.Footer, atom.Nav, atom.Main, atom.Figure,
		atom.Figcaption, atom.Blockquote, atom.Pre, atom.Ul, atom.Ol,
		atom.Li, atom.Dl, atom.Dt, atom.Dd, atom.Table, atom.Tr, atom.Td,
		atom.Th, atom.Thead, atom.Tbody, atom.Tfoot, atom.Caption,
		atom.H1, atom.H2, atom.H3, atom.H4, atom.H5, atom.H6,
		atom.Details, atom.Summary, atom.Fieldset, atom.Legend,
		atom.Address, atom.Hgroup, atom.Form:
		return true
	}
	return false
}

func headingLevel(a atom.Atom) int {
	switch a {
	case atom.H1:
		return 1
	case atom.H2:
		return 2
	case atom.H3:
		return 3
	case atom.H4:
		return 4
	case atom.H5:
		return 5
	case atom.H6:
		return 6
	}
	return 0
}
