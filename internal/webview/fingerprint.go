package webview

import (
	"fmt"
	"math/rand"
	"strings"
)

// Fingerprint represents a unique browser fingerprint profile.
// Each native window gets a randomized fingerprint to appear as a different browser.
type Fingerprint struct {
	UserAgent       string
	Platform        string
	Language        string
	Languages       []string
	TimezoneOffset  int    // minutes from UTC
	Timezone        string // e.g. "America/New_York"
	ScreenWidth     int
	ScreenHeight    int
	ColorDepth      int
	PixelRatio      float64
	HardwareConcurrency int
	MaxTouchPoints  int
	WebGLVendor     string
	WebGLRenderer   string
	CanvasNoise     float64 // small noise value to perturb canvas fingerprint
}

// common screen resolutions (width x height)
var screenResolutions = [][2]int{
	{1920, 1080},
	{2560, 1440},
	{1366, 768},
	{1440, 900},
	{1536, 864},
	{1680, 1050},
	{1280, 720},
	{1600, 900},
	{2560, 1600},
	{1920, 1200},
}

var timezones = []struct {
	Name   string
	Offset int // minutes from UTC
}{
	{"America/New_York", -300},
	{"America/Chicago", -360},
	{"America/Denver", -420},
	{"America/Los_Angeles", -480},
	{"America/Phoenix", -420},
	{"Europe/London", 0},
	{"Europe/Berlin", 60},
	{"Europe/Paris", 60},
	{"Asia/Tokyo", 540},
	{"Asia/Shanghai", 480},
	{"Australia/Sydney", 660},
	{"Pacific/Auckland", 780},
}

var userAgents = []struct {
	UA       string
	Platform string
}{
	{
		"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.3 Safari/605.1.15",
		"MacIntel",
	},
	{
		"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
		"MacIntel",
	},
	{
		"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
		"Win32",
	},
	{
		"Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0",
		"Win32",
	},
	{
		"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
		"Linux x86_64",
	},
	{
		"Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36",
		"MacIntel",
	},
	{
		"Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0",
		"Win32",
	},
}

var webGLRenderers = []struct {
	Vendor   string
	Renderer string
}{
	{"Google Inc. (Apple)", "ANGLE (Apple, ANGLE Metal Renderer: Apple M1 Pro, Unspecified Version)"},
	{"Google Inc. (Apple)", "ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)"},
	{"Google Inc. (Apple)", "ANGLE (Apple, ANGLE Metal Renderer: Apple M3, Unspecified Version)"},
	{"Google Inc. (NVIDIA)", "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)"},
	{"Google Inc. (NVIDIA)", "ANGLE (NVIDIA, NVIDIA GeForce RTX 4070 Direct3D11 vs_5_0 ps_5_0, D3D11)"},
	{"Google Inc. (Intel)", "ANGLE (Intel, Intel(R) UHD Graphics 770 Direct3D11 vs_5_0 ps_5_0, D3D11)"},
	{"Google Inc. (AMD)", "ANGLE (AMD, AMD Radeon RX 7600 Direct3D11 vs_5_0 ps_5_0, D3D11)"},
	{"Google Inc. (Intel)", "ANGLE (Intel, Intel(R) Iris(R) Xe Graphics Direct3D11 vs_5_0 ps_5_0, D3D11)"},
}

var languages = [][]string{
	{"en-US", "en"},
	{"en-US", "en", "es"},
	{"en-GB", "en"},
	{"en-US", "en", "fr"},
	{"en-US", "en", "de"},
}

// GenerateFingerprint creates a randomized but consistent-looking fingerprint.
func GenerateFingerprint() *Fingerprint {
	ua := userAgents[rand.Intn(len(userAgents))]
	screen := screenResolutions[rand.Intn(len(screenResolutions))]
	tz := timezones[rand.Intn(len(timezones))]
	gl := webGLRenderers[rand.Intn(len(webGLRenderers))]
	lang := languages[rand.Intn(len(languages))]

	pixelRatios := []float64{1.0, 1.25, 1.5, 2.0}
	concurrencies := []int{4, 6, 8, 10, 12, 16}

	return &Fingerprint{
		UserAgent:            ua.UA,
		Platform:             ua.Platform,
		Language:             lang[0],
		Languages:            lang,
		TimezoneOffset:       tz.Offset,
		Timezone:             tz.Name,
		ScreenWidth:          screen[0],
		ScreenHeight:         screen[1],
		ColorDepth:           24,
		PixelRatio:           pixelRatios[rand.Intn(len(pixelRatios))],
		HardwareConcurrency: concurrencies[rand.Intn(len(concurrencies))],
		MaxTouchPoints:       0,
		WebGLVendor:          gl.Vendor,
		WebGLRenderer:        gl.Renderer,
		CanvasNoise:          rand.Float64()*0.001 + 0.0001, // tiny noise: 0.0001 to 0.0011
	}
}

// InjectJS returns JavaScript that overrides browser fingerprint APIs.
// Should be injected via ExecJS before any page scripts run.
func (fp *Fingerprint) InjectJS() string {
	langArray := make([]string, len(fp.Languages))
	for i, l := range fp.Languages {
		langArray[i] = fmt.Sprintf("%q", l)
	}

	return fmt.Sprintf(`(function(){
// Navigator overrides
var nav = navigator;
Object.defineProperty(nav, 'userAgent', {get: function(){return %s;}});
Object.defineProperty(nav, 'platform', {get: function(){return %s;}});
Object.defineProperty(nav, 'language', {get: function(){return %s;}});
Object.defineProperty(nav, 'languages', {get: function(){return [%s];}});
Object.defineProperty(nav, 'hardwareConcurrency', {get: function(){return %d;}});
Object.defineProperty(nav, 'maxTouchPoints', {get: function(){return %d;}});

// Screen overrides
Object.defineProperty(screen, 'width', {get: function(){return %d;}});
Object.defineProperty(screen, 'height', {get: function(){return %d;}});
Object.defineProperty(screen, 'availWidth', {get: function(){return %d;}});
Object.defineProperty(screen, 'availHeight', {get: function(){return %d;}});
Object.defineProperty(screen, 'colorDepth', {get: function(){return %d;}});
Object.defineProperty(window, 'devicePixelRatio', {get: function(){return %f;}});

// Timezone override
var origDTF = Intl.DateTimeFormat;
Intl.DateTimeFormat = function(locale, opts) {
  opts = opts || {};
  if (!opts.timeZone) opts.timeZone = %s;
  return new origDTF(locale, opts);
};
Object.setPrototypeOf(Intl.DateTimeFormat, origDTF);
Object.setPrototypeOf(Intl.DateTimeFormat.prototype, origDTF.prototype);
Date.prototype.getTimezoneOffset = function(){return %d;};

// WebGL fingerprint override
var origGetParam = WebGLRenderingContext.prototype.getParameter;
WebGLRenderingContext.prototype.getParameter = function(param) {
  var ext = this.getExtension('WEBGL_debug_renderer_info');
  if (ext) {
    if (param === ext.UNMASKED_VENDOR_WEBGL) return %s;
    if (param === ext.UNMASKED_RENDERER_WEBGL) return %s;
  }
  return origGetParam.call(this, param);
};
if (typeof WebGL2RenderingContext !== 'undefined') {
  var origGetParam2 = WebGL2RenderingContext.prototype.getParameter;
  WebGL2RenderingContext.prototype.getParameter = function(param) {
    var ext = this.getExtension('WEBGL_debug_renderer_info');
    if (ext) {
      if (param === ext.UNMASKED_VENDOR_WEBGL) return %s;
      if (param === ext.UNMASKED_RENDERER_WEBGL) return %s;
    }
    return origGetParam2.call(this, param);
  };
}

// Canvas fingerprint noise
var origToDataURL = HTMLCanvasElement.prototype.toDataURL;
HTMLCanvasElement.prototype.toDataURL = function(type, quality) {
  var ctx = this.getContext('2d');
  if (ctx) {
    var imgData = ctx.getImageData(0, 0, this.width, this.height);
    var noise = %f;
    for (var i = 0; i < imgData.data.length; i += 4) {
      imgData.data[i] = Math.min(255, Math.max(0, imgData.data[i] + Math.floor((Math.random() - 0.5) * noise * 255)));
    }
    ctx.putImageData(imgData, 0, 0);
  }
  return origToDataURL.call(this, type, quality);
};
})();`,
		jsonString(fp.UserAgent),
		jsonString(fp.Platform),
		jsonString(fp.Language),
		strings.Join(langArray, ","),
		fp.HardwareConcurrency,
		fp.MaxTouchPoints,
		fp.ScreenWidth,
		fp.ScreenHeight,
		fp.ScreenWidth,
		fp.ScreenHeight-40, // taskbar offset
		fp.ColorDepth,
		fp.PixelRatio,
		jsonString(fp.Timezone),
		fp.TimezoneOffset,
		jsonString(fp.WebGLVendor),
		jsonString(fp.WebGLRenderer),
		jsonString(fp.WebGLVendor),
		jsonString(fp.WebGLRenderer),
		fp.CanvasNoise,
	)
}
