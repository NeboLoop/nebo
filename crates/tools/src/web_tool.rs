use std::sync::Arc;

use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ResourceKind, ToolResult};

/// Threshold in characters above which tool output is spilled to a file.
/// Matches `large_input.rs` sizing (~2000 tokens at chars/4 heuristic).
const LARGE_OUTPUT_THRESHOLD: usize = 8_000;

/// Default preview length (chars) included in the compact metadata response.
const DEFAULT_PREVIEW_CHARS: usize = 1_200;

/// Callback type for broadcasting events to connected WebSocket clients.
pub type Broadcaster = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

/// WebTool consolidates web operations: HTTP fetch, search, and browser automation.
pub struct WebTool {
    client: reqwest::Client,
    browser: Option<Arc<browser::Manager>>,
    store: Option<Arc<db::Store>>,
    broadcaster: Option<Broadcaster>,
}

impl WebTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            browser: None,
            store: None,
            broadcaster: None,
        }
    }

    pub fn with_browser(mut self, manager: Arc<browser::Manager>) -> Self {
        self.browser = Some(manager);
        self
    }

    pub fn with_store(mut self, store: Arc<db::Store>) -> Self {
        self.store = Some(store);
        self
    }

    pub fn with_broadcaster(mut self, broadcaster: Broadcaster) -> Self {
        self.broadcaster = Some(broadcaster);
        self
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "fetch" | "get" | "post" | "put" | "delete" | "head" | "sanitize" => "http",
            "search" | "query" => "search",
            "navigate"
            | "snapshot"
            | "read_page"
            | "click"
            | "double_click"
            | "triple_click"
            | "right_click"
            | "fill"
            | "form_input"
            | "type"
            | "screenshot"
            | "evaluate"
            | "close"
            | "list_tabs"
            | "new_tab"
            | "close_tab"
            | "back"
            | "go_back"
            | "forward"
            | "go_forward"
            | "scroll"
            | "scroll_to"
            | "hover"
            | "select"
            | "press"
            | "key"
            | "wait"
            | "drag"
            | "status"
            | "zoom"
            | "get_page_text"
            | "read_console_messages"
            | "console_messages"
            | "read_network_requests"
            | "network_requests"
            | "resize_window"
            | "resize"
            | "file_upload"
            | "upload_file"
            | "find"
            | "find_elements"
            | "browser_batch" => "browser",
            "console" | "source" | "storage" | "dom" | "cookies" | "performance" => "devtools",
            _ => "",
        }
    }

    async fn handle_http(&self, input: &serde_json::Value) -> ToolResult {
        let url = match input.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => return ToolResult::error("url is required for HTTP requests"),
        };

        // SSRF protection: block private IPs
        if is_private_url(url) {
            return ToolResult::error("Cannot fetch private/internal URLs (SSRF protection)");
        }

        // Sanitize action: fetch HTML, extract visible text, chunk for LLM context
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("fetch");
        if action == "sanitize" {
            let resp = match self.client.get(url).send().await {
                Ok(r) => r,
                Err(e) => return ToolResult::error(format!("HTTP request failed: {}", e)),
            };
            let status = resp.status().as_u16();
            let html = match resp.text().await {
                Ok(t) => t,
                Err(e) => return ToolResult::error(format!("Failed to read response body: {}", e)),
            };
            let clean = sanitize_html(&html);
            let max_chars = input
                .get("chunk_size")
                .and_then(|v| v.as_u64())
                .unwrap_or(4000) as usize;
            let chunks = chunk_text(&clean, max_chars);
            let total = chunks.len();
            let chunk_idx = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            if total == 0 {
                return ToolResult::ok(format!(
                    "HTTP {} — Status: {}\n\n(no visible text)",
                    url, status
                ));
            }
            let idx = chunk_idx.min(total - 1);
            return ToolResult::ok(format!(
                "HTTP {} — Status: {}\nChunk {}/{} ({} chars each)\n\n{}",
                url,
                status,
                idx + 1,
                total,
                max_chars,
                chunks[idx]
            ));
        }

        // Infer HTTP method from action name (get/post/put/delete/head) or explicit method param
        let method = input
            .get("method")
            .and_then(|v| v.as_str())
            .or_else(|| match action {
                "get" | "post" | "put" | "delete" | "head" => Some(action),
                _ => None,
            })
            .unwrap_or("GET")
            .to_uppercase();

        let mut req = match method.as_str() {
            "GET" => self.client.get(url),
            "POST" => self.client.post(url),
            "PUT" => self.client.put(url),
            "DELETE" => self.client.delete(url),
            "HEAD" => self.client.head(url),
            "PATCH" => self.client.patch(url),
            _ => return ToolResult::error(format!("Unsupported HTTP method: {}", method)),
        };

        // Add custom headers
        if let Some(headers) = input.get("headers").and_then(|v| v.as_object()) {
            for (key, value) in headers {
                if let Some(val) = value.as_str() {
                    if let (Ok(name), Ok(val)) = (
                        reqwest::header::HeaderName::from_bytes(key.as_bytes()),
                        reqwest::header::HeaderValue::from_str(val),
                    ) {
                        req = req.header(name, val);
                    }
                }
            }
        }

        // Add body
        if let Some(body) = input.get("body").and_then(|v| v.as_str()) {
            req = req.body(body.to_string());
        }

        match req.send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let content_type = resp
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();

                match resp.text().await {
                    Ok(body) => {
                        let display_body = if body.len() > 50_000 {
                            let raw_offset =
                                input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                            let chunk_size = 20_000;
                            // Snap both ends to char boundaries so multi-byte
                            // UTF-8 chars don't cause an index panic.
                            let offset = types::strutil::floor_char_boundary(&body, raw_offset);
                            let raw_end = (offset + chunk_size).min(body.len());
                            let end = types::strutil::floor_char_boundary(&body, raw_end);
                            let chunk = &body[offset..end];
                            format!(
                                "[Showing bytes {}-{} of {}]\n{}",
                                offset,
                                end,
                                body.len(),
                                chunk
                            )
                        } else {
                            // Strip HTML tags for readability if HTML
                            if content_type.contains("html") {
                                strip_html(&body)
                            } else {
                                body
                            }
                        };

                        ToolResult::ok(format!(
                            "HTTP {} {} — Status: {}\n\n{}",
                            method, url, status, display_body
                        ))
                    }
                    Err(e) => ToolResult::error(format!("Failed to read response body: {}", e)),
                }
            }
            Err(e) => ToolResult::error(format!("HTTP request failed: {}", e)),
        }
    }

    async fn handle_search(&self, input: &serde_json::Value, session_id: &str) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return ToolResult::error("query is required for search"),
        };

        // 1. Try BYOK API providers (check auth_profiles for search-* providers)
        if let Some(store) = &self.store {
            for provider in [
                "search-brave",
                "search-tavily",
                "search-google",
                "search-serpapi",
            ] {
                if let Ok(profiles) = store.list_active_auth_profiles_by_provider(provider) {
                    if let Some(profile) = profiles.first() {
                        match self
                            .search_via_api(
                                provider,
                                &profile.api_key,
                                query,
                                profile.metadata.as_deref().unwrap_or(""),
                            )
                            .await
                        {
                            Ok(results) if !results.is_empty() => {
                                return format_search_results(query, &results);
                            }
                            Err(e) => {
                                tracing::warn!(provider, error = %e, "BYOK search failed, trying next");
                            }
                            _ => {} // empty results, try next
                        }
                    }
                }
            }
        }

        // 2. Fallback: browser-based search (Chrome extension navigates to DuckDuckGo)
        tracing::info!(query, "no search API configured — trying browser search");
        let browser_result = self.search_via_browser(query, session_id).await;
        if !browser_result.is_error {
            return browser_result;
        }

        // 3. Final fallback: DuckDuckGo HTTP scraping (no browser needed)
        tracing::info!(
            query,
            "browser search failed — using DuckDuckGo HTTP scraping"
        );
        self.search_duckduckgo_html(query).await
    }

    /// Search via the user's browser — navigate to DuckDuckGo and read the results page.
    async fn search_via_browser(&self, query: &str, session_id: &str) -> ToolResult {
        let manager = match &self.browser {
            Some(m) => m,
            None => {
                // No browser — try DuckDuckGo HTTP scraping, then Brave
                return self.search_duckduckgo_html(query).await;
            }
        };

        let executor = match manager.executor() {
            Some(e) => e,
            None => {
                return self.search_duckduckgo_html(query).await;
            }
        };

        if !executor.is_connected() {
            let grace = std::time::Duration::from_secs(3);
            if !executor.was_recently_connected(grace).await
                || !executor.wait_for_connection(grace).await
            {
                self.broadcast_extension_disconnected("not_connected", session_id);
                return self.search_duckduckgo_html(query).await;
            }
        }

        // Navigate to DuckDuckGo search
        let search_url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );
        let nav_args = serde_json::json!({ "url": search_url });
        if let Err(e) = executor
            .execute("navigate", &nav_args, Some(session_id))
            .await
        {
            tracing::warn!(error = %e, "browser navigate failed, falling back to DDG scraping");
            return self.search_duckduckgo_html(query).await;
        }

        // Read the search results page
        let read_args = serde_json::json!({});
        match executor
            .execute("read_page", &read_args, Some(session_id))
            .await
        {
            Ok(result) => {
                let text = result
                    .get("pageContent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if text.is_empty() {
                    // DuckDuckGo HTML version failed too — try HTTP scraping as final fallback
                    tracing::warn!(
                        "browser read_page returned empty for DuckDuckGo, trying DDG HTTP scraping"
                    );
                    self.search_duckduckgo_html(query).await
                } else {
                    ToolResult::ok(format!("Search results for: {}\n\n{}", query, text))
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "read_page failed after DuckDuckGo search");
                self.search_duckduckgo_html(query).await
            }
        }
    }

    /// Dispatch to the correct BYOK search API provider.
    async fn search_via_api(
        &self,
        provider: &str,
        api_key: &str,
        query: &str,
        metadata: &str,
    ) -> Result<Vec<SearchResult>, String> {
        match provider {
            "search-brave" => self.search_brave_api(api_key, query).await,
            "search-tavily" => self.search_tavily(api_key, query).await,
            "search-google" => self.search_google_cse(api_key, query, metadata).await,
            "search-serpapi" => self.search_serpapi(api_key, query).await,
            _ => Err(format!("unknown search provider: {}", provider)),
        }
    }

    /// Brave Search API (requires X-Subscription-Token header).
    async fn search_brave_api(
        &self,
        api_key: &str,
        query: &str,
    ) -> Result<Vec<SearchResult>, String> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count=10",
            urlencoding::encode(query)
        );
        let resp = self
            .client
            .get(&url)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Brave API returned status {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_brave_api_results(&body))
    }

    /// Tavily Search API (api_key in JSON body).
    async fn search_tavily(&self, api_key: &str, query: &str) -> Result<Vec<SearchResult>, String> {
        let body = serde_json::json!({ "api_key": api_key, "query": query, "max_results": 10 });
        let resp = self
            .client
            .post("https://api.tavily.com/search")
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Tavily API returned status {}", resp.status()));
        }
        let result: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_tavily_results(&result))
    }

    /// Google Custom Search Engine API (key + cx params).
    async fn search_google_cse(
        &self,
        api_key: &str,
        query: &str,
        metadata: &str,
    ) -> Result<Vec<SearchResult>, String> {
        let cx = serde_json::from_str::<serde_json::Value>(metadata)
            .ok()
            .and_then(|m| m["cx"].as_str().map(String::from))
            .ok_or("Google CSE requires 'cx' in metadata")?;
        let url = format!(
            "https://www.googleapis.com/customsearch/v1?key={}&cx={}&q={}",
            api_key,
            cx,
            urlencoding::encode(query)
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Google CSE API returned status {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_google_cse_results(&body))
    }

    /// SerpAPI (api_key as query param).
    async fn search_serpapi(
        &self,
        api_key: &str,
        query: &str,
    ) -> Result<Vec<SearchResult>, String> {
        let url = format!(
            "https://serpapi.com/search.json?api_key={}&q={}&num=10",
            api_key,
            urlencoding::encode(query)
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("SerpAPI returned status {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_serpapi_results(&body))
    }

    /// Fallback: Brave HTML scraping (no API key needed).
    async fn search_brave_html(&self, query: &str) -> ToolResult {
        let search_url = format!(
            "https://search.brave.com/search?q={}",
            urlencoding::encode(query)
        );

        match self.client.get(&search_url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(html) => {
                    let results = parse_brave_results(&html);
                    if results.is_empty() {
                        ToolResult::ok(format!("No results found for: {}", query))
                    } else {
                        format_search_results(query, &results)
                    }
                }
                Err(e) => ToolResult::error(format!("Failed to read search response: {}", e)),
            },
            Err(e) => ToolResult::error(format!("Search request failed: {}", e)),
        }
    }

    /// Fallback: DuckDuckGo HTML scraping (no API key needed, no rate limits).
    async fn search_duckduckgo_html(&self, query: &str) -> ToolResult {
        let search_url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );

        match self.client.get(&search_url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(html) => {
                    let results = parse_duckduckgo_results(&html);
                    if results.is_empty() {
                        // Final fallback: try Brave
                        self.search_brave_html(query).await
                    } else {
                        format_search_results(query, &results)
                    }
                }
                Err(e) => ToolResult::error(format!("Failed to read DuckDuckGo response: {}", e)),
            },
            Err(e) => {
                tracing::warn!(error = %e, "DuckDuckGo scraping failed, falling back to Brave");
                self.search_brave_html(query).await
            }
        }
    }

    fn broadcast_extension_disconnected(&self, reason: &str, session_id: &str) {
        if let Some(ref broadcast) = self.broadcaster {
            broadcast(
                "browser_extension_disconnected",
                serde_json::json!({
                    "reason": reason,
                    "session_id": session_id,
                }),
            );
        }
    }

    async fn handle_browser(&self, input: &serde_json::Value, session_id: &str) -> ToolResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let manager = match &self.browser {
            Some(m) => m,
            None => {
                return ToolResult::error(
                    "Browser automation is not available. Use web(action: \"fetch\", url: \"...\") for HTTP requests instead.",
                );
            }
        };

        // Status works even when disconnected
        if action == "status" {
            let ext_connected = manager.extension_connected();
            let headless = manager.headless_available();
            let status = if ext_connected {
                "Browser extension connected. Ready. Use read_page to see the current page."
            } else if headless {
                "Headless browser available (agent-browser). Use read_page to see the current page."
            } else {
                "No browser backend available. Connect the Nebo Chrome/Brave extension \
                 or install agent-browser (`npm i -g agent-browser && agent-browser install`)."
            };
            return ToolResult::ok(format!(
                "Extension: {}, Headless: {}\n{}",
                ext_connected, headless, status
            ));
        }

        let executor = match manager.executor() {
            Some(e) => e,
            None => {
                return ToolResult::error("Browser automation not configured.");
            }
        };

        if !executor.is_connected() {
            let grace = std::time::Duration::from_secs(3);
            if executor.was_recently_connected(grace).await {
                if !executor.wait_for_connection(grace).await {
                    self.broadcast_extension_disconnected("reconnecting", session_id);
                    return ToolResult::error(
                        "Browser extension reconnecting — try again in a moment.",
                    );
                }
            } else {
                self.broadcast_extension_disconnected("not_connected", session_id);
                return ToolResult::error(
                    "No browser backend available. Connect the Nebo Chrome/Brave extension \
                     or install agent-browser (`npm i -g agent-browser && agent-browser install`).",
                );
            }
        }

        self.handle_browser_via_extension(&executor, action, input, Some(session_id))
            .await
    }

    /// Handle devtools actions via the Chrome extension (CDP bridge).
    async fn handle_devtools(&self, input: &serde_json::Value, session_id: &str) -> ToolResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let manager = match &self.browser {
            Some(m) => m,
            None => {
                return ToolResult::error(
                    "DevTools requires browser extension. Use web(action: \"status\") to check connection.",
                );
            }
        };

        let executor = match manager.executor() {
            Some(e) => e,
            None => {
                return ToolResult::error("Browser automation not configured.");
            }
        };

        if !executor.is_connected() {
            self.broadcast_extension_disconnected("not_connected", session_id);
            return ToolResult::error("Browser extension not connected.");
        }

        // Forward devtools actions to the extension's actual tool names
        let tool_name = match action {
            "console" => "read_console_messages",
            "source" | "storage" | "dom" | "cookies" | "performance" => {
                return ToolResult::error(format!(
                    "DevTools action '{}' is not yet available. Use web(action: \"console\") for console logs, \
                     or web(action: \"read_network_requests\") for network activity.",
                    action
                ));
            }
            _ => {
                return ToolResult::error(format!(
                    "Unknown devtools action '{}'. Available: console",
                    action
                ));
            }
        };

        // Translate devtools-style params to extension tool params
        let args = match action {
            "console" => {
                let mut a = serde_json::Map::new();
                // Map "filter" to "pattern" for backward compat
                if let Some(v) = input.get("filter") {
                    a.insert("pattern".to_string(), v.clone());
                }
                if let Some(v) = input.get("pattern") {
                    a.insert("pattern".to_string(), v.clone());
                }
                if let Some(v) = input.get("onlyErrors") {
                    a.insert("onlyErrors".to_string(), v.clone());
                }
                if let Some(v) = input.get("clear") {
                    a.insert("clear".to_string(), v.clone());
                }
                if let Some(v) = input.get("limit") {
                    a.insert("limit".to_string(), v.clone());
                }
                serde_json::Value::Object(a)
            }
            _ => build_extension_args(action, input),
        };
        match executor.execute(tool_name, &args, Some(session_id)).await {
            Ok(result) => {
                let text =
                    serde_json::to_string_pretty(&result).unwrap_or_else(|_| format!("{}", result));
                ToolResult::ok(text)
            }
            Err(e) => ToolResult::error(format!("DevTools action failed: {}", e)),
        }
    }

    /// Handle browser actions via the Chrome extension (native messaging).
    async fn handle_browser_via_extension(
        &self,
        executor: &browser::ActionExecutor,
        action: &str,
        input: &serde_json::Value,
        session_id: Option<&str>,
    ) -> ToolResult {
        // browser_batch: execute multiple actions in one round trip
        if action == "browser_batch" {
            let actions_val = match input.get("actions").and_then(|v| v.as_array()) {
                Some(a) if !a.is_empty() => a,
                _ => {
                    return ToolResult::error("browser_batch requires a non-empty 'actions' array");
                }
            };

            let mut batch_actions = Vec::new();
            for item in actions_val {
                let sub_action = match item.get("action").and_then(|v| v.as_str()) {
                    Some(a) => a,
                    None => {
                        return ToolResult::error(
                            "Each action in browser_batch must have an 'action' field",
                        );
                    }
                };
                let tool = match map_action_to_tool(sub_action) {
                    Some(t) => t,
                    None => {
                        return ToolResult::error(format!(
                            "browser_batch: unsupported action '{}'. Use individual tool calls for tab/console/network actions.",
                            sub_action
                        ));
                    }
                };
                let args = build_extension_args(sub_action, item);
                batch_actions.push(browser::BatchAction {
                    tool: tool.to_string(),
                    args,
                });
            }

            let opts = browser::BatchOptions {
                stop_on_error: true,
            };
            return match executor
                .batch_execute(batch_actions, opts, session_id)
                .await
            {
                Ok(results) => {
                    let mut parts = Vec::new();
                    for (i, result) in results.iter().enumerate() {
                        let action_name = actions_val
                            .get(i)
                            .and_then(|v| v.get("action"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        match result {
                            Ok(val) => {
                                let text = if let Some(pc) =
                                    val.get("pageContent").and_then(|v| v.as_str())
                                {
                                    pc.to_string()
                                } else if let Some(t) = val.get("text").and_then(|v| v.as_str()) {
                                    t.to_string()
                                } else {
                                    serde_json::to_string(val).unwrap_or_default()
                                };
                                parts.push(format!("[{}] {}: {}", i + 1, action_name, text));
                            }
                            Err(e) => {
                                parts.push(format!("[{}] {}: ERROR — {}", i + 1, action_name, e));
                            }
                        }
                    }
                    ToolResult::ok(parts.join("\n\n"))
                }
                Err(e) => ToolResult::error(format!("browser_batch failed: {}", e)),
            };
        }

        // Special cases that need validation before mapping
        if action == "new_tab" {
            let url = input.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() || url == "about:blank" {
                return ToolResult::error(
                    "new_tab requires a URL. Use navigate to change the current tab, \
                     or new_tab with a specific URL.",
                );
            }
        }
        if action == "status" {
            return ToolResult::ok(
                "Extension connected: true\nUse read_page to see the current page.".to_string(),
            );
        }

        // Map action names to extension tool names
        let tool_name = match map_action_to_tool(action) {
            Some(t) => t,
            None => {
                return ToolResult::error(format!(
                    "Browser action '{}' is not supported via extension. Available: navigate, read_page, click, double_click, triple_click, right_click, hover, fill, form_input, type, select, screenshot, scroll, scroll_to, press, drag, go_back, go_forward, wait, evaluate, list_tabs, zoom, get_page_text, read_console_messages, read_network_requests, resize_window, file_upload, find, browser_batch",
                    action
                ));
            }
        };

        // Build args for the extension tool
        let args = build_extension_args(action, input);

        // Execute with auto-retry for read_page character limit errors.
        // The extension (at parity with Claude) returns an error when output > maxChars.
        // Nebo handles this by retrying with tighter params so the agent always gets content.
        let result = executor.execute(tool_name, &args, session_id).await;

        // read_page character limit retry: depth 5 → depth 3 → filter interactive
        if action == "snapshot" || action == "read_page" {
            if let Err(ref e) = result {
                let err_msg = e.to_string();
                if err_msg.contains("character limit") || err_msg.contains("Output exceeds") {
                    let retries: Vec<serde_json::Value> = vec![
                        serde_json::json!({"depth": 5, "filter": null, "maxChars": 50000}),
                        serde_json::json!({"depth": 3, "filter": null, "maxChars": 50000}),
                        serde_json::json!({"filter": "interactive", "maxChars": 50000}),
                    ];
                    for retry_override in &retries {
                        let mut retry_args = args.clone();
                        if let (Some(obj), Some(overrides)) =
                            (retry_args.as_object_mut(), retry_override.as_object())
                        {
                            for (k, v) in overrides {
                                if v.is_null() {
                                    obj.remove(k);
                                } else {
                                    obj.insert(k.clone(), v.clone());
                                }
                            }
                        }
                        if let Ok(retry_result) =
                            executor.execute(tool_name, &retry_args, session_id).await
                        {
                            let page_content = retry_result
                                .get("pageContent")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !page_content.is_empty() {
                                let sid = session_id.unwrap_or("default");
                                if let Some(spilled) =
                                    maybe_spill_large_result(page_content, input, sid, action)
                                {
                                    return spilled;
                                }
                                return ToolResult {
                                    content: page_content.to_string(),
                                    is_error: false,
                                    image_url: None,
                                };
                            }
                        }
                    }
                }
            }
        }

        match result {
            Ok(result) => {
                // Check for post-action screenshot in result: { text: "...", screenshot: { data, format } }
                let (mut text_result, screenshot_b64) =
                    if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
                        let screenshot = result.get("screenshot").and_then(|s| {
                            let data = s.get("data")?.as_str()?;
                            let fmt = s.get("format").and_then(|f| f.as_str()).unwrap_or("jpeg");
                            Some(format!("data:image/{};base64,{}", fmt, data))
                        });
                        (text.to_string(), screenshot)
                    } else if action == "snapshot" || action == "read_page" {
                        let page_content = result
                            .get("pageContent")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        (page_content.to_string(), None)
                    } else {
                        let s = serde_json::to_string_pretty(&result)
                            .unwrap_or_else(|_| format!("{}", result));
                        (s, None)
                    };

                // Auto-snapshot after navigate: return a compact interactive snapshot
                // so the model can act immediately without a separate read_page call.
                if action == "navigate" {
                    let snap_args = serde_json::json!({"filter": "interactive"});
                    match executor.execute("read_page", &snap_args, session_id).await {
                        Ok(snap_result) => {
                            let snapshot_text = snap_result
                                .get("pageContent")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if !snapshot_text.is_empty() {
                                let truncated = truncate_snapshot(snapshot_text, 8000);
                                text_result = format!(
                                    "{}\n\n## Page Snapshot (interactive elements)\n{}",
                                    text_result, truncated
                                );
                            }
                        }
                        Err(_) => {} // Snapshot failed — navigate still succeeded
                    }
                }

                // Auto-spill large results to file for evaluate, read_page, get_page_text
                if matches!(
                    action,
                    "evaluate" | "snapshot" | "read_page" | "get_page_text"
                ) {
                    let sid = session_id.unwrap_or("default");
                    if let Some(spilled) =
                        maybe_spill_large_result(&text_result, input, sid, action)
                    {
                        return ToolResult {
                            content: spilled.content,
                            is_error: false,
                            image_url: screenshot_b64,
                        };
                    }
                }

                ToolResult {
                    content: text_result,
                    is_error: false,
                    image_url: screenshot_b64,
                }
            }
            Err(e) => ToolResult::error(friendly_browser_error(action, &e.to_string())),
        }
    }
}

impl DynTool for WebTool {
    fn name(&self) -> &str {
        "web"
    }

    fn description(&self) -> String {
        "Web operations — HTTP requests, search, browser automation, and devtools.\n\n\
         Decision: API response or static HTML → fetch/search. Rendered page, interaction, or user sessions → browser.\n\n\
         ## HTTP & Search\n\
         fetch, sanitize, search — see parameter descriptions in schema.\n\n\
         ## Browser\n\
         Controls the user's real Chrome browser. navigate returns a compact page snapshot automatically.\n\n\
         Reading: read_page (accessibility tree with refs like [ref_1]), get_page_text, find, screenshot, zoom\n\
         Interaction: click, fill, type, select, press, scroll, hover, drag, file_upload, evaluate, wait\n\
         Navigation: navigate, new_tab, close_tab, list_tabs, go_back, go_forward\n\
         Batching: browser_batch — chain 2+ predictable steps in one call (e.g. click + type + press Enter)\n\
         Debugging: read_console_messages, read_network_requests, resize_window\n\
         DevTools: console, source, storage, dom, cookies, performance\n\n\
         ## CRITICAL — Browse Like a Human\n\
         - read_page first to see elements before clicking. Scroll + read_page to find more content.\n\
         - browser_batch for predictable multi-step sequences (faster, fewer round trips).\n\
         - If fill fails on React/modern sites: click → press(key: \"cmd+a\") → type.\n\
         - Do NOT click file upload buttons (opens native picker). Use file_upload with ref instead.\n\
         - NEVER navigate to URLs with search query params (anti-bot). Use the site's search box.\n\
         - Always cite source URLs when reporting web results."
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "REQUIRED. The web resource category — determines which actions are available.",
                    "enum": ["http", "search", "browser", "devtools"]
                },
                "action": {
                    "type": "string",
                    "description": "The operation to perform on the selected resource. Never put a resource name here.",
                    "enum": ["fetch", "get", "post", "put", "delete", "head", "sanitize",
                             "search",
                             "navigate", "read_page", "click", "double_click",
                             "triple_click", "right_click", "hover", "fill", "form_input",
                             "type", "select", "screenshot", "scroll", "scroll_to", "press",
                             "drag", "go_back", "go_forward", "wait", "evaluate",
                             "list_tabs", "new_tab", "close_tab", "status",
                             "zoom", "get_page_text", "read_console_messages",
                             "read_network_requests", "resize_window", "file_upload", "find",
                             "browser_batch",
                             "console", "source", "storage", "dom", "cookies", "performance"]
                },
                "url": {
                    "type": "string",
                    "description": "URL for HTTP request or browser navigation"
                },
                "method": {
                    "type": "string",
                    "description": "HTTP method (GET, POST, PUT, DELETE, HEAD, PATCH)"
                },
                "headers": {
                    "type": "object",
                    "description": "HTTP headers as key-value pairs"
                },
                "body": {
                    "type": "string",
                    "description": "HTTP request body"
                },
                "query": {
                    "type": "string",
                    "description": "Search query"
                },
                "offset": {
                    "type": "integer",
                    "description": "Byte offset for paginated content"
                },
                "ref": {
                    "type": "string",
                    "description": "Element reference from read_page output (e.g. ref_1, ref_2)"
                },
                "selector": {
                    "type": "string",
                    "description": "CSS selector for browser operations"
                },
                "value": {
                    "type": ["string", "boolean", "number"],
                    "description": "Value for fill/select operations. For checkboxes use true/false, for selects use option value or text, for other inputs use string/number."
                },
                "text": {
                    "type": "string",
                    "description": "Text to type character by character"
                },
                "key": {
                    "type": "string",
                    "description": "Key name for press (Enter, Tab, Escape, etc.)"
                },
                "filter": {
                    "type": "string",
                    "description": "Filter mode for read_page: all (default) or interactive",
                    "enum": ["all", "interactive"]
                },
                "direction": {
                    "type": "string",
                    "description": "Scroll direction: up, down, left, right",
                    "enum": ["up", "down", "left", "right"]
                },
                "expression": {
                    "type": "string",
                    "description": "JavaScript expression for evaluate"
                },
                "output": {
                    "type": "string",
                    "description": "Output mode for evaluate/read_page/get_page_text: inline (always return full text), artifact (always save to file), auto (save to file if large, default)",
                    "enum": ["inline", "artifact", "auto"]
                },
                "max_inline_chars": {
                    "type": "integer",
                    "description": "For auto output mode: inline threshold in chars (default 8000). Results larger than this are saved to file."
                },
                "preview_chars": {
                    "type": "integer",
                    "description": "For artifact output: chars of preview to include in response (default 1200)"
                },
                "depth": {
                    "type": "integer",
                    "description": "Max tree depth for read_page (default 15). Use smaller values for large pages."
                },
                "maxChars": {
                    "type": "integer",
                    "description": "Max output characters for read_page. Omit for no limit."
                },
                "refId": {
                    "type": "string",
                    "description": "Element ref to read subtree from (e.g. ref_3). For read_page only."
                },
                "ms": {
                    "type": "integer",
                    "description": "Milliseconds to wait (for wait action, max 10000)"
                },
                "amount": {
                    "type": "integer",
                    "description": "Scroll amount in ticks (default 3, 100px per tick)"
                },
                "coordinate": {
                    "type": "array",
                    "items": { "type": "number" },
                    "description": "[x, y] coordinates for click/scroll actions (alternative to ref)"
                },
                "modifiers": {
                    "type": "string",
                    "description": "Modifier keys for click: ctrl, shift, alt, cmd. Combine with + (e.g. ctrl+shift)"
                },
                "repeat": {
                    "type": "integer",
                    "description": "Number of times to repeat key sequence (for press, default 1, max 100)"
                },
                "start_coordinate": {
                    "type": "array",
                    "items": { "type": "number" },
                    "description": "[x, y] start coordinates for drag action"
                },
                "duration": {
                    "type": "number",
                    "description": "Seconds to wait (for wait action, max 30)"
                },
                "chunk_size": {
                    "type": "integer",
                    "description": "Max characters per chunk for sanitize (default 4000)"
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Max output characters for get_page_text (default 50000)"
                },
                "onlyErrors": {
                    "type": "boolean",
                    "description": "For read_console_messages: only return error/exception messages (default false)"
                },
                "clear": {
                    "type": "boolean",
                    "description": "For read_console_messages/read_network_requests: clear after reading (default false)"
                },
                "pattern": {
                    "type": "string",
                    "description": "For read_console_messages: regex pattern to filter messages"
                },
                "limit": {
                    "type": "integer",
                    "description": "For read_console_messages/read_network_requests: max results (default 100)"
                },
                "urlPattern": {
                    "type": "string",
                    "description": "For read_network_requests: URL substring to filter requests"
                },
                "width": {
                    "type": "number",
                    "description": "For resize_window: target window width in pixels"
                },
                "height": {
                    "type": "number",
                    "description": "For resize_window: target window height in pixels"
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "For file_upload: absolute file paths to upload"
                },
                "query": {
                    "type": "string",
                    "description": "For find: natural language description of elements to find"
                },
                "region": {
                    "type": "array",
                    "items": { "type": "number" },
                    "minItems": 4,
                    "maxItems": 4,
                    "description": "For zoom: [x0, y0, x1, y1] rectangle from top-left to bottom-right in viewport pixels"
                },
                "force": {
                    "type": "boolean",
                    "description": "For navigate: force navigation past 'Leave site?' dialogs (default false)"
                },
                "actions": {
                    "type": "array",
                    "description": "For browser_batch: list of actions to execute sequentially in one round trip. Each item is an object with 'action' plus that action's normal params. Stops on first error.",
                    "items": {
                        "type": "object"
                    }
                }
            },
            "required": ["resource", "action"]
        })
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn resource_permit(&self, input: &serde_json::Value) -> Option<ResourceKind> {
        let resource = input.get("resource").and_then(|v| v.as_str()).unwrap_or("");
        let resource = if resource.is_empty() {
            let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");
            self.infer_resource(action)
        } else {
            resource
        };
        match resource {
            "browser" | "devtools" => Some(ResourceKind::Browser),
            // http, search are parallelizable
            _ => None,
        }
    }

    fn is_concurrent_safe(&self, _input: &serde_json::Value) -> bool {
        // Web operations are read-only by nature (fetch, search, browse).
        true
    }

    fn execute_dyn<'a>(
        &'a self,
        ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}", e)),
            };

            let mut input = input;
            let resource = {
                let corrected = crate::domain::auto_correct_resource(
                    &domain_input,
                    &mut input,
                    &["http", "search", "browser", "devtools"],
                );
                if corrected.is_empty() {
                    self.infer_resource(&domain_input.action).to_string()
                } else {
                    corrected
                }
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: http, search, browser, devtools",
                );
            }

            let session_id = &ctx.session_id;

            // Signal the extension to show visual indicators for this agent's tab group
            if matches!(resource.as_str(), "browser" | "search" | "devtools") {
                if let Some(ref mgr) = self.browser {
                    if let Some(executor) = mgr.executor() {
                        executor
                            .send_command("show_indicators", Some(session_id))
                            .await;
                    }
                }
            }

            match resource.as_str() {
                "http" => self.handle_http(&input).await,
                "search" => self.handle_search(&input, session_id).await,
                "browser" => self.handle_browser(&input, session_id).await,
                "devtools" => self.handle_devtools(&input, session_id).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: http, search, browser, devtools",
                    other
                )),
            }
        })
    }
}

/// Save large tool output to a file, returning a compact ToolResult with metadata + preview.
///
/// Follows the same directory tree and replacement format as `large_input.rs` so the agent
/// already knows how to read the full content back via `system(resource: "file", action: "read")`.
fn save_large_result(
    content: &str,
    session_id: &str,
    action: &str,
    preview_chars: usize,
) -> ToolResult {
    let data_dir = match config::data_dir() {
        Ok(d) => d,
        Err(_) => return ToolResult::ok(content.to_string()), // fallback: inline
    };

    let dir = data_dir
        .join("files")
        .join("large_outputs")
        .join(session_id);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!(error = %e, "failed to create large_outputs dir, returning inline");
        return ToolResult::ok(content.to_string());
    }

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let filename = format!("{}_{}.txt", action, ts);
    let path = dir.join(&filename);

    if let Err(e) = std::fs::write(&path, content) {
        tracing::warn!(error = %e, "failed to write large output, returning inline");
        return ToolResult::ok(content.to_string());
    }

    let path_str = path.display().to_string();
    let preview_end = content
        .char_indices()
        .nth(preview_chars)
        .map(|(i, _)| i)
        .unwrap_or(content.len());
    let preview = &content[..preview_end];
    let ellipsis = if preview_end < content.len() {
        "\n..."
    } else {
        ""
    };

    tracing::info!(
        path = %path_str,
        chars = content.len(),
        action,
        "large browser output saved to file"
    );

    ToolResult::ok(format!(
        "Result saved to file ({} chars). Preview below.\n\n\
         Path: {}\n\
         Read full content with: system(resource: \"file\", action: \"read\", path: \"{}\")\n\n\
         {}{}",
        content.len(),
        path_str,
        path_str,
        preview,
        ellipsis,
    ))
}

/// If the result exceeds the inline threshold, spill to file and return compact metadata.
/// Respects `output` mode from input: "inline" (always inline), "artifact" (always file),
/// "auto" (file if over threshold). Default is "auto" for evaluate/read_page/get_page_text.
fn maybe_spill_large_result(
    content: &str,
    input: &serde_json::Value,
    session_id: &str,
    action: &str,
) -> Option<ToolResult> {
    let output_mode = input
        .get("output")
        .and_then(|v| v.as_str())
        .unwrap_or("auto");

    match output_mode {
        "inline" => None, // caller returns content as-is
        "artifact" => {
            let preview_chars = input
                .get("preview_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(DEFAULT_PREVIEW_CHARS as u64) as usize;
            Some(save_large_result(
                content,
                session_id,
                action,
                preview_chars,
            ))
        }
        _ => {
            // "auto": spill only if over threshold
            let max_inline = input
                .get("max_inline_chars")
                .and_then(|v| v.as_u64())
                .unwrap_or(LARGE_OUTPUT_THRESHOLD as u64) as usize;
            if content.len() <= max_inline {
                None
            } else {
                let preview_chars = input
                    .get("preview_chars")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(DEFAULT_PREVIEW_CHARS as u64)
                    as usize;
                Some(save_large_result(
                    content,
                    session_id,
                    action,
                    preview_chars,
                ))
            }
        }
    }
}

/// Map a web tool action name to the corresponding extension tool name.
/// Returns None for actions that don't map (status, new_tab validation, etc.)
fn map_action_to_tool(action: &str) -> Option<&'static str> {
    match action {
        "snapshot" | "read_page" => Some("read_page"),
        "navigate" => Some("navigate"),
        "click" => Some("click"),
        "double_click" => Some("double_click"),
        "triple_click" => Some("triple_click"),
        "right_click" => Some("right_click"),
        "hover" => Some("hover"),
        "fill" | "form_input" => Some("form_input"),
        "type" => Some("type"),
        "select" => Some("select"),
        "screenshot" => Some("screenshot"),
        "scroll" => Some("scroll"),
        "scroll_to" => Some("scroll_to"),
        "press" | "key" => Some("press"),
        "drag" => Some("drag"),
        "back" | "go_back" => Some("go_back"),
        "forward" | "go_forward" => Some("go_forward"),
        "wait" => Some("wait"),
        "evaluate" => Some("evaluate"),
        "list_tabs" => Some("list_tabs"),
        "new_tab" => Some("new_tab"),
        "close_tab" | "close" => Some("close_tab"),
        "zoom" => Some("zoom"),
        "get_page_text" => Some("get_page_text"),
        "read_console_messages" | "console_messages" => Some("read_console_messages"),
        "read_network_requests" | "network_requests" => Some("read_network_requests"),
        "resize_window" | "resize" => Some("resize_window"),
        "file_upload" | "upload_file" => Some("file_upload"),
        "find" | "find_elements" => Some("find"),
        _ => None,
    }
}

/// Build extension tool arguments from the web tool input.
fn build_extension_args(action: &str, input: &serde_json::Value) -> serde_json::Value {
    let mut args = serde_json::Map::new();

    // Forward common parameters
    let forward_keys = match action {
        "navigate" => vec!["url", "force"],
        "new_tab" => vec!["url"],
        "click" | "double_click" | "triple_click" | "right_click" => {
            vec!["ref", "selector", "coordinate", "x", "y", "modifiers"]
        }
        "hover" => vec!["ref", "coordinate", "x", "y"],
        "fill" | "form_input" => vec!["ref", "selector", "value"],
        "type" => vec!["text"],
        "select" => vec!["ref", "selector", "value"],
        "scroll" => vec![
            "direction",
            "amount",
            "scroll_direction",
            "scroll_amount",
            "coordinate",
        ],
        "scroll_to" => vec!["ref"],
        "press" | "key" => vec!["key", "text", "repeat"],
        "drag" => vec!["start_coordinate", "coordinate"],
        "wait" => vec!["ms", "duration"],
        "evaluate" => vec!["expression", "text"],
        "snapshot" | "read_page" => vec!["filter", "depth", "maxChars", "refId"],
        "close_tab" | "close" => vec!["tabId", "tabIds"],
        "zoom" => vec!["region"],
        "get_page_text" => vec!["max_chars"],
        "read_console_messages" | "console_messages" => {
            vec!["onlyErrors", "clear", "pattern", "limit"]
        }
        "read_network_requests" | "network_requests" => vec!["urlPattern", "clear", "limit"],
        "resize_window" | "resize" => vec!["width", "height"],
        "file_upload" | "upload_file" => vec!["paths", "ref"],
        "find" | "find_elements" => vec!["query"],
        // DevTools actions — forward url, selector, expression, and filter params
        "console" | "source" | "storage" | "dom" | "cookies" | "performance" => {
            vec!["url", "selector", "expression", "filter"]
        }
        _ => vec![],
    };

    for key in forward_keys {
        if let Some(val) = input.get(key) {
            args.insert(key.to_string(), val.clone());
        }
    }

    serde_json::Value::Object(args)
}

/// Truncate a snapshot at a line boundary, appending an omission note.
/// Used by auto-snapshot after navigate to keep output compact.
fn truncate_snapshot(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    // Char-boundary safe: snap to a valid boundary so &text[..safe_max] never panics.
    let safe_max = types::strutil::floor_char_boundary(text, max_chars);
    let truncated = &text[..safe_max];
    let last_newline = truncated.rfind('\n').unwrap_or(safe_max);
    let clean = &text[..last_newline];
    let omitted = text.len() - last_newline;
    format!(
        "{}\n\n[...{} chars omitted. Use read_page for full content.]",
        clean, omitted
    )
}

/// Map raw browser errors to AI-friendly messages with recovery suggestions.
fn friendly_browser_error(action: &str, raw_error: &str) -> String {
    let suggestion = if raw_error.contains("Timeout") || raw_error.contains("timeout") {
        "The page may still be loading. Try read_page to check current state, or wait and retry."
    } else if raw_error.contains("not found")
        || raw_error.contains("No element")
        || raw_error.contains("no element")
    {
        "Element not found on page. Use read_page to get current page elements and their refs."
    } else if raw_error.contains("not connected") || raw_error.contains("disconnected") {
        "Browser disconnected. Check web(action: \"status\") and retry."
    } else if raw_error.contains("intercept") || raw_error.contains("overlay") {
        "Click was intercepted by an overlay/popup. Try closing it first, or click a different element."
    } else if raw_error.contains("navigation") || raw_error.contains("net::ERR") {
        "Navigation failed. The URL may be invalid or the site may be down. Verify the URL and retry."
    } else {
        "Try read_page to see current page state and adjust your approach."
    };
    format!("{} failed: {}. Recovery: {}", action, raw_error, suggestion)
}

/// Simple SSRF check: block private/loopback IPs.
fn is_private_url(url: &str) -> bool {
    let lower = url.to_lowercase();
    // Block obvious private addresses
    lower.contains("://localhost")
        || lower.contains("://127.")
        || lower.contains("://0.")
        || lower.contains("://10.")
        || lower.contains("://172.16.")
        || lower.contains("://172.17.")
        || lower.contains("://172.18.")
        || lower.contains("://172.19.")
        || lower.contains("://172.2")
        || lower.contains("://172.30.")
        || lower.contains("://172.31.")
        || lower.contains("://192.168.")
        || lower.contains("://[::1]")
        || lower.contains("://169.254.")
}

/// Strip HTML tags for readable text output.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_space = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        if !in_tag && chars[i] == '<' {
            in_tag = true;
            // Check for script/style tags
            let remaining: String = lower_chars[i..].iter().take(10).collect();
            if remaining.starts_with("<script") {
                in_script = true;
            } else if remaining.starts_with("<style") {
                in_style = true;
            } else if remaining.starts_with("</script") {
                in_script = false;
            } else if remaining.starts_with("</style") {
                in_style = false;
            }
        } else if in_tag && chars[i] == '>' {
            in_tag = false;
        } else if !in_tag && !in_script && !in_style {
            let ch = chars[i];
            if ch.is_whitespace() {
                if !last_was_space {
                    result.push(' ');
                    last_was_space = true;
                }
            } else {
                result.push(ch);
                last_was_space = false;
            }
        }
        i += 1;
    }

    // Decode common HTML entities
    result
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

struct SearchResult {
    title: String,
    url: String,
    snippet: String,
}

/// Format search results into a ToolResult.
fn format_search_results(query: &str, results: &[SearchResult]) -> ToolResult {
    let formatted: Vec<String> = results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("{}. {}\n   {}\n   {}", i + 1, r.title, r.url, r.snippet))
        .collect();
    ToolResult::ok(format!(
        "Search results for: {}\n\n{}",
        query,
        formatted.join("\n\n")
    ))
}

/// Parse Brave Search API JSON response.
fn parse_brave_api_results(body: &serde_json::Value) -> Vec<SearchResult> {
    let empty = vec![];
    let results = body
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
        .unwrap_or(&empty);
    results
        .iter()
        .filter_map(|r| {
            let title = r.get("title").and_then(|v| v.as_str())?;
            let url = r.get("url").and_then(|v| v.as_str())?;
            let snippet = r.get("description").and_then(|v| v.as_str()).unwrap_or("");
            Some(SearchResult {
                title: title.to_string(),
                url: url.to_string(),
                snippet: snippet.to_string(),
            })
        })
        .take(10)
        .collect()
}

/// Parse Tavily Search API JSON response.
fn parse_tavily_results(body: &serde_json::Value) -> Vec<SearchResult> {
    let empty = vec![];
    let results = body
        .get("results")
        .and_then(|r| r.as_array())
        .unwrap_or(&empty);
    results
        .iter()
        .filter_map(|r| {
            let title = r.get("title").and_then(|v| v.as_str())?;
            let url = r.get("url").and_then(|v| v.as_str())?;
            let snippet = r.get("content").and_then(|v| v.as_str()).unwrap_or("");
            Some(SearchResult {
                title: title.to_string(),
                url: url.to_string(),
                snippet: snippet.to_string(),
            })
        })
        .take(10)
        .collect()
}

/// Parse Google Custom Search Engine API JSON response.
fn parse_google_cse_results(body: &serde_json::Value) -> Vec<SearchResult> {
    let empty = vec![];
    let items = body
        .get("items")
        .and_then(|r| r.as_array())
        .unwrap_or(&empty);
    items
        .iter()
        .filter_map(|r| {
            let title = r.get("title").and_then(|v| v.as_str())?;
            let url = r.get("link").and_then(|v| v.as_str())?;
            let snippet = r.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
            Some(SearchResult {
                title: title.to_string(),
                url: url.to_string(),
                snippet: snippet.to_string(),
            })
        })
        .take(10)
        .collect()
}

/// Parse SerpAPI JSON response.
fn parse_serpapi_results(body: &serde_json::Value) -> Vec<SearchResult> {
    let empty = vec![];
    let results = body
        .get("organic_results")
        .and_then(|r| r.as_array())
        .unwrap_or(&empty);
    results
        .iter()
        .filter_map(|r| {
            let title = r.get("title").and_then(|v| v.as_str())?;
            let url = r.get("link").and_then(|v| v.as_str())?;
            let snippet = r.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
            Some(SearchResult {
                title: title.to_string(),
                url: url.to_string(),
                snippet: snippet.to_string(),
            })
        })
        .take(10)
        .collect()
}

/// Parse Brave Search HTML results.
fn parse_brave_results(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Brave wraps each result in a div with id="N-web" or similar data attributes.
    // Titles are in <div class="title search-snippet-title ...">
    // URLs are in <cite class="snippet-url ...">
    // Descriptions are in <div class="snippet-description ...">
    // We split by snippet-url to isolate each result block.
    let chunks: Vec<&str> = html.split("snippet-url").collect();

    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 || results.len() >= 10 {
            continue;
        }

        // Extract URL from the cite tag content (e.g., "neboai.com › blog")
        // But we need the actual href — look for href in the nearby anchor.
        // The cite contains display URL; the actual link is in the title's parent <a>.
        let display_url = extract_between(chunk, ">", "<")
            .map(|s| strip_html(&s).trim().to_string())
            .unwrap_or_default();

        // Look for the title in the next chunk section (search-snippet-title)
        let title = if let Some(title_chunk) = chunk.split("search-snippet-title").nth(1) {
            extract_between(title_chunk, ">", "</div>")
                .map(|s| strip_html(&s).trim().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };

        // Look for description (snippet-description)
        let snippet = if let Some(desc_chunk) = chunk.split("snippet-description").nth(1) {
            extract_between(desc_chunk, ">", "</div>")
                .map(|s| strip_html(&s).trim().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };

        // Extract actual href from nearby anchor tag
        let url = extract_attr_forward(chunk, "href")
            .or_else(|| {
                // Try to reconstruct URL from display URL
                let clean = display_url.replace(" › ", "/");
                if !clean.is_empty() && !clean.contains(' ') {
                    Some(format!("https://{}", clean))
                } else {
                    None
                }
            })
            .unwrap_or_default();

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }

    results
}

/// Parse DuckDuckGo HTML lite results.
/// DDG HTML lite page has results in <a class="result__a" href="...">title</a>
/// and snippets in <a class="result__snippet" ...>description</a>.
fn parse_duckduckgo_results(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // Split by "result__a" class which marks each result link
    let chunks: Vec<&str> = html.split("result__a").collect();

    for (i, chunk) in chunks.iter().enumerate() {
        if i == 0 || results.len() >= 10 {
            continue;
        }

        // Extract href from the result link
        let url = extract_attr_forward(chunk, "href")
            .map(|u| {
                // DDG wraps URLs in redirect: //duckduckgo.com/l/?uddg=ENCODED_URL
                if let Some(uddg_idx) = u.find("uddg=") {
                    let encoded = &u[uddg_idx + 5..];
                    let end = encoded.find('&').unwrap_or(encoded.len());
                    urlencoding::decode(&encoded[..end])
                        .map(|s| s.into_owned())
                        .unwrap_or(u)
                } else {
                    u
                }
            })
            .unwrap_or_default();

        // Title is the text content of the <a> tag
        let title = extract_between(chunk, ">", "</a>")
            .map(|s| strip_html(&s).trim().to_string())
            .unwrap_or_default();

        // Snippet is in the nearby result__snippet
        let snippet = if let Some(snip_chunk) = chunk.split("result__snippet").nth(1) {
            extract_between(snip_chunk, ">", "</a>")
                .or_else(|| extract_between(snip_chunk, ">", "</"))
                .map(|s| strip_html(&s).trim().to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };

        if !title.is_empty() && !url.is_empty() && url.starts_with("http") {
            results.push(SearchResult {
                title,
                url,
                snippet,
            });
        }
    }

    results
}

/// Extract the first href="..." value found in a chunk.
fn extract_attr_forward(html: &str, attr: &str) -> Option<String> {
    let pattern = format!("{}=\"", attr);
    let idx = html.find(&pattern)?;
    let after = &html[idx + pattern.len()..];
    let end = after.find('"')?;
    let val = &after[..end];
    // Skip javascript: and # links
    if val.starts_with("http") {
        Some(val.to_string())
    } else {
        None
    }
}

fn extract_between(html: &str, start_marker: &str, end_marker: &str) -> Option<String> {
    let start_idx = html.find(start_marker)?;
    let after_start = &html[start_idx + start_marker.len()..];
    // Skip to first >
    let gt_idx = after_start.find('>')?;
    let content_start = &after_start[gt_idx + 1..];
    let end_idx = content_start.find(end_marker)?;
    Some(content_start[..end_idx].to_string())
}

/// Extract visible text from HTML, stripping tags, scripts, styles,
/// and collapsing blank lines.
fn sanitize_html(html: &str) -> String {
    let stripped = strip_html(html);
    stripped
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Chunk text into LLM-friendly segments by line boundaries.
fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = if max_chars == 0 { 4000 } else { max_chars };
    let mut chunks = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if current.len() + line.len() + 1 > max_chars && !current.is_empty() {
            chunks.push(current.clone());
            current.clear();
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}
