//go:build desktop && darwin

package cli

/*
#cgo CFLAGS: -x objective-c
#cgo LDFLAGS: -framework Cocoa -framework WebKit

#import <objc/runtime.h>
#import <WebKit/WebKit.h>
#import <stdio.h>

// Auto-grant microphone and camera permissions for the WebView.
// WKWebView calls this WKUIDelegate method when getUserMedia() is invoked.
// Without it, WebKit shows a permission dialog every time.
static void grantMediaCapturePermission(id self, SEL _cmd, WKWebView *webView,
    id origin, id frame, WKMediaCaptureType type,
    void (^decisionHandler)(WKPermissionDecision)) {
    printf("[webview] Auto-granting media capture permission (type=%ld)\n", (long)type);
    decisionHandler(WKPermissionDecisionGrant);
}

// Injects the requestMediaCapturePermission delegate method into Wails'
// WebviewWindowDelegate class at runtime using the ObjC runtime.
// Must be called after the Wails app is created (so the class exists).
static int injectMediaPermissionHandler() {
    Class delegateClass = objc_getClass("WebviewWindowDelegate");
    if (!delegateClass) {
        printf("[webview] WebviewWindowDelegate class not found — cannot inject media permission handler\n");
        return 0;
    }

    SEL sel = @selector(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:);

    // Don't add if already present (e.g. future Wails version adds it)
    if (class_respondsToSelector(delegateClass, sel)) {
        printf("[webview] Media permission handler already exists — skipping injection\n");
        return 1;
    }

    // Method signature: void(id, SEL, WKWebView*, WKSecurityOrigin*, WKFrameInfo*, WKMediaCaptureType, block)
    // v = void, @ = object, : = SEL, @ = WKWebView, @ = origin, @ = frame, q = int64 (enum), @? = block
    BOOL ok = class_addMethod(delegateClass, sel, (IMP)grantMediaCapturePermission, "v@:@@@@q@?");
    if (ok) {
        printf("[webview] Successfully injected media permission auto-grant handler\n");
    } else {
        printf("[webview] Failed to inject media permission handler\n");
    }
    return ok ? 1 : 0;
}
*/
import "C"

// InjectWebViewMediaPermissions adds a WKUIDelegate method to Wails'
// WebviewWindowDelegate that auto-grants microphone/camera permissions.
// Call this after application.New() but before the window loads content.
func InjectWebViewMediaPermissions() {
	C.injectMediaPermissionHandler()
}
