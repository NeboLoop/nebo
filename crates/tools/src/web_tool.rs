use std::sync::Arc;

use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ResourceKind, ToolResult};

/// WebTool consolidates web operations: HTTP fetch, search, and browser automation.
pub struct WebTool {
    client: reqwest::Client,
    browser: Option<Arc<browser::Manager>>,
    store: Option<Arc<db::Store>>,
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

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "fetch" | "get" | "post" | "put" | "delete" | "head" | "sanitize" => "http",
            "search" | "query" => "search",
            "navigate" | "snapshot" | "read_page" | "click" | "double_click" | "triple_click"
            | "right_click" | "fill" | "form_input" | "type" | "screenshot" | "evaluate"
            | "launch" | "close" | "list_pages" | "list_tabs" | "new_tab" | "close_tab"
            | "back" | "go_back" | "forward" | "go_forward" | "reload" | "scroll" | "scroll_to"
            | "hover" | "select" | "press" | "key" | "wait" | "drag" | "status" | "text" => {
                "browser"
            }
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
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("fetch");
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
            let chunk_idx = input
                .get("offset")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            if total == 0 {
                return ToolResult::ok(format!("HTTP {} — Status: {}\n\n(no visible text)", url, status));
            }
            let idx = chunk_idx.min(total - 1);
            return ToolResult::ok(format!(
                "HTTP {} — Status: {}\nChunk {}/{} ({} chars each)\n\n{}",
                url, status, idx + 1, total, max_chars, chunks[idx]
            ));
        }

        let method = input
            .get("method")
            .and_then(|v| v.as_str())
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
                            let offset = input
                                .get("offset")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize;
                            let chunk_size = 20_000;
                            let end = (offset + chunk_size).min(body.len());
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

    async fn handle_search(&self, input: &serde_json::Value) -> ToolResult {
        let query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => return ToolResult::error("query is required for search"),
        };

        // 1. Try BYOK API providers (check auth_profiles for search-* providers)
        if let Some(store) = &self.store {
            for provider in ["search-brave", "search-tavily", "search-google", "search-serpapi"] {
                if let Ok(profiles) = store.list_active_auth_profiles_by_provider(provider) {
                    if let Some(profile) = profiles.first() {
                        match self.search_via_api(provider, &profile.api_key, query, profile.metadata.as_deref().unwrap_or("")).await {
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

        // 2. Fallback: Brave HTML scraping (no API key needed)
        self.search_brave_html(query).await
    }

    /// Dispatch to the correct BYOK search API provider.
    async fn search_via_api(&self, provider: &str, api_key: &str, query: &str, metadata: &str) -> Result<Vec<SearchResult>, String> {
        match provider {
            "search-brave" => self.search_brave_api(api_key, query).await,
            "search-tavily" => self.search_tavily(api_key, query).await,
            "search-google" => self.search_google_cse(api_key, query, metadata).await,
            "search-serpapi" => self.search_serpapi(api_key, query).await,
            _ => Err(format!("unknown search provider: {}", provider)),
        }
    }

    /// Brave Search API (requires X-Subscription-Token header).
    async fn search_brave_api(&self, api_key: &str, query: &str) -> Result<Vec<SearchResult>, String> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count=10",
            urlencoding::encode(query)
        );
        let resp = self.client.get(&url)
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Brave API returned status {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_brave_api_results(&body))
    }

    /// Tavily Search API (api_key in JSON body).
    async fn search_tavily(&self, api_key: &str, query: &str) -> Result<Vec<SearchResult>, String> {
        let body = serde_json::json!({ "api_key": api_key, "query": query, "max_results": 10 });
        let resp = self.client.post("https://api.tavily.com/search")
            .json(&body)
            .send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Tavily API returned status {}", resp.status()));
        }
        let result: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_tavily_results(&result))
    }

    /// Google Custom Search Engine API (key + cx params).
    async fn search_google_cse(&self, api_key: &str, query: &str, metadata: &str) -> Result<Vec<SearchResult>, String> {
        let cx = serde_json::from_str::<serde_json::Value>(metadata).ok()
            .and_then(|m| m["cx"].as_str().map(String::from))
            .ok_or("Google CSE requires 'cx' in metadata")?;
        let url = format!(
            "https://www.googleapis.com/customsearch/v1?key={}&cx={}&q={}",
            api_key, cx, urlencoding::encode(query)
        );
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("Google CSE API returned status {}", resp.status()));
        }
        let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
        Ok(parse_google_cse_results(&body))
    }

    /// SerpAPI (api_key as query param).
    async fn search_serpapi(&self, api_key: &str, query: &str) -> Result<Vec<SearchResult>, String> {
        let url = format!(
            "https://serpapi.com/search.json?api_key={}&q={}&num=10",
            api_key, urlencoding::encode(query)
        );
        let resp = self.client.get(&url).send().await.map_err(|e| e.to_string())?;
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

    async fn handle_browser(&self, input: &serde_json::Value) -> ToolResult {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let manager = match &self.browser {
            Some(m) => m,
            None => {
                return ToolResult::error(
                    "Browser automation is not available. Use web(action: \"fetch\", url: \"...\") for HTTP requests instead."
                );
            }
        };

        // Status works even when disconnected
        if action == "status" {
            let connected = manager.extension_connected();
            return ToolResult::ok(format!(
                "Browser extension connected: {}\n{}",
                connected,
                if connected {
                    "Ready. Use read_page to see the current page."
                } else {
                    "Install the Nebo Chrome/Brave extension and make sure Nebo is running."
                }
            ));
        }

        // Extension is the only browser path — no managed profiles
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
                    return ToolResult::error(
                        "Browser extension reconnecting — try again in a moment."
                    );
                }
            } else {
                return ToolResult::error(
                    "Browser extension not connected. Install the Nebo Chrome/Brave extension \
                     and make sure Nebo is running."
                );
            }
        }

        self.handle_browser_via_extension(&executor, action, input).await
    }

    /// Handle devtools actions via the Chrome extension (CDP bridge).
    async fn handle_devtools(&self, input: &serde_json::Value) -> ToolResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let manager = match &self.browser {
            Some(m) => m,
            None => {
                return ToolResult::error(
                    "DevTools requires browser extension. Use web(action: \"status\") to check connection."
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
            return ToolResult::error("Browser extension not connected.");
        }

        // Forward devtools actions to the extension
        let tool_name = match action {
            "console" => "devtools_console",
            "source" => "devtools_source",
            "storage" => "devtools_storage",
            "dom" => "devtools_dom",
            "cookies" => "devtools_cookies",
            "performance" => "devtools_performance",
            _ => {
                return ToolResult::error(format!(
                    "Unknown devtools action '{}'. Available: console, source, storage, dom, cookies, performance",
                    action
                ));
            }
        };

        let args = build_extension_args(action, input);
        match executor.execute(tool_name, &args).await {
            Ok(result) => {
                let text = serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|_| format!("{}", result));
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
    ) -> ToolResult {
        // Map action names to extension tool names
        let tool_name = match action {
            "snapshot" | "read_page" => "read_page",
            "navigate" => "navigate",
            "click" => "click",
            "double_click" => "double_click",
            "triple_click" => "triple_click",
            "right_click" => "right_click",
            "hover" => "hover",
            "fill" | "form_input" => "form_input",
            "type" => "type",
            "select" => "select",
            "screenshot" => "screenshot",
            "scroll" => "scroll",
            "scroll_to" => "scroll_to",
            "press" | "key" => "press",
            "drag" => "drag",
            "back" | "go_back" => "go_back",
            "forward" | "go_forward" => "go_forward",
            "wait" => "wait",
            "evaluate" => "evaluate",
            "list_tabs" => "list_tabs",
            "new_tab" => "new_tab",
            "close_tab" | "close" => "close_tab",
            "status" => {
                return ToolResult::ok(format!(
                    "Extension connected: true\nUse read_page to see the current page."
                ));
            }
            _ => {
                return ToolResult::error(format!(
                    "Browser action '{}' is not supported via extension. Available: navigate, read_page, click, double_click, triple_click, right_click, hover, fill, form_input, type, select, screenshot, scroll, scroll_to, press, drag, go_back, go_forward, wait, evaluate, list_tabs",
                    action
                ));
            }
        };

        // Build args for the extension tool
        let args = build_extension_args(action, input);

        match executor.execute(tool_name, &args).await {
            Ok(result) => {
                // Check for post-action screenshot in result: { text: "...", screenshot: { data, format } }
                let (text_result, screenshot_b64) = if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
                    let screenshot = result.get("screenshot")
                        .and_then(|s| s.get("data"))
                        .and_then(|d| d.as_str())
                        .map(|d| format!("data:image/png;base64,{}", d));
                    (text.to_string(), screenshot)
                } else if action == "snapshot" || action == "read_page" {
                    let page_content = result.get("pageContent").and_then(|v| v.as_str()).unwrap_or("");
                    (page_content.to_string(), None)
                } else {
                    let s = serde_json::to_string_pretty(&result).unwrap_or_else(|_| format!("{}", result));
                    (s, None)
                };

                ToolResult {
                    content: text_result,
                    is_error: false,
                    image_url: screenshot_b64,
                }
            }
            Err(e) => ToolResult::error(format!("Browser action failed: {}", e)),
        }
    }
}

impl DynTool for WebTool {
    fn name(&self) -> &str {
        "web"
    }

    fn description(&self) -> String {
        "Web operations — HTTP requests, search, browser automation, and devtools.\n\n\
         Resources and Actions:\n\
         - http: fetch (HTTP request with any method), sanitize (extract visible text and chunk for LLM)\n\
         - search: query (Web search via Brave Search)\n\
         - browser: navigate, read_page, click, double_click, triple_click, right_click, hover, fill, form_input, type, select, screenshot, scroll, scroll_to, press, drag, go_back, go_forward, wait, evaluate\n\
         - devtools: console, source, storage, dom, cookies, performance (Chrome DevTools Protocol via extension)\n\n\
         Browser workflow: read_page returns an accessibility tree with element refs. Use refs for click/fill/select.\n\
         For React/modern sites: use click on field first, then type to fill (character-by-character via real key events).\n\
         triple_click selects all text in a field, then type to replace it.\n\n\
         Examples:\n  \
         web(action: \"fetch\", url: \"https://example.com\")\n  \
         web(action: \"sanitize\", url: \"https://example.com\") — extract text, chunked\n  \
         web(action: \"sanitize\", url: \"https://example.com\", chunk_size: 8000) — larger chunks\n  \
         web(action: \"search\", query: \"rust async programming\")\n  \
         web(action: \"navigate\", url: \"https://example.com\")\n  \
         web(action: \"read_page\", filter: \"interactive\")\n  \
         web(action: \"click\", ref: \"ref_1\")\n  \
         web(action: \"triple_click\", ref: \"ref_3\") — select all text\n  \
         web(action: \"type\", text: \"hello\") — types via real key events\n  \
         web(action: \"fill\", ref: \"ref_3\", value: \"hello\") — direct value set\n  \
         web(action: \"hover\", ref: \"ref_5\")\n  \
         web(action: \"scroll_to\", ref: \"ref_10\")\n  \
         web(action: \"press\", key: \"cmd+a\") — key chord\n  \
         web(action: \"press\", key: \"ArrowDown ArrowDown Enter\") — key sequence\n  \
         web(action: \"console\") — read browser console logs\n  \
         web(action: \"cookies\") — inspect cookies\n  \
         web(action: \"performance\") — page performance metrics"
            .to_string()
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "resource": {
                    "type": "string",
                    "description": "Resource type: http, search, browser, devtools",
                    "enum": ["http", "search", "browser", "devtools"]
                },
                "action": {
                    "type": "string",
                    "description": "Action to perform",
                    "enum": ["fetch", "get", "post", "put", "delete", "head", "sanitize",
                             "search", "query",
                             "navigate", "read_page", "snapshot", "click", "double_click",
                             "triple_click", "right_click", "hover", "fill", "form_input",
                             "type", "select", "screenshot", "scroll", "scroll_to", "press",
                             "key", "drag", "go_back", "go_forward", "wait", "evaluate",
                             "list_tabs", "new_tab", "close_tab", "status",
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
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        true
    }

    fn resource_permit(&self, input: &serde_json::Value) -> Option<ResourceKind> {
        let resource = input.get("resource")
            .and_then(|v| v.as_str())
            .unwrap_or("");
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

    fn execute_dyn<'a>(
        &'a self,
        _ctx: &'a ToolContext,
        input: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let domain_input: DomainInput = match serde_json::from_value(input.clone()) {
                Ok(v) => v,
                Err(e) => return ToolResult::error(format!("Failed to parse input: {}", e)),
            };

            let resource = if domain_input.resource.is_empty() {
                self.infer_resource(&domain_input.action).to_string()
            } else {
                domain_input.resource
            };

            if resource.is_empty() {
                return ToolResult::error(
                    "Resource is required. Available: http, search, browser, devtools",
                );
            }

            match resource.as_str() {
                "http" => self.handle_http(&input).await,
                "search" => self.handle_search(&input).await,
                "browser" => self.handle_browser(&input).await,
                "devtools" => self.handle_devtools(&input).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: http, search, browser, devtools",
                    other
                )),
            }
        })
    }
}

/// Build extension tool arguments from the web tool input.
fn build_extension_args(action: &str, input: &serde_json::Value) -> serde_json::Value {
    let mut args = serde_json::Map::new();

    // Forward common parameters
    let forward_keys = match action {
        "navigate" | "new_tab" => vec!["url"],
        "click" | "double_click" | "triple_click" | "right_click" => {
            vec!["ref", "selector", "coordinate", "x", "y", "modifiers"]
        }
        "hover" => vec!["ref", "coordinate", "x", "y"],
        "fill" | "form_input" => vec!["ref", "selector", "value"],
        "type" => vec!["text"],
        "select" => vec!["ref", "selector", "value"],
        "scroll" => vec!["direction", "amount", "scroll_direction", "scroll_amount", "coordinate"],
        "scroll_to" => vec!["ref"],
        "press" | "key" => vec!["key", "text", "repeat"],
        "drag" => vec!["start_coordinate", "coordinate"],
        "wait" => vec!["ms", "duration"],
        "evaluate" => vec!["expression", "text"],
        "snapshot" | "read_page" => vec!["filter", "depth", "maxChars", "refId"],
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
    let results = body.get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
        .unwrap_or(&empty);
    results.iter().filter_map(|r| {
        let title = r.get("title").and_then(|v| v.as_str())?;
        let url = r.get("url").and_then(|v| v.as_str())?;
        let snippet = r.get("description").and_then(|v| v.as_str()).unwrap_or("");
        Some(SearchResult { title: title.to_string(), url: url.to_string(), snippet: snippet.to_string() })
    }).take(10).collect()
}

/// Parse Tavily Search API JSON response.
fn parse_tavily_results(body: &serde_json::Value) -> Vec<SearchResult> {
    let empty = vec![];
    let results = body.get("results").and_then(|r| r.as_array()).unwrap_or(&empty);
    results.iter().filter_map(|r| {
        let title = r.get("title").and_then(|v| v.as_str())?;
        let url = r.get("url").and_then(|v| v.as_str())?;
        let snippet = r.get("content").and_then(|v| v.as_str()).unwrap_or("");
        Some(SearchResult { title: title.to_string(), url: url.to_string(), snippet: snippet.to_string() })
    }).take(10).collect()
}

/// Parse Google Custom Search Engine API JSON response.
fn parse_google_cse_results(body: &serde_json::Value) -> Vec<SearchResult> {
    let empty = vec![];
    let items = body.get("items").and_then(|r| r.as_array()).unwrap_or(&empty);
    items.iter().filter_map(|r| {
        let title = r.get("title").and_then(|v| v.as_str())?;
        let url = r.get("link").and_then(|v| v.as_str())?;
        let snippet = r.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
        Some(SearchResult { title: title.to_string(), url: url.to_string(), snippet: snippet.to_string() })
    }).take(10).collect()
}

/// Parse SerpAPI JSON response.
fn parse_serpapi_results(body: &serde_json::Value) -> Vec<SearchResult> {
    let empty = vec![];
    let results = body.get("organic_results").and_then(|r| r.as_array()).unwrap_or(&empty);
    results.iter().filter_map(|r| {
        let title = r.get("title").and_then(|v| v.as_str())?;
        let url = r.get("link").and_then(|v| v.as_str())?;
        let snippet = r.get("snippet").and_then(|v| v.as_str()).unwrap_or("");
        Some(SearchResult { title: title.to_string(), url: url.to_string(), snippet: snippet.to_string() })
    }).take(10).collect()
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

        // Extract URL from the cite tag content (e.g., "neboloop.com › blog")
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

