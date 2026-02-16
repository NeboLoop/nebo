package webview

import (
	"strings"
	"testing"
)

func TestCursorClickJSContainsBezierPath(t *testing.T) {
	js := cursorClickJS("req-click", "http://localhost/cb", "e5", "")

	if !strings.Contains(js, "bezier") {
		t.Error("cursorClickJS should contain bezier curve function")
	}
	if !strings.Contains(js, "mousemove") {
		t.Error("cursorClickJS should dispatch mousemove events")
	}
	if !strings.Contains(js, "mousedown") {
		t.Error("cursorClickJS should dispatch mousedown")
	}
	if !strings.Contains(js, "mouseup") {
		t.Error("cursorClickJS should dispatch mouseup")
	}
	if !strings.Contains(js, "click") {
		t.Error("cursorClickJS should dispatch click")
	}
	if !strings.Contains(js, "moveStep") {
		t.Error("cursorClickJS should have an animated step function")
	}
}

func TestCursorClickJSByRef(t *testing.T) {
	js := cursorClickJS("req-cr", "http://localhost/cb", "e3", "")
	if !strings.Contains(js, "data-nebo-ref") {
		t.Error("cursorClickJS with ref should use data-nebo-ref selector")
	}
}

func TestCursorClickJSBySelector(t *testing.T) {
	js := cursorClickJS("req-cs", "http://localhost/cb", "", ".submit-btn")
	if !strings.Contains(js, ".submit-btn") {
		t.Error("cursorClickJS with selector should use CSS selector")
	}
}

func TestCursorClickJSHasEasing(t *testing.T) {
	js := cursorClickJS("req-ease", "http://localhost/cb", "e1", "")
	// Ease in-out formula
	if !strings.Contains(js, "0.5?2*t*t") {
		t.Error("cursorClickJS should have ease in-out timing")
	}
}

func TestCursorClickJSHasJitter(t *testing.T) {
	js := cursorClickJS("req-jit", "http://localhost/cb", "e1", "")
	if !strings.Contains(js, "jitter") {
		t.Error("cursorClickJS should apply jitter to path")
	}
}

func TestCursorClickJSHasRandomStartPoint(t *testing.T) {
	js := cursorClickJS("req-start", "http://localhost/cb", "e1", "")
	if !strings.Contains(js, "Math.random()*window.innerWidth") {
		t.Error("cursorClickJS should start from random screen position")
	}
}

func TestCursorClickJSHasReactionDelay(t *testing.T) {
	js := cursorClickJS("req-delay", "http://localhost/cb", "e1", "")
	// Should have delay between mousedown and mouseup (human reaction)
	if !strings.Contains(js, "20+Math.random()") {
		t.Error("cursorClickJS should have variable delay between mousedown/mouseup")
	}
}

func TestCursorHoverJSContainsBezierPath(t *testing.T) {
	js := cursorHoverJS("req-hover", "http://localhost/cb", "e2", "")

	if !strings.Contains(js, "bezier") {
		t.Error("cursorHoverJS should contain bezier curve function")
	}
	if !strings.Contains(js, "mouseover") {
		t.Error("cursorHoverJS should dispatch mouseover")
	}
	if !strings.Contains(js, "mouseenter") {
		t.Error("cursorHoverJS should dispatch mouseenter")
	}
	// Should NOT contain mousedown/click
	if strings.Contains(js, "mousedown") {
		t.Error("cursorHoverJS should NOT dispatch mousedown")
	}
}

func TestCursorClickJSUsesNativeBridge(t *testing.T) {
	js := cursorClickJS("req-bridge", "http://localhost/cb", "e1", "")
	if !strings.Contains(js, "window._wails") {
		t.Error("cursorClickJS should use Wails native bridge")
	}
	if !strings.Contains(js, "__cb(") {
		t.Error("cursorClickJS should use __cb callback")
	}
}

func TestCursorHoverJSUsesNativeBridge(t *testing.T) {
	js := cursorHoverJS("req-bridge", "http://localhost/cb", "e1", "")
	if !strings.Contains(js, "window._wails") {
		t.Error("cursorHoverJS should use Wails native bridge")
	}
	if !strings.Contains(js, "__cb(") {
		t.Error("cursorHoverJS should use __cb callback")
	}
}

func TestCursorClickJSParametersVary(t *testing.T) {
	// Generate multiple click JS and verify parameters vary (randomized steps/jitter/delay)
	jsSet := make(map[string]bool)
	for range 10 {
		js := cursorClickJS("req-vary", "http://localhost/cb", "e1", "")
		jsSet[js] = true
	}

	// With randomized parameters, we should get multiple distinct JS strings
	if len(jsSet) < 2 {
		t.Error("cursorClickJS should produce varied JS across calls (randomized parameters)")
	}
}
