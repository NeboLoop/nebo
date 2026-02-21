package tools

import (
	"image"
	"image/color"
	"testing"
)

func TestRenderAnnotations_Basic(t *testing.T) {
	// Create a synthetic 800x600 white image
	img := image.NewRGBA(image.Rect(0, 0, 800, 600))
	for y := 0; y < 600; y++ {
		for x := 0; x < 800; x++ {
			img.Set(x, y, color.White)
		}
	}

	elements := []*Element{
		{ID: "B1", Role: "button", Label: "Save", Bounds: Rect{X: 10, Y: 10, Width: 80, Height: 30}, Actionable: true},
		{ID: "T1", Role: "textfield", Label: "Input", Bounds: Rect{X: 100, Y: 10, Width: 200, Height: 30}, Actionable: true},
		{ID: "L1", Role: "link", Label: "Click here", Bounds: Rect{X: 50, Y: 300, Width: 100, Height: 20}, Actionable: true},
	}

	result, err := RenderAnnotations(img, elements)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Check output dimensions match input
	if result.Bounds().Dx() != 800 || result.Bounds().Dy() != 600 {
		t.Errorf("output size %dx%d, want 800x600", result.Bounds().Dx(), result.Bounds().Dy())
	}

	// The overlay area should no longer be pure white (annotations drawn)
	px := result.At(50, 25) // Middle of first button
	r, g, b, _ := px.RGBA()
	if r == 0xffff && g == 0xffff && b == 0xffff {
		t.Error("expected overlay to modify pixels within element bounds")
	}
}

func TestRenderAnnotations_NoElements(t *testing.T) {
	img := image.NewRGBA(image.Rect(0, 0, 100, 100))

	result, err := RenderAnnotations(img, nil)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	if result.Bounds().Dx() != 100 || result.Bounds().Dy() != 100 {
		t.Errorf("output size %dx%d, want 100x100", result.Bounds().Dx(), result.Bounds().Dy())
	}
}

func TestRenderAnnotations_NonActionableSkipped(t *testing.T) {
	img := image.NewRGBA(image.Rect(0, 0, 200, 200))
	for y := 0; y < 200; y++ {
		for x := 0; x < 200; x++ {
			img.Set(x, y, color.White)
		}
	}

	elements := []*Element{
		{ID: "X1", Role: "static text", Label: "Label", Bounds: Rect{X: 10, Y: 10, Width: 80, Height: 20}, Actionable: false},
	}

	result, err := RenderAnnotations(img, elements)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Non-actionable element should not have overlay — pixel should remain white
	px := result.At(50, 20)
	r, g, b, _ := px.RGBA()
	if r != 0xffff || g != 0xffff || b != 0xffff {
		t.Errorf("non-actionable element should not be annotated, got pixel (%d, %d, %d)", r>>8, g>>8, b>>8)
	}
}

func TestRenderAnnotations_OutOfBoundsElement(t *testing.T) {
	img := image.NewRGBA(image.Rect(0, 0, 100, 100))

	// Element completely outside image bounds
	elements := []*Element{
		{ID: "B1", Role: "button", Label: "Offscreen", Bounds: Rect{X: 200, Y: 200, Width: 80, Height: 30}, Actionable: true},
	}

	// Should not panic
	_, err := RenderAnnotations(img, elements)
	if err != nil {
		t.Fatalf("unexpected error for out-of-bounds element: %v", err)
	}
}

func TestRenderAnnotations_NegativeBoundsElement(t *testing.T) {
	img := image.NewRGBA(image.Rect(0, 0, 100, 100))

	// Element with negative position (partially offscreen)
	elements := []*Element{
		{ID: "B1", Role: "button", Label: "Partial", Bounds: Rect{X: -20, Y: -10, Width: 80, Height: 30}, Actionable: true},
	}

	// Should not panic, should clamp to image bounds
	_, err := RenderAnnotations(img, elements)
	if err != nil {
		t.Fatalf("unexpected error for negative bounds: %v", err)
	}
}

func TestRenderAnnotations_LargeElementCount(t *testing.T) {
	img := image.NewRGBA(image.Rect(0, 0, 1920, 1080))

	// 50 elements in a grid
	var elements []*Element
	for i := 0; i < 50; i++ {
		x := (i % 10) * 180
		y := (i / 10) * 200
		elements = append(elements, &Element{
			ID:         "B" + string(rune('0'+i%10)),
			Role:       "button",
			Label:      "Btn",
			Bounds:     Rect{X: x + 10, Y: y + 10, Width: 160, Height: 40},
			Actionable: true,
		})
	}

	_, err := RenderAnnotations(img, elements)
	if err != nil {
		t.Fatalf("unexpected error with many elements: %v", err)
	}
}

func TestRenderAnnotations_PreservesOriginal(t *testing.T) {
	// Original image should not be modified
	original := image.NewRGBA(image.Rect(0, 0, 100, 100))
	for y := 0; y < 100; y++ {
		for x := 0; x < 100; x++ {
			original.Set(x, y, color.RGBA{R: 255, G: 0, B: 0, A: 255})
		}
	}

	// Save a reference pixel
	beforeR, beforeG, beforeB, _ := original.At(50, 50).RGBA()

	elements := []*Element{
		{ID: "B1", Role: "button", Label: "Test", Bounds: Rect{X: 40, Y: 40, Width: 20, Height: 20}, Actionable: true},
	}

	_, err := RenderAnnotations(original, elements)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Original should still be red
	afterR, afterG, afterB, _ := original.At(50, 50).RGBA()
	if afterR != beforeR || afterG != beforeG || afterB != beforeB {
		t.Error("original image was modified by RenderAnnotations")
	}
}

func TestRenderAnnotations_ImageWithOffset(t *testing.T) {
	// Test with image that has non-zero origin (e.g., sub-image)
	parent := image.NewRGBA(image.Rect(0, 0, 500, 500))
	sub := parent.SubImage(image.Rect(100, 100, 400, 400))

	elements := []*Element{
		{ID: "B1", Role: "button", Label: "Test", Bounds: Rect{X: 150, Y: 150, Width: 80, Height: 30}, Actionable: true},
	}

	result, err := RenderAnnotations(sub, elements)
	if err != nil {
		t.Fatalf("unexpected error: %v", err)
	}

	// Output dimensions should match the sub-image, not the parent
	if result.Bounds().Dx() != 300 || result.Bounds().Dy() != 300 {
		t.Errorf("output size %dx%d, want 300x300", result.Bounds().Dx(), result.Bounds().Dy())
	}
}
