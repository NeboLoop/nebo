//go:build desktop && linux

package cli

/*
#cgo linux pkg-config: gtk+-3.0 webkit2gtk-4.1

#include <gtk/gtk.h>
#include <webkit2/webkit2.h>
#include <stdio.h>
#include <string.h>

static gboolean isLocalURI(const gchar *uri) {
    if (!uri) return FALSE;
    return g_str_has_prefix(uri, "http://localhost") ||
           g_str_has_prefix(uri, "http://127.0.0.1");
}

static void openInBrowser(const gchar *uri) {
    GError *error = NULL;
    gchar *argv[] = {"xdg-open", (gchar *)uri, NULL};
    g_spawn_async(NULL, argv, NULL, G_SPAWN_SEARCH_PATH, NULL, NULL, NULL, &error);
    if (error) {
        fprintf(stderr, "[webview] xdg-open failed: %s\n", error->message);
        g_error_free(error);
    }
}

// Global emission hook for the "decide-policy" signal on all WebKitWebView
// instances. Intercepts external navigation from localhost pages and opens
// the URL in the system browser.
static gboolean decidePolicyHook(GSignalInvocationHint *ihint,
    guint n_params, const GValue *params, gpointer data) {

    if (n_params < 3) return TRUE;

    WebKitWebView *webView = WEBKIT_WEB_VIEW(g_value_get_object(&params[0]));
    WebKitPolicyDecision *decision = WEBKIT_POLICY_DECISION(g_value_get_object(&params[1]));
    WebKitPolicyDecisionType type = (WebKitPolicyDecisionType)g_value_get_enum(&params[2]);

    // Only handle navigation and new-window actions
    if (type != WEBKIT_POLICY_DECISION_TYPE_NAVIGATION_ACTION &&
        type != WEBKIT_POLICY_DECISION_TYPE_NEW_WINDOW_ACTION) {
        return TRUE;
    }

    // Only intercept from localhost pages (Nebo's own UI)
    const gchar *currentURI = webkit_web_view_get_uri(webView);
    if (!isLocalURI(currentURI)) {
        return TRUE;
    }

    WebKitNavigationPolicyDecision *navDecision = WEBKIT_NAVIGATION_POLICY_DECISION(decision);
    WebKitNavigationAction *action = webkit_navigation_policy_decision_get_navigation_action(navDecision);
    WebKitURIRequest *request = webkit_navigation_action_get_request(action);
    const gchar *uri = webkit_uri_request_get_uri(request);

    if (!uri) return TRUE;

    // Allow localhost navigation
    if (isLocalURI(uri)) return TRUE;

    // External http/https URL — open in system browser and deny
    if (g_str_has_prefix(uri, "http://") || g_str_has_prefix(uri, "https://")) {
        printf("[webview] Redirecting external URL to system browser: %s\n", uri);
        openInBrowser(uri);
        webkit_policy_decision_ignore(decision);
        return TRUE;
    }

    return TRUE;
}

static void injectLinuxNavigationHandler() {
    guint signalId = g_signal_lookup("decide-policy", webkit_web_view_get_type());
    if (signalId == 0) {
        printf("[webview] decide-policy signal not found — cannot inject navigation handler\n");
        return;
    }

    g_signal_add_emission_hook(signalId, 0, decidePolicyHook, NULL, NULL);
    printf("[webview] Injected decide-policy emission hook\n");
}
*/
import "C"

// InjectWebViewNavigationHandler adds a global GObject emission hook on
// WebKitWebView's "decide-policy" signal to redirect external URLs from
// Nebo's UI to the system browser. Agent-controlled browser windows are
// unaffected. Call after application.New() (which initializes GTK).
func InjectWebViewNavigationHandler() {
	C.injectLinuxNavigationHandler()
}
