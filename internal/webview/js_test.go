package webview

import (
	"strings"
	"testing"
)

func TestCallbackJSNativeBridge(t *testing.T) {
	js := callbackJS("http://localhost:27895/callback")
	if !strings.Contains(js, "window._wails") {
		t.Error("callbackJS should check for Wails native bridge")
	}
	if !strings.Contains(js, "nebo:cb:") {
		t.Error("callbackJS should use nebo:cb: prefix")
	}
	if !strings.Contains(js, "webkit.messageHandlers") {
		t.Error("callbackJS should have WebKit fallback")
	}
	if !strings.Contains(js, "fetch(") {
		t.Error("callbackJS should have HTTP fetch fallback")
	}
}

func TestWrapJSContainsRequestID(t *testing.T) {
	js := wrapJS("req-123", "http://localhost:27895/callback", `var __result="hello";`)

	if !strings.Contains(js, `"req-123"`) {
		t.Error("JS should contain the request ID")
	}
	if !strings.Contains(js, `var __result="hello"`) {
		t.Error("JS should contain the action code")
	}
	if !strings.Contains(js, "window._wails") {
		t.Error("JS should use native bridge (window._wails)")
	}
	if !strings.Contains(js, "__cb(") {
		t.Error("JS should call __cb() for callback")
	}
}

func TestWrapJSHasErrorHandler(t *testing.T) {
	js := wrapJS("req-err", "http://localhost/cb", `throw new Error("test");`)

	if !strings.Contains(js, "catch(e)") {
		t.Error("JS should have error catch handler")
	}
	if !strings.Contains(js, "error:e.message") {
		t.Error("JS should send error message in catch block")
	}
}

func TestPageInfoJS(t *testing.T) {
	js := pageInfoJS("req-info", "http://localhost/cb")

	if !strings.Contains(js, "location.href") {
		t.Error("pageInfoJS should read location.href")
	}
	if !strings.Contains(js, "document.title") {
		t.Error("pageInfoJS should read document.title")
	}
	if !strings.Contains(js, "scrollY") {
		t.Error("pageInfoJS should read scroll position")
	}
}

func TestSnapshotJS(t *testing.T) {
	js := snapshotJS("req-snap", "http://localhost/cb")

	if !strings.Contains(js, "__walk") {
		t.Error("snapshotJS should define __walk function")
	}
	if !strings.Contains(js, "data-nebo-ref") {
		t.Error("snapshotJS should set data-nebo-ref attributes")
	}
	if !strings.Contains(js, "isInteractive") {
		t.Error("snapshotJS should identify interactive elements")
	}
	if !strings.Contains(js, "contenteditable") {
		t.Error("snapshotJS should detect contenteditable elements")
	}
	if !strings.Contains(js, "aria-label") {
		t.Error("snapshotJS should read aria-label")
	}
	if !strings.Contains(js, "el.options") {
		t.Error("snapshotJS should enumerate select options")
	}
	if !strings.Contains(js, "el.required") {
		t.Error("snapshotJS should show required attribute")
	}
	if !strings.Contains(js, `tag==="form"`) {
		t.Error("snapshotJS should detect form elements")
	}
}

func TestClickJS(t *testing.T) {
	// By ref
	js := clickJS("req-click", "http://localhost/cb", "e5", "")
	if !strings.Contains(js, "data-nebo-ref") {
		t.Error("clickJS with ref should use data-nebo-ref selector")
	}
	if !strings.Contains(js, "el.click()") {
		t.Error("clickJS should call click()")
	}

	// By selector
	js = clickJS("req-click2", "http://localhost/cb", "", ".submit-btn")
	if !strings.Contains(js, ".submit-btn") {
		t.Error("clickJS with selector should use the CSS selector")
	}
}

func TestFillJS(t *testing.T) {
	js := fillJS("req-fill", "http://localhost/cb", "e3", "", "hello world")

	if !strings.Contains(js, "hello world") {
		t.Error("fillJS should contain the value")
	}
	if !strings.Contains(js, "nativeSetter") {
		t.Error("fillJS should use native value setter for React/Vue compat")
	}
	if !strings.Contains(js, "HTMLInputElement.prototype") {
		t.Error("fillJS should reference HTMLInputElement prototype")
	}
	if !strings.Contains(js, "InputEvent") {
		t.Error("fillJS should dispatch InputEvent")
	}
	if !strings.Contains(js, "change") {
		t.Error("fillJS should dispatch change event")
	}
}

func TestTypeJS(t *testing.T) {
	js := typeJS("req-type", "http://localhost/cb", "", "#search", "query text")

	if !strings.Contains(js, "query text") {
		t.Error("typeJS should contain the text to type")
	}
	if !strings.Contains(js, "nativeSetter") {
		t.Error("typeJS should use native value setter")
	}
	if !strings.Contains(js, "keydown") {
		t.Error("typeJS should dispatch keydown events")
	}
	if !strings.Contains(js, "keyup") {
		t.Error("typeJS should dispatch keyup events")
	}
	if !strings.Contains(js, "InputEvent") {
		t.Error("typeJS should dispatch InputEvent per character")
	}
}

func TestScrollJS(t *testing.T) {
	directions := map[string]string{
		"down":   "scrollBy(0,window.innerHeight",
		"up":     "scrollBy(0,-window.innerHeight",
		"top":    "scrollTo(0,0)",
		"bottom": "scrollTo(0,document.documentElement.scrollHeight)",
	}

	for dir, expected := range directions {
		js := scrollJS("req-scroll", "http://localhost/cb", dir)
		if !strings.Contains(js, expected) {
			t.Errorf("scrollJS(%q) should contain %q", dir, expected)
		}
	}
}

func TestWaitJS(t *testing.T) {
	js := waitJS("req-wait", "http://localhost/cb", ".loaded", 5000)

	if !strings.Contains(js, ".loaded") {
		t.Error("waitJS should contain the selector")
	}
	if !strings.Contains(js, "5000") {
		t.Error("waitJS should contain the timeout")
	}
	if !strings.Contains(js, "__poll") {
		t.Error("waitJS should use polling function")
	}
	if !strings.Contains(js, "__cb(") {
		t.Error("waitJS should use __cb callback (not fetch)")
	}
}

func TestGetTextJS(t *testing.T) {
	// With selector
	js := getTextJS("req-text", "http://localhost/cb", ".article")
	if !strings.Contains(js, ".article") {
		t.Error("getTextJS should contain the selector")
	}
	if !strings.Contains(js, "textContent") {
		t.Error("getTextJS should read textContent")
	}

	// Without selector (full body)
	js = getTextJS("req-text2", "http://localhost/cb", "")
	if !strings.Contains(js, "document.body.textContent") {
		t.Error("getTextJS without selector should read body textContent")
	}
}

func TestEvalJS(t *testing.T) {
	js := evalJS("req-eval", "http://localhost/cb", "return document.title;")
	if !strings.Contains(js, "return document.title;") {
		t.Error("evalJS should contain the user's code")
	}
}

func TestJSONStringEscaping(t *testing.T) {
	// Verify special characters are escaped
	result := jsonString(`he said "hello" and \n left`)
	if !strings.Contains(result, `\"hello\"`) {
		t.Error("jsonString should escape double quotes")
	}
	if !strings.Contains(result, `\\n`) {
		t.Error("jsonString should escape backslashes")
	}
}

func TestHoverJS(t *testing.T) {
	js := hoverJS("req-hover", "http://localhost/cb", "e2", "")
	if !strings.Contains(js, "mouseenter") {
		t.Error("hoverJS should dispatch mouseenter")
	}
	if !strings.Contains(js, "mouseover") {
		t.Error("hoverJS should dispatch mouseover")
	}
}

func TestSelectJS(t *testing.T) {
	js := selectJS("req-sel", "http://localhost/cb", "", "#country", "US")
	if !strings.Contains(js, "US") {
		t.Error("selectJS should contain the value")
	}
	if !strings.Contains(js, "change") {
		t.Error("selectJS should dispatch change event")
	}
}
