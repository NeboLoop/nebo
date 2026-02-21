package tools

import (
	"image"
	"image/color"

	"github.com/fogleman/gg"
)

// Annotation colors
var (
	overlayColor = color.NRGBA{R: 51, G: 153, B: 255, A: 38}  // Semi-transparent blue
	borderColor  = color.NRGBA{R: 51, G: 153, B: 255, A: 200} // Blue border
	pillBG       = color.NRGBA{R: 30, G: 30, B: 30, A: 220}   // Dark background
	pillText     = color.White
)

const (
	borderWidth = 2.0
	pillPadX    = 4.0
	pillPadY    = 2.0
	pillRadius  = 4.0
)

// RenderAnnotations draws labeled element overlays on a screenshot.
// Returns a new image with annotations; the original is not modified.
func RenderAnnotations(img image.Image, elements []*Element) (image.Image, error) {
	bounds := img.Bounds()
	dc := gg.NewContext(bounds.Dx(), bounds.Dy())
	dc.DrawImage(img, 0, 0)

	for _, elem := range elements {
		if !elem.Actionable {
			continue
		}
		drawElementOverlay(dc, elem, bounds)
	}

	return dc.Image(), nil
}

func drawElementOverlay(dc *gg.Context, elem *Element, imgBounds image.Rectangle) {
	// Element bounds relative to image origin
	x := float64(elem.Bounds.X - imgBounds.Min.X)
	y := float64(elem.Bounds.Y - imgBounds.Min.Y)
	w := float64(elem.Bounds.Width)
	h := float64(elem.Bounds.Height)

	// Clamp to image bounds
	imgW := float64(imgBounds.Dx())
	imgH := float64(imgBounds.Dy())
	if x < 0 {
		w += x
		x = 0
	}
	if y < 0 {
		h += y
		y = 0
	}
	if x+w > imgW {
		w = imgW - x
	}
	if y+h > imgH {
		h = imgH - y
	}
	if w <= 0 || h <= 0 {
		return
	}

	// Draw semi-transparent overlay
	dc.SetColor(overlayColor)
	dc.DrawRectangle(x, y, w, h)
	dc.Fill()

	// Draw border
	dc.SetColor(borderColor)
	dc.SetLineWidth(borderWidth)
	dc.DrawRectangle(x, y, w, h)
	dc.Stroke()

	// Draw label pill
	drawLabelPill(dc, elem.ID, x, y, w, imgW, imgH)
}

func drawLabelPill(dc *gg.Context, label string, elemX, elemY, elemW, imgW, imgH float64) {
	// Measure text - use default font (no external font files needed)
	textW, textH := dc.MeasureString(label)
	pillW := textW + pillPadX*2
	pillH := textH + pillPadY*2

	// Try placement positions in order of preference:
	// 1. Above-left of element
	// 2. Above-right
	// 3. Below-left
	// 4. Inside top-left
	type pos struct{ x, y float64 }
	candidates := []pos{
		{elemX, elemY - pillH - 2},                // above-left
		{elemX + elemW - pillW, elemY - pillH - 2}, // above-right
		{elemX, elemY + pillH + 2},                 // below-left (offset by element height handled below)
		{elemX + 2, elemY + 2},                     // inside top-left
	}

	var px, py float64
	for _, c := range candidates {
		if c.x >= 0 && c.y >= 0 && c.x+pillW <= imgW && c.y+pillH <= imgH {
			px, py = c.x, c.y
			goto draw
		}
	}
	// Fallback: inside top-left regardless
	px, py = elemX+2, elemY+2

draw:
	// Draw pill background
	dc.SetColor(pillBG)
	dc.DrawRoundedRectangle(px, py, pillW, pillH, pillRadius)
	dc.Fill()

	// Draw text
	dc.SetColor(pillText)
	dc.DrawString(label, px+pillPadX, py+pillPadY+textH*0.85)
}
