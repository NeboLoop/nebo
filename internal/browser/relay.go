package browser

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"
	"github.com/gorilla/websocket"

	"nebo/internal/events"
)

const (
	RelayAuthHeader = "x-nebo-relay-token"
)

// relayDebug controls verbose CDP message tracing through the relay.
// Set to true to log all CDP messages between Playwright ↔ Relay ↔ Extension.
var relayDebug = false

func relayLog(format string, args ...any) {
	if relayDebug {
		fmt.Printf("[relay-cdp] "+format+"\n", args...)
	}
}

func truncateRelay(s string, maxLen int) string {
	if len(s) <= maxLen {
		return s
	}
	return s[:maxLen] + "..."
}

// cdpClientState tracks a CDP client's WebSocket and event subscription.
// All writes to ws go through the events system's single eventLoop goroutine.
type cdpClientState struct {
	ws           *websocket.Conn
	clientID     string
	subscription events.Subscription
}

// ExtensionRelay bridges a Chrome extension to CDP clients.
type ExtensionRelay struct {
	mu      sync.RWMutex
	writeMu sync.Mutex // Protects writes to extensionWS

	host      string
	port      int
	baseURL   string
	authToken string

	server      *http.Server
	upgrader    websocket.Upgrader
	extensionWS *websocket.Conn
	cdpClients  map[string]*cdpClientState
	cdpEvents   *events.Subject

	// Target tracking
	connectedTargets map[string]*ConnectedTarget

	// Pending requests to extension
	pendingRequests map[int]*pendingRequest
	nextRequestID   int

	stopped bool
}

// ConnectedTarget represents a tab attached via the extension.
type ConnectedTarget struct {
	SessionID  string      `json:"sessionId"`
	TargetID   string      `json:"targetId"`
	TargetInfo *TargetInfo `json:"targetInfo"`
}

// TargetInfo contains metadata about a browser target.
type TargetInfo struct {
	TargetID         string `json:"targetId"`
	Type             string `json:"type"`
	Title            string `json:"title"`
	URL              string `json:"url"`
	Attached         bool   `json:"attached"`
	BrowserContextID string `json:"browserContextId"`
}

type pendingRequest struct {
	resolve chan any
	reject  chan error
	timer   *time.Timer
}

// CDP protocol types
type cdpCommand struct {
	ID        int    `json:"id"`
	Method    string `json:"method"`
	Params    any    `json:"params,omitempty"`
	SessionID string `json:"sessionId,omitempty"`
}

type cdpResponse struct {
	ID        int         `json:"id"`
	Result    any         `json:"result,omitempty"`
	Error     *cdpError   `json:"error,omitempty"`
	SessionID string      `json:"sessionId,omitempty"`
}

type cdpError struct {
	Message string `json:"message"`
}

type cdpEvent struct {
	Method    string `json:"method"`
	Params    any    `json:"params,omitempty"`
	SessionID string `json:"sessionId,omitempty"`
}

// Extension protocol types
type extensionCommand struct {
	ID     int                     `json:"id"`
	Method string                  `json:"method"`
	Params *extensionCommandParams `json:"params,omitempty"`
}

type extensionCommandParams struct {
	Method    string `json:"method"`
	Params    any    `json:"params,omitempty"`
	SessionID string `json:"sessionId,omitempty"`
}

type extensionResponse struct {
	ID     int    `json:"id"`
	Result any    `json:"result,omitempty"`
	Error  string `json:"error,omitempty"`
}

type extensionEvent struct {
	Method string               `json:"method"`
	Params *extensionEventParams `json:"params,omitempty"`
}

type extensionEventParams struct {
	Method    string `json:"method"`
	Params    any    `json:"params,omitempty"`
	SessionID string `json:"sessionId,omitempty"`
}

// Singleton relay management
var (
	relaysMu     sync.RWMutex
	relaysByPort = make(map[int]*ExtensionRelay)
	authByPort   = make(map[int]string)
)

// GetRelayAuthHeaders returns auth headers for a relay URL.
func GetRelayAuthHeaders(rawURL string) map[string]string {
	u, err := url.Parse(rawURL)
	if err != nil {
		return nil
	}
	if !isLoopbackHost(u.Hostname()) {
		return nil
	}
	port := 80
	if u.Port() != "" {
		fmt.Sscanf(u.Port(), "%d", &port)
	} else if u.Scheme == "https" || u.Scheme == "wss" {
		port = 443
	}

	relaysMu.RLock()
	token := authByPort[port]
	relaysMu.RUnlock()

	if token == "" {
		return nil
	}
	return map[string]string{RelayAuthHeader: token}
}

// EnsureExtensionRelay ensures an extension relay is running for the given CDP URL.
func EnsureExtensionRelay(cdpURL string) (*ExtensionRelay, error) {
	u, err := url.Parse(cdpURL)
	if err != nil {
		return nil, fmt.Errorf("invalid cdpURL: %w", err)
	}

	host := u.Hostname()
	if !isLoopbackHost(host) {
		return nil, fmt.Errorf("extension relay requires loopback host, got %s", host)
	}

	port := 80
	if u.Port() != "" {
		fmt.Sscanf(u.Port(), "%d", &port)
	}

	relaysMu.Lock()
	defer relaysMu.Unlock()

	if relay, ok := relaysByPort[port]; ok && !relay.stopped {
		return relay, nil
	}

	relay, err := newExtensionRelay(host, port)
	if err != nil {
		return nil, err
	}

	relaysByPort[port] = relay
	authByPort[port] = relay.authToken
	return relay, nil
}

// StopExtensionRelay stops the relay for the given CDP URL.
func StopExtensionRelay(cdpURL string) error {
	u, err := url.Parse(cdpURL)
	if err != nil {
		return err
	}
	port := 80
	if u.Port() != "" {
		fmt.Sscanf(u.Port(), "%d", &port)
	}

	relaysMu.Lock()
	relay := relaysByPort[port]
	delete(relaysByPort, port)
	delete(authByPort, port)
	relaysMu.Unlock()

	if relay != nil {
		return relay.Stop()
	}
	return nil
}

func newExtensionRelay(host string, port int) (*ExtensionRelay, error) {
	// Generate auth token
	tokenBytes := make([]byte, 32)
	if _, err := rand.Read(tokenBytes); err != nil {
		return nil, err
	}
	authToken := base64.URLEncoding.EncodeToString(tokenBytes)

	relay := &ExtensionRelay{
		host:             host,
		port:             port,
		baseURL:          fmt.Sprintf("http://%s:%d", host, port),
		authToken:        authToken,
		cdpClients:       make(map[string]*cdpClientState),
		cdpEvents:        events.NewSubject(events.WithSyncDelivery(), events.WithBufferSize(256)),
		connectedTargets: make(map[string]*ConnectedTarget),
		pendingRequests:  make(map[int]*pendingRequest),
		nextRequestID:    1,
		upgrader: websocket.Upgrader{
			CheckOrigin: func(r *http.Request) bool {
				origin := r.Header.Get("Origin")
				// Allow Chrome extensions
				if strings.HasPrefix(origin, "chrome-extension://") {
					return true
				}
				// Allow no origin (direct connections)
				if origin == "" {
					return true
				}
				return false
			},
		},
	}

	mux := http.NewServeMux()
	mux.HandleFunc("/", relay.HandleRoot)
	mux.HandleFunc("/extension/status", relay.HandleExtensionStatus)
	mux.HandleFunc("/json/version", relay.HandleJSONVersion)
	mux.HandleFunc("/json/version/", relay.HandleJSONVersion)
	mux.HandleFunc("/json", relay.HandleJSONList)
	mux.HandleFunc("/json/", relay.HandleJSONList)
	mux.HandleFunc("/json/list", relay.HandleJSONList)
	mux.HandleFunc("/json/list/", relay.HandleJSONList)
	mux.HandleFunc("/json/activate/", relay.HandleJSONActivate)
	mux.HandleFunc("/json/close/", relay.HandleJSONClose)
	mux.HandleFunc("/extension", relay.HandleExtensionWS)
	mux.HandleFunc("/cdp", relay.HandleCdpWS)

	relay.server = &http.Server{
		Addr:    fmt.Sprintf("%s:%d", host, port),
		Handler: mux,
	}

	listener, err := net.Listen("tcp", relay.server.Addr)
	if err != nil {
		return nil, fmt.Errorf("failed to listen on %s: %w", relay.server.Addr, err)
	}

	go func() {
		if err := relay.server.Serve(listener); err != http.ErrServerClosed {
			fmt.Printf("relay server error: %v\n", err)
		}
	}()

	return relay, nil
}

// Stop stops the relay server.
func (r *ExtensionRelay) Stop() error {
	r.mu.Lock()
	r.stopped = true

	// Close extension connection
	if r.extensionWS != nil {
		r.extensionWS.Close()
		r.extensionWS = nil
	}

	// Close all CDP clients
	for id, client := range r.cdpClients {
		client.subscription.Unsubscribe()
		client.ws.Close()
		delete(r.cdpClients, id)
	}

	// Cancel pending requests
	for id, req := range r.pendingRequests {
		req.timer.Stop()
		req.reject <- fmt.Errorf("relay stopped")
		delete(r.pendingRequests, id)
	}
	r.mu.Unlock()

	events.Complete(r.cdpEvents)

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	return r.server.Shutdown(ctx)
}

// ExtensionConnected returns true if a Chrome extension is connected.
func (r *ExtensionRelay) ExtensionConnected() bool {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.extensionWS != nil
}

// CDPWebSocketURL returns the CDP WebSocket URL for Playwright to connect to.
func (r *ExtensionRelay) CDPWebSocketURL() string {
	// Parse baseURL to get the path prefix (e.g., /relay)
	if u, err := url.Parse(r.baseURL); err == nil && u.Path != "" {
		return fmt.Sprintf("ws://127.0.0.1:%d%s/cdp", r.port, u.Path)
	}
	return fmt.Sprintf("ws://127.0.0.1:%d/cdp", r.port)
}

// AuthToken returns the authentication token required for CDP connections.
func (r *ExtensionRelay) AuthToken() string {
	return r.authToken
}

// Handler returns an http.Handler that can be mounted on an existing server.
// Use this to mount the relay at a path like "/relay" on the main nebo server.
func (r *ExtensionRelay) Handler() http.Handler {
	router := chi.NewRouter()
	router.Get("/", r.HandleRoot)
	router.Head("/", r.HandleRoot)
	router.Get("/extension/status", r.HandleExtensionStatus)
	router.Get("/json/version", r.HandleJSONVersion)
	router.Get("/json", r.HandleJSONList)
	router.Get("/json/list", r.HandleJSONList)
	router.Get("/json/activate/{targetId}", r.HandleJSONActivate)
	router.Get("/json/close/{targetId}", r.HandleJSONClose)
	router.HandleFunc("/extension", r.HandleExtensionWS)
	router.HandleFunc("/cdp", r.HandleCdpWS)
	return router
}

// NewRelayHandler creates a relay that can be mounted on an existing server.
// Unlike EnsureExtensionRelay, this does not start its own HTTP server.
func NewRelayHandler(baseURL string) (*ExtensionRelay, error) {
	u, err := url.Parse(baseURL)
	if err != nil {
		return nil, err
	}

	host := u.Hostname()
	port := 80
	if u.Port() != "" {
		fmt.Sscanf(u.Port(), "%d", &port)
	}

	// Generate auth token
	tokenBytes := make([]byte, 32)
	if _, err := rand.Read(tokenBytes); err != nil {
		return nil, err
	}
	authToken := base64.URLEncoding.EncodeToString(tokenBytes)

	relay := &ExtensionRelay{
		host:             host,
		port:             port,
		baseURL:          baseURL,
		authToken:        authToken,
		cdpClients:       make(map[string]*cdpClientState),
		cdpEvents:        events.NewSubject(events.WithSyncDelivery(), events.WithBufferSize(256)),
		connectedTargets: make(map[string]*ConnectedTarget),
		pendingRequests:  make(map[int]*pendingRequest),
		nextRequestID:    1,
		upgrader: websocket.Upgrader{
			CheckOrigin: func(r *http.Request) bool {
				origin := r.Header.Get("Origin")
				if strings.HasPrefix(origin, "chrome-extension://") {
					return true
				}
				if origin == "" {
					return true
				}
				// Allow local.nebo.bot
				if strings.Contains(origin, "local.nebo.bot") || strings.Contains(origin, "127.0.0.1") || strings.Contains(origin, "localhost") {
					return true
				}
				return false
			},
		},
	}

	// Register in global maps so GetRelayAuthHeaders() can find it
	relaysMu.Lock()
	relaysByPort[port] = relay
	authByPort[port] = authToken
	relaysMu.Unlock()

	return relay, nil
}

// HTTP Handlers

func (r *ExtensionRelay) HandleRoot(w http.ResponseWriter, req *http.Request) {
	// Allow both "/" (when mounted) and full path (when used directly)
	w.Write([]byte("OK"))
}

func (r *ExtensionRelay) HandleExtensionStatus(w http.ResponseWriter, req *http.Request) {
	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(map[string]any{
		"connected": r.ExtensionConnected(),
		"port":      r.port,
	})
}

func (r *ExtensionRelay) HandleJSONVersion(w http.ResponseWriter, req *http.Request) {
	if !r.checkAuth(w, req) {
		return
	}

	payload := map[string]any{
		"Browser":          "Nebo/extension-relay",
		"Protocol-Version": "1.3",
	}
	if r.ExtensionConnected() {
		payload["webSocketDebuggerUrl"] = r.CDPWebSocketURL()
	}

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(payload)
}

func (r *ExtensionRelay) HandleJSONList(w http.ResponseWriter, req *http.Request) {
	if !r.checkAuth(w, req) {
		return
	}

	r.mu.RLock()
	list := make([]map[string]string, 0, len(r.connectedTargets))
	for _, t := range r.connectedTargets {
		list = append(list, map[string]string{
			"id":                   t.TargetID,
			"type":                 t.TargetInfo.Type,
			"title":                t.TargetInfo.Title,
			"url":                  t.TargetInfo.URL,
			"webSocketDebuggerUrl": r.CDPWebSocketURL(),
		})
	}
	r.mu.RUnlock()

	w.Header().Set("Content-Type", "application/json")
	json.NewEncoder(w).Encode(list)
}

func (r *ExtensionRelay) HandleJSONActivate(w http.ResponseWriter, req *http.Request) {
	if !r.checkAuth(w, req) {
		return
	}

	// Try chi path param first, then fall back to path parsing
	targetID := chi.URLParam(req, "targetId")
	if targetID == "" {
		// Fallback for standalone mode
		targetID = strings.TrimPrefix(req.URL.Path, "/json/activate/")
	}
	if targetID == "" {
		http.Error(w, "targetId required", http.StatusBadRequest)
		return
	}

	go func() {
		r.sendToExtension(&extensionCommand{
			ID:     r.nextID(),
			Method: "forwardCDPCommand",
			Params: &extensionCommandParams{
				Method: "Target.activateTarget",
				Params: map[string]string{"targetId": targetID},
			},
		})
	}()

	w.Write([]byte("OK"))
}

func (r *ExtensionRelay) HandleJSONClose(w http.ResponseWriter, req *http.Request) {
	if !r.checkAuth(w, req) {
		return
	}

	// Try chi path param first, then fall back to path parsing
	targetID := chi.URLParam(req, "targetId")
	if targetID == "" {
		// Fallback for standalone mode
		targetID = strings.TrimPrefix(req.URL.Path, "/json/close/")
	}
	if targetID == "" {
		http.Error(w, "targetId required", http.StatusBadRequest)
		return
	}

	go func() {
		r.sendToExtension(&extensionCommand{
			ID:     r.nextID(),
			Method: "forwardCDPCommand",
			Params: &extensionCommandParams{
				Method: "Target.closeTarget",
				Params: map[string]string{"targetId": targetID},
			},
		})
	}()

	w.Write([]byte("OK"))
}

func (r *ExtensionRelay) checkAuth(w http.ResponseWriter, req *http.Request) bool {
	// Only verify auth for /json paths (both standalone and when mounted at /relay)
	if !strings.HasPrefix(req.URL.Path, "/json") && !strings.Contains(req.URL.Path, "/json") {
		return true
	}

	// Allow loopback connections without token (same policy as HandleCdpWS)
	remoteIP := req.RemoteAddr
	if host, _, err := net.SplitHostPort(remoteIP); err == nil {
		remoteIP = host
	}
	if isLoopbackIP(remoteIP) {
		// Loopback: allow if token is empty OR matches
		token := req.Header.Get(RelayAuthHeader)
		if token == "" || token == r.authToken {
			return true
		}
		// Token provided but wrong
		http.Error(w, "Unauthorized", http.StatusUnauthorized)
		return false
	}

	// Non-loopback: require valid token
	token := req.Header.Get(RelayAuthHeader)
	if token != r.authToken {
		http.Error(w, "Unauthorized", http.StatusUnauthorized)
		return false
	}
	return true
}

// WebSocket Handlers

func (r *ExtensionRelay) HandleExtensionWS(w http.ResponseWriter, req *http.Request) {
	// Only allow loopback
	remoteIP := req.RemoteAddr
	if host, _, err := net.SplitHostPort(remoteIP); err == nil {
		remoteIP = host
	}
	if !isLoopbackIP(remoteIP) {
		http.Error(w, "Forbidden", http.StatusForbidden)
		return
	}

	r.mu.Lock()
	if r.extensionWS != nil {
		r.mu.Unlock()
		relayLog("Extension connection rejected: already connected")
		http.Error(w, "Extension already connected", http.StatusConflict)
		return
	}
	r.mu.Unlock()

	ws, err := r.upgrader.Upgrade(w, req, nil)
	if err != nil {
		relayLog("Extension WS upgrade failed: %v", err)
		return
	}

	relayLog("Extension connected from %s", req.RemoteAddr)
	r.mu.Lock()
	r.extensionWS = ws
	r.mu.Unlock()

	// Start ping ticker
	pingTicker := time.NewTicker(5 * time.Second)
	defer pingTicker.Stop()

	go func() {
		for range pingTicker.C {
			r.mu.RLock()
			extWS := r.extensionWS
			r.mu.RUnlock()
			if extWS == nil {
				return
			}
			r.writeMu.Lock()
			extWS.WriteJSON(map[string]string{"method": "ping"})
			r.writeMu.Unlock()
		}
	}()

	// Read messages from extension
	for {
		_, message, err := ws.ReadMessage()
		if err != nil {
			relayLog("Extension read error (disconnecting): %v", err)
			break
		}
		r.handleExtensionMessage(message)
	}

	// Cleanup on disconnect
	relayLog("Extension disconnected, cleaning up %d targets and %d CDP clients",
		len(r.connectedTargets), len(r.cdpClients))
	r.mu.Lock()
	r.extensionWS = nil
	r.connectedTargets = make(map[string]*ConnectedTarget)

	// Reject pending requests
	for id, req := range r.pendingRequests {
		req.timer.Stop()
		req.reject <- fmt.Errorf("extension disconnected")
		delete(r.pendingRequests, id)
	}

	// Close CDP clients
	for id, client := range r.cdpClients {
		client.subscription.Unsubscribe()
		client.ws.Close()
		delete(r.cdpClients, id)
	}
	r.mu.Unlock()
}

func (r *ExtensionRelay) HandleCdpWS(w http.ResponseWriter, req *http.Request) {
	// Only allow loopback
	remoteIP := req.RemoteAddr
	if host, _, err := net.SplitHostPort(remoteIP); err == nil {
		remoteIP = host
	}
	if !isLoopbackIP(remoteIP) {
		http.Error(w, "Forbidden", http.StatusForbidden)
		return
	}

	// Check auth - but allow loopback connections without token
	// since we already verified the request is from localhost
	token := req.Header.Get(RelayAuthHeader)
	if token != "" && token != r.authToken {
		http.Error(w, "Unauthorized", http.StatusUnauthorized)
		return
	}

	if !r.ExtensionConnected() {
		relayLog("CDP client rejected: extension not connected")
		http.Error(w, "Chrome extension not connected. Click the Nebo extension icon in Chrome to attach a tab.", http.StatusServiceUnavailable)
		return
	}

	ws, err := r.upgrader.Upgrade(w, req, nil)
	if err != nil {
		return
	}

	clientID := uuid.NewString()
	relayLog("CDP client connected: %s", clientID)

	// Subscribe to per-client topic — handler runs in the single eventLoop goroutine
	sub := events.Subscribe[any](r.cdpEvents, events.CDPClientTopic(clientID),
		func(_ context.Context, msg any) error {
			if relayDebug {
				if data, err := json.Marshal(msg); err == nil {
					relayLog("  → CDP client %s: %s", clientID[:8], truncateRelay(string(data), 300))
				}
			}
			return ws.WriteJSON(msg)
		})

	r.mu.Lock()
	r.cdpClients[clientID] = &cdpClientState{
		ws:           ws,
		clientID:     clientID,
		subscription: sub,
	}
	r.mu.Unlock()

	// Read CDP commands
	for {
		_, message, err := ws.ReadMessage()
		if err != nil {
			relayLog("CDP client %s read error: %v", clientID[:8], err)
			break
		}

		var cmd cdpCommand
		if err := json.Unmarshal(message, &cmd); err != nil {
			relayLog("CDP client %s unmarshal error: %v", clientID[:8], err)
			continue
		}

		relayLog("← CDP client %s: id=%d method=%s sessionId=%q", clientID[:8], cmd.ID, cmd.Method, cmd.SessionID)
		r.handleCdpCommand(clientID, &cmd)
	}

	relayLog("CDP client disconnected: %s", clientID[:8])
	r.mu.Lock()
	delete(r.cdpClients, clientID)
	r.mu.Unlock()
	sub.Unsubscribe()
}

// Message handling

func (r *ExtensionRelay) handleExtensionMessage(data []byte) {
	relayLog("← Extension: %s", truncateRelay(string(data), 300))

	// Try to parse as response first
	var resp extensionResponse
	if err := json.Unmarshal(data, &resp); err == nil && resp.ID > 0 {
		r.mu.Lock()
		pending := r.pendingRequests[resp.ID]
		delete(r.pendingRequests, resp.ID)
		r.mu.Unlock()

		if pending != nil {
			pending.timer.Stop()
			if resp.Error != "" {
				pending.reject <- fmt.Errorf("%s", resp.Error)
			} else {
				pending.resolve <- resp.Result
			}
		}
		return
	}

	// Try to parse as event
	var evt extensionEvent
	if err := json.Unmarshal(data, &evt); err != nil {
		return
	}

	if evt.Method == "pong" {
		return
	}

	if evt.Method != "forwardCDPEvent" || evt.Params == nil {
		return
	}

	method := evt.Params.Method
	params := evt.Params.Params
	sessionID := evt.Params.SessionID

	// Handle target attachment events
	if method == "Target.attachedToTarget" {
		r.handleTargetAttached(params)
		return
	}

	if method == "Target.detachedFromTarget" {
		r.handleTargetDetached(params)
		return
	}

	if method == "Target.targetInfoChanged" {
		r.handleTargetInfoChanged(params)
		// Fall through to broadcast the event to CDP clients
	}

	// Broadcast to all CDP clients
	r.broadcastToCdpClients(&cdpEvent{
		Method:    method,
		Params:    params,
		SessionID: sessionID,
	})
}

func (r *ExtensionRelay) handleTargetAttached(params any) {
	paramsMap, ok := params.(map[string]any)
	if !ok {
		return
	}

	sessionID, _ := paramsMap["sessionId"].(string)
	targetInfoRaw, _ := paramsMap["targetInfo"].(map[string]any)
	if sessionID == "" || targetInfoRaw == nil {
		return
	}

	targetType, _ := targetInfoRaw["type"].(string)
	if targetType != "page" && targetType != "" {
		return
	}

	targetID, _ := targetInfoRaw["targetId"].(string)
	title, _ := targetInfoRaw["title"].(string)
	url, _ := targetInfoRaw["url"].(string)
	browserContextID, _ := targetInfoRaw["browserContextId"].(string)
	if browserContextID == "" {
		browserContextID = "default"
	}
	// Default type to "page" if not specified
	if targetType == "" {
		targetType = "page"
	}

	target := &ConnectedTarget{
		SessionID: sessionID,
		TargetID:  targetID,
		TargetInfo: &TargetInfo{
			TargetID:         targetID,
			Type:             targetType,
			Title:            title,
			URL:              url,
			Attached:         true,
			BrowserContextID: browserContextID,
		},
	}

	relayLog("Target attached: sessionId=%s targetId=%s type=%s url=%s browserContextId=%s",
		sessionID, targetID, targetType, truncateRelay(url, 80), browserContextID)

	r.mu.Lock()
	r.connectedTargets[sessionID] = target
	r.mu.Unlock()

	// Broadcast attachment event (no top-level SessionID — this is a browser-level event)
	r.broadcastToCdpClients(&cdpEvent{
		Method: "Target.attachedToTarget",
		Params: map[string]any{
			"sessionId":          sessionID,
			"targetInfo":         target.TargetInfo,
			"waitingForDebugger": false,
		},
	})
}

func (r *ExtensionRelay) handleTargetDetached(params any) {
	paramsMap, ok := params.(map[string]any)
	if !ok {
		return
	}

	sessionID, _ := paramsMap["sessionId"].(string)
	if sessionID == "" {
		return
	}

	r.mu.Lock()
	delete(r.connectedTargets, sessionID)
	r.mu.Unlock()

	// No top-level SessionID — this is a browser-level event
	r.broadcastToCdpClients(&cdpEvent{
		Method: "Target.detachedFromTarget",
		Params: params,
	})
}

func (r *ExtensionRelay) handleTargetInfoChanged(params any) {
	paramsMap, ok := params.(map[string]any)
	if !ok {
		return
	}

	targetInfoRaw, _ := paramsMap["targetInfo"].(map[string]any)
	if targetInfoRaw == nil {
		return
	}

	targetID, _ := targetInfoRaw["targetId"].(string)
	if targetID == "" {
		return
	}

	r.mu.Lock()
	for _, target := range r.connectedTargets {
		if target.TargetID == targetID {
			if title, ok := targetInfoRaw["title"].(string); ok {
				target.TargetInfo.Title = title
			}
			if url, ok := targetInfoRaw["url"].(string); ok {
				target.TargetInfo.URL = url
			}
		}
	}
	r.mu.Unlock()
}

func (r *ExtensionRelay) handleCdpCommand(clientID string, cmd *cdpCommand) {
	topic := events.CDPClientTopic(clientID)

	var result any
	var err error
	// postEvents are sent AFTER the response (Playwright expects response before events)
	var postEvents []any

	// Handle commands locally or forward to extension
	switch cmd.Method {
	case "Browser.getVersion":
		result = map[string]string{
			"protocolVersion": "1.3",
			"product":         "Chrome/Nebo-Extension-Relay",
			"revision":        "0",
			"userAgent":       "Nebo-Extension-Relay",
			"jsVersion":       "V8",
		}
	case "Browser.setDownloadBehavior":
		result = map[string]any{}
	case "Target.setAutoAttach":
		result = map[string]any{}
		// Queue existing targets as post-response events (browser-level only)
		if cmd.SessionID == "" {
			postEvents = r.buildExistingTargetEvents(clientID, cmd.Method)
		}
	case "Target.setDiscoverTargets":
		result = map[string]any{}
		if params, ok := cmd.Params.(map[string]any); ok {
			if discover, _ := params["discover"].(bool); discover {
				postEvents = r.buildExistingTargetEvents(clientID, cmd.Method)
			}
		}
	case "Target.getTargets":
		r.mu.RLock()
		targets := make([]map[string]any, 0, len(r.connectedTargets))
		for _, t := range r.connectedTargets {
			targetInfo := map[string]any{
				"targetId": t.TargetID,
				"type":     t.TargetInfo.Type,
				"title":    t.TargetInfo.Title,
				"url":      t.TargetInfo.URL,
				"attached": true,
			}
			if t.TargetInfo.BrowserContextID != "" {
				targetInfo["browserContextId"] = t.TargetInfo.BrowserContextID
			}
			targets = append(targets, targetInfo)
		}
		r.mu.RUnlock()
		result = map[string]any{"targetInfos": targets}
	case "Target.getTargetInfo":
		result = r.getTargetInfo(cmd)
	case "Target.attachToTarget":
		result, err = r.attachToTarget(cmd)
		// Queue attachedToTarget event as post-response event
		if err == nil {
			if params, ok := cmd.Params.(map[string]any); ok {
				if targetID, _ := params["targetId"].(string); targetID != "" {
					r.mu.RLock()
					for _, t := range r.connectedTargets {
						if t.TargetID == targetID {
							postEvents = append(postEvents, &cdpEvent{
								Method: "Target.attachedToTarget",
								Params: map[string]any{
									"sessionId":          t.SessionID,
									"targetInfo":         t.TargetInfo,
									"waitingForDebugger": false,
								},
							})
							break
						}
					}
					r.mu.RUnlock()
				}
			}
		}
	default:
		// Forward to extension
		result, err = r.forwardToExtension(cmd)
	}

	// Send response FIRST
	resp := &cdpResponse{
		ID:        cmd.ID,
		SessionID: cmd.SessionID,
	}
	if err != nil {
		resp.Error = &cdpError{Message: err.Error()}
	} else {
		resp.Result = result
	}

	events.Emit[any](r.cdpEvents, topic, resp)

	// Then send post-response events (e.g., Target.attachedToTarget after setAutoAttach)
	for _, evt := range postEvents {
		events.Emit[any](r.cdpEvents, topic, evt)
	}
}

func (r *ExtensionRelay) forwardToExtension(cmd *cdpCommand) (any, error) {
	extCmd := &extensionCommand{
		ID:     r.nextID(),
		Method: "forwardCDPCommand",
		Params: &extensionCommandParams{
			Method:    cmd.Method,
			Params:    cmd.Params,
			SessionID: cmd.SessionID,
		},
	}

	relayLog("→ Extension: id=%d method=%s sessionId=%q", extCmd.ID, cmd.Method, cmd.SessionID)
	result, err := r.sendToExtension(extCmd)
	if err != nil {
		relayLog("→ Extension error: %v", err)
	}
	return result, err
}

func (r *ExtensionRelay) sendToExtension(cmd *extensionCommand) (any, error) {
	r.mu.RLock()
	ws := r.extensionWS
	r.mu.RUnlock()

	if ws == nil {
		return nil, fmt.Errorf("extension not connected")
	}

	resolve := make(chan any, 1)
	reject := make(chan error, 1)
	timer := time.AfterFunc(30*time.Second, func() {
		r.mu.Lock()
		delete(r.pendingRequests, cmd.ID)
		r.mu.Unlock()
		reject <- fmt.Errorf("extension request timeout")
	})

	r.mu.Lock()
	r.pendingRequests[cmd.ID] = &pendingRequest{
		resolve: resolve,
		reject:  reject,
		timer:   timer,
	}
	r.mu.Unlock()

	r.writeMu.Lock()
	err := ws.WriteJSON(cmd)
	r.writeMu.Unlock()

	if err != nil {
		r.mu.Lock()
		delete(r.pendingRequests, cmd.ID)
		r.mu.Unlock()
		timer.Stop()
		return nil, err
	}

	select {
	case result := <-resolve:
		return result, nil
	case err := <-reject:
		return nil, err
	}
}

func (r *ExtensionRelay) broadcastToCdpClients(evt *cdpEvent) {
	r.mu.RLock()
	ids := make([]string, 0, len(r.cdpClients))
	for id := range r.cdpClients {
		ids = append(ids, id)
	}
	r.mu.RUnlock()

	for _, id := range ids {
		events.Emit[any](r.cdpEvents, events.CDPClientTopic(id), evt)
	}
}

func (r *ExtensionRelay) buildExistingTargetEvents(_ string, method string) []any {
	r.mu.RLock()
	targets := make([]*ConnectedTarget, 0, len(r.connectedTargets))
	for _, t := range r.connectedTargets {
		targets = append(targets, t)
	}
	r.mu.RUnlock()

	evts := make([]any, 0, len(targets))
	for _, target := range targets {
		if method == "Target.setAutoAttach" {
			evts = append(evts, &cdpEvent{
				Method: "Target.attachedToTarget",
				Params: map[string]any{
					"sessionId":          target.SessionID,
					"targetInfo":         target.TargetInfo,
					"waitingForDebugger": false,
				},
			})
		} else {
			evts = append(evts, &cdpEvent{
				Method: "Target.targetCreated",
				Params: map[string]any{
					"targetInfo": target.TargetInfo,
				},
			})
		}
	}
	return evts
}

func (r *ExtensionRelay) getTargetInfo(cmd *cdpCommand) map[string]any {
	var targetID string
	if params, ok := cmd.Params.(map[string]any); ok {
		targetID, _ = params["targetId"].(string)
	}

	r.mu.RLock()
	defer r.mu.RUnlock()

	// Find by targetId
	if targetID != "" {
		for _, t := range r.connectedTargets {
			if t.TargetID == targetID {
				return map[string]any{"targetInfo": t.TargetInfo}
			}
		}
	}

	// Find by sessionId
	if cmd.SessionID != "" {
		if t, ok := r.connectedTargets[cmd.SessionID]; ok {
			return map[string]any{"targetInfo": t.TargetInfo}
		}
	}

	// Return first available
	for _, t := range r.connectedTargets {
		return map[string]any{"targetInfo": t.TargetInfo}
	}

	return map[string]any{"targetInfo": nil}
}

func (r *ExtensionRelay) attachToTarget(cmd *cdpCommand) (map[string]any, error) {
	var targetID string
	if params, ok := cmd.Params.(map[string]any); ok {
		targetID, _ = params["targetId"].(string)
	}
	if targetID == "" {
		return nil, fmt.Errorf("targetId required")
	}

	r.mu.RLock()
	defer r.mu.RUnlock()

	for _, t := range r.connectedTargets {
		if t.TargetID == targetID {
			return map[string]any{"sessionId": t.SessionID}, nil
		}
	}

	return nil, fmt.Errorf("target not found")
}

func (r *ExtensionRelay) nextID() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	id := r.nextRequestID
	r.nextRequestID++
	return id
}

func isLoopbackHost(host string) bool {
	h := strings.ToLower(strings.TrimSpace(host))
	return h == "localhost" || h == "127.0.0.1" || h == "0.0.0.0" ||
		h == "[::1]" || h == "::1" || h == "[::]" || h == "::"
}

func isLoopbackIP(ip string) bool {
	if ip == "127.0.0.1" || strings.HasPrefix(ip, "127.") {
		return true
	}
	if ip == "::1" || strings.HasPrefix(ip, "::ffff:127.") {
		return true
	}
	return false
}
