use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ResourceKind, ToolResult};

/// Max chars for auto-snapshot appended after mutation actions.
const AUTO_SNAPSHOT_MAX_CHARS: usize = 6_000;

/// Max chars for inline read_page/evaluate results.
/// Results beyond this are truncated with a hint, never spilled to files.
const MAX_INLINE_CHARS: usize = 15_000;

/// Callback type for broadcasting events to connected WebSocket clients.
pub type Broadcaster = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

/// Cached result from a previous visit, shared across sibling subagents.
#[derive(Clone)]
struct VisitedPage {
    content: String,
    is_error: bool,
    visited_by: String,
    timestamp: std::time::Instant,
}

/// WebTool consolidates web operations: HTTP fetch, search, and browser automation.
pub struct WebTool {
    client: reqwest::Client,
    browser: Option<Arc<browser::Manager>>,
    store: Option<Arc<db::Store>>,
    broadcaster: Option<Broadcaster>,
    /// Per-session navigate URL visit counts for loop detection.
    nav_history: Mutex<HashMap<String, HashMap<String, u32>>>,
    /// Cross-subagent visited pages: group_key → url/query → cached result.
    /// Siblings in the same parent group share this cache so they don't
    /// duplicate browsing work.
    visited_pages: Mutex<HashMap<String, HashMap<String, VisitedPage>>>,
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
            nav_history: Mutex::new(HashMap::new()),
            visited_pages: Mutex::new(HashMap::new()),
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

    /// Derive a group key from the session_key so sibling subagents share
    /// a visited-pages cache. For `subagent:parent_key:sa-xxx`, the group
    /// is the parent_key. For top-level sessions, each is its own group.
    fn session_group_key(session_key: &str) -> String {
        if let Some(rest) = session_key.strip_prefix("subagent:") {
            // subagent:{parent_key}:sa-{uuid} → parent_key
            if let Some(pos) = rest.rfind(":sa-") {
                return rest[..pos].to_string();
            }
        }
        session_key.to_string()
    }

    /// Check if a URL or query was already visited by a sibling in the same group.
    fn check_visited(&self, group_key: &str, url_or_query: &str) -> Option<VisitedPage> {
        let guard = self.visited_pages.lock().ok()?;
        let group = guard.get(group_key)?;
        let entry = group.get(url_or_query)?;
        if entry.timestamp.elapsed() < std::time::Duration::from_secs(300) {
            Some(entry.clone())
        } else {
            None
        }
    }

    /// Record a visited URL/query result so siblings can reuse it.
    fn record_visited(
        &self,
        group_key: &str,
        url_or_query: &str,
        content: &str,
        is_error: bool,
        session_id: &str,
    ) {
        if let Ok(mut guard) = self.visited_pages.lock() {
            let group = guard.entry(group_key.to_string()).or_default();
            group.insert(
                url_or_query.to_string(),
                VisitedPage {
                    content: content.to_string(),
                    is_error,
                    visited_by: session_id.to_string(),
                    timestamp: std::time::Instant::now(),
                },
            );
        }
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "fetch" | "sanitize" => "http",
            "search" => "search",
            "navigate" | "read_page" | "click" | "fill" | "type" | "screenshot"
            | "evaluate" | "list_tabs" | "new_tab" | "close_tab" | "history"
            | "scroll" | "hover" | "select" | "press" | "wait" | "drag" | "status"
            | "read_console_messages" | "read_network_requests" | "resize_window"
            | "file_upload" | "find" | "fill_form" | "browser_batch" => "browser",
            "console" => "devtools",
            _ => "",
        }
    }

    async fn handle_http(&self, input: &serde_json::Value) -> ToolResult {
        let url = match input.get("url").and_then(|v| v.as_str()) {
            Some(u) => u,
            None => {
                return ToolResult::error(crate::errors::missing_param(
                    "fetch",
                    "url",
                    "web(action: \"fetch\", url: \"https://example.com\")",
                ))
            }
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
                Err(e) => {
                    return ToolResult::error(format!(
                        "HTTP request failed for {}: {}. Check that the URL is correct and the server is reachable.",
                        url, e
                    ))
                }
            };
            let status = resp.status().as_u16();
            let html = match resp.text().await {
                Ok(t) => t,
                Err(e) => {
                    return ToolResult::error(format!(
                        "Failed to read response body from {} (status {}): {}",
                        url, status, e
                    ))
                }
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
                ))
                .with_http_status(status);
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
            ))
            .with_http_status(status);
        }

        // HTTP method comes from the `method` param (the one way); defaults to GET.
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
                        let display_body = if content_type.contains("html") {
                            // Rendered page: return capped VISIBLE TEXT, not a wall of raw
                            // HTML/markup/scripts. `sanitize_html` drops script/style/nav
                            // noise (same extractor the `sanitize` action uses). For the full
                            // page use read_page after navigate; for structured data fetch a
                            // JSON/API endpoint (which stays raw below).
                            let text = sanitize_html(&body);
                            if text.len() > MAX_INLINE_CHARS {
                                let end =
                                    types::strutil::floor_char_boundary(&text, MAX_INLINE_CHARS);
                                format!(
                                    "{}\n\n[Truncated to {} chars — this is a rendered web page. \
                                     For the full page use web(action: \"read_page\") after \
                                     navigate; for structured data fetch a JSON/API endpoint.]",
                                    &text[..end],
                                    MAX_INLINE_CHARS
                                )
                            } else {
                                text
                            }
                        } else if body.len() > 50_000 {
                            // Non-HTML (e.g. JSON/API) — keep RAW so it stays parseable,
                            // paginated by `offset` for very large responses.
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
                            body
                        };

                        ToolResult::ok(format!(
                            "HTTP {} {} — Status: {}\n\n{}",
                            method, url, status, display_body
                        ))
                        .with_http_status(status)
                    }
                    Err(e) => ToolResult::error(format!(
                        "Failed to read response body from {}: {}",
                        url, e
                    )),
                }
            }
            Err(e) => ToolResult::error(format!(
                "HTTP request failed for {}: {}. Check that the URL is correct and the server is reachable.",
                url, e
            )),
        }
    }

    async fn handle_search(&self, input: &serde_json::Value, session_id: &str, group_key: &str) -> ToolResult {
        let raw_query = match input.get("query").and_then(|v| v.as_str()) {
            Some(q) => q,
            None => {
                return ToolResult::error(crate::errors::missing_param(
                    "search",
                    "query",
                    "web(action: \"search\", query: \"rust async tutorial\")",
                ))
            }
        };
        // Weak models stuff queries with stacked `site:` filters and run them hundreds of chars
        // long; keyword engines (DuckDuckGo) reject those and return nothing. Normalize to a clean
        // keyword query the engine will actually accept.
        let query_owned = normalize_search_query(raw_query);
        if query_owned != raw_query.trim() {
            tracing::info!(original = %raw_query, normalized = %query_owned, "rewrote search query");
        }
        let query = query_owned.as_str();

        // Skip re-running a query already searched recently (by a sibling OR earlier this session).
        let search_key = format!("search:{}", query.to_lowercase().trim());
        if let Some(cached) = self.check_visited(group_key, &search_key) {
            tracing::info!(
                session_id = %session_id,
                visited_by = %cached.visited_by,
                query = %query,
                "search cache hit — returning cached results instead of re-searching"
            );
            return ToolResult {
                content: format!("[Already searched this recently — cached results]\n\n{}", cached.content),
                is_error: cached.is_error,
                image_url: None,
                http_status: None,
            };
        }

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
                                let result = format_search_results(query, &results);
                                self.record_visited(group_key, &search_key, &result.content, false, session_id);
                                return result;
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

        // 2. Prefer the connected browser/extension — it uses the user's real Chrome (handles
        //    JS, bot-detection, and auth), whereas DDG HTTP scraping is unreliable and can stall.
        if self.browser_search_available() {
            tracing::info!(query, "browser available — searching via browser/extension");
            let browser_result = self.search_via_browser(query, session_id).await;
            if !browser_result.is_error {
                self.record_visited(group_key, &search_key, &browser_result.content, false, session_id);
                return browser_result;
            }
            tracing::warn!(query, "browser search failed — falling back to DDG scraping");
        }

        // 3. DuckDuckGo HTTP scraping (fail-fast: cap at 8s so a hung/blocked request can't
        //    stall the whole turn — see docs/bugs/web-search-slow-fallback.md). It internally
        //    chains to Brave scraping if DDG returns nothing.
        tracing::info!(query, "trying DDG HTTP scraping");
        let result = match tokio::time::timeout(
            std::time::Duration::from_secs(8),
            self.search_duckduckgo_html(query),
        )
        .await
        {
            Ok(r) => r,
            Err(_) => {
                tracing::warn!(query, "DDG scraping timed out after 8s");
                ToolResult::error(
                    "Web search timed out and no browser is connected. Connect the Nebo Chrome \
                     extension or configure a search API key for reliable results.",
                )
            }
        };
        if !result.is_error {
            self.record_visited(group_key, &search_key, &result.content, false, session_id);
        }
        result
    }

    /// Whether a browser backend (connected extension or headless agent-browser) is available
    /// to run a search — used to prefer it over DDG HTTP scraping.
    fn browser_search_available(&self) -> bool {
        match &self.browser {
            Some(m) => m.executor().map(|e| e.is_connected()).unwrap_or(false),
            None => false,
        }
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

        // Nudge the user to install the extension whenever it isn't connected — even if the
        // built-in CDP browser is carrying the work. The extension is the intended path.
        if !executor.extension_connected() {
            self.broadcast_extension_disconnected("not_connected", session_id);
        }

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

        // Pull the rendered page HTML and parse result links generically. Reading the real
        // browser's DOM (vs a direct scrape) uses the user's IP/cookies + JS, sidestepping the
        // bot-block that hits direct scraping. `read_page` returns the accessibility tree (the
        // search FORM, not the results), so we evaluate the raw HTML and run the same generic
        // link extractor. If the page yields nothing usable (bot-check, or a results-less form
        // page), fall through to the direct scrape chain (DDG → Brave) so we never return chrome
        // as if it were results.
        let html_expr = serde_json::json!({ "expression": "document.documentElement.outerHTML" });
        if let Ok(v) = executor.execute("evaluate", &html_expr, Some(session_id)).await {
            let html = v
                .get("text")
                .or_else(|| v.get("result"))
                .or_else(|| v.get("value"))
                .or_else(|| v.get("pageContent"))
                .and_then(|x| x.as_str())
                .unwrap_or("");
            let results = extract_search_links(html, "duckduckgo.com");
            if !results.is_empty() {
                return format_search_results(query, &results);
            }
        }
        tracing::warn!("browser search yielded no parseable results — falling back to direct scrape chain");
        self.search_duckduckgo_html(query).await
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
                    let results = extract_search_links(&html, "search.brave.com");
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
                    let results = extract_search_links(&html, "duckduckgo.com");
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

    async fn handle_browser(&self, input: &serde_json::Value, session_id: &str, group_key: &str) -> ToolResult {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let manager = match &self.browser {
            Some(m) => m,
            None => {
                return ToolResult::error(
                    "Browser automation is not available. Use web(action: \"fetch\", url: \"...\") for HTTP requests instead.",
                );
            }
        };

        // The executor is the single source of truth for backend state — both the `status`
        // report and the connection gate below read it, so they can never disagree.
        let executor = match manager.executor() {
            Some(e) => e,
            None => {
                return ToolResult::error("Browser automation not configured.");
            }
        };

        // Status works even when disconnected
        if action == "status" {
            let ext_connected = executor.extension_connected();
            let cdp = executor.cdp_available();
            let status = if ext_connected {
                "Browser extension connected. Ready. Use read_page to see the current page."
            } else if cdp {
                "Built-in Chrome (CDP) available as a fallback. Use read_page to see the current page."
            } else {
                "No browser backend available. Connect the Nebo Chrome/Brave extension."
            };
            return ToolResult::ok(format!(
                "Extension: {}, Built-in Chrome: {}\n{}",
                ext_connected, cdp, status
            ));
        }

        // Nudge to install the extension whenever it isn't connected — even if the built-in
        // CDP browser is handling this action. The extension is the intended path.
        if !executor.extension_connected() {
            self.broadcast_extension_disconnected("not_connected", session_id);
        }

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
                    "No browser backend available. Connect the Nebo Chrome/Brave extension.",
                );
            }
        }

        if action == "navigate" {
            if let Some(url) = input.get("url").and_then(|v| v.as_str()) {
                // Don't navigate the real browser to a binary/file URL (PDF, docx, zip, …): it
                // can't render it, so it triggers a download + OS save dialog that derails the
                // run. Tell the agent to find the info on an HTML page instead.
                if let Some(ext) = file_download_ext(url) {
                    tracing::info!(url = %url, ext = %ext, "skipping navigate to binary file URL (would trigger download)");
                    return ToolResult::ok(format!(
                        "Skipped: {url} is a .{ext} file the browser can't display (opening it only \
                         triggers a download). Find the information on an HTML page instead — e.g. \
                         the article's abstract/landing page rather than the file itself."
                    ));
                }
                // Skip re-navigating to a URL visited recently (by a sibling OR earlier this
                // session) — return the cached page instead of re-loading it.
                let nav_key = format!("nav:{}", url);
                if let Some(cached) = self.check_visited(group_key, &nav_key) {
                    tracing::info!(
                        session_id = %session_id,
                        visited_by = %cached.visited_by,
                        url = %url,
                        "navigate cache hit — returning cached page instead of re-visiting"
                    );
                    return ToolResult {
                        content: format!("[Already visited this page recently — cached content]\n\n{}", cached.content),
                        is_error: cached.is_error,
                        image_url: None,
                        http_status: None,
                    };
                }
            }
        }

        let result = self.handle_browser_via_extension(&executor, action, input, Some(session_id))
            .await;

        // Record navigate results for sibling dedup
        if action == "navigate" && !result.is_error {
            if let Some(url) = input.get("url").and_then(|v| v.as_str()) {
                let nav_key = format!("nav:{}", url);
                self.record_visited(group_key, &nav_key, &result.content, false, session_id);
            }
        }

        result
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
                    let total = results.len();
                    let mut last_text = String::new();
                    let mut last_action = "unknown";
                    let mut error_msg: Option<String> = None;

                    for (i, result) in results.iter().enumerate() {
                        let action_name = actions_val
                            .get(i)
                            .and_then(|v| v.get("action"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        match result {
                            Ok(val) => {
                                last_action = action_name;
                                last_text = if let Some(t) = val.get("text").and_then(|v| v.as_str()) {
                                    t.to_string()
                                } else if let Some(pc) = val.get("pageContent").and_then(|v| v.as_str()) {
                                    pc.to_string()
                                } else {
                                    serde_json::to_string(val).unwrap_or_default()
                                };
                            }
                            Err(e) => {
                                error_msg = Some(format!(
                                    "Step {}/{} ({}) failed: {}",
                                    i + 1, total, action_name, e
                                ));
                                break;
                            }
                        }
                    }

                    let mut content = if let Some(err) = error_msg {
                        err
                    } else {
                        format!("Batch completed ({} actions). {}: {}", total, last_action, last_text)
                    };

                    // Auto-snapshot after batch
                    auto_snapshot(executor, session_id, &mut content, AUTO_SNAPSHOT_MAX_CHARS).await;
                    ToolResult::ok(content)
                }
                Err(e) => ToolResult::error(format!("browser_batch failed: {}", e)),
            };
        }

        // fill_form: batch-fill multiple form fields in one call
        if action == "fill_form" {
            let fields = match input.get("fields").and_then(|v| v.as_array()) {
                Some(f) if !f.is_empty() => f,
                _ => {
                    return ToolResult::error(
                        "fill_form requires a non-empty 'fields' array. Each field: {ref, value}.\n\
                         Example: web(action: \"fill_form\", fields: [{ref: \"ref_3\", value: \"John\"}])"
                    );
                }
            };

            let mut batch_actions = Vec::new();
            for field in fields {
                let field_ref = match field.get("ref").and_then(|v| v.as_str()) {
                    Some(r) => r,
                    None => {
                        return ToolResult::error("Each field in fill_form must have a 'ref'.");
                    }
                };
                let value = match field.get("value") {
                    Some(v) => v,
                    None => {
                        return ToolResult::error("Each field in fill_form must have a 'value'.");
                    }
                };

                // For text values: click → select all → type (works on all frameworks)
                // For booleans/numbers: use fill directly (checkboxes, selects)
                if value.is_string() {
                    batch_actions.push(browser::BatchAction {
                        tool: "click".to_string(),
                        args: serde_json::json!({"ref": field_ref}),
                    });
                    batch_actions.push(browser::BatchAction {
                        tool: "press".to_string(),
                        args: serde_json::json!({"key": "cmd+a"}),
                    });
                    batch_actions.push(browser::BatchAction {
                        tool: "type".to_string(),
                        args: serde_json::json!({"text": value}),
                    });
                } else {
                    batch_actions.push(browser::BatchAction {
                        tool: "form_input".to_string(),
                        args: serde_json::json!({"ref": field_ref, "value": value}),
                    });
                }
            }

            let opts = browser::BatchOptions { stop_on_error: true };
            return match executor.batch_execute(batch_actions, opts, session_id).await {
                Ok(results) => {
                    let mut content = format!("Filled {} field(s).", fields.len());
                    if let Some(Err(e)) = results.iter().find(|r| r.is_err()) {
                        content = format!("fill_form partially failed: {}", e);
                    }
                    auto_snapshot(executor, session_id, &mut content, AUTO_SNAPSHOT_MAX_CHARS).await;
                    ToolResult::ok(content)
                }
                Err(e) => ToolResult::error(format!("fill_form failed: {}", e)),
            };
        }

        // history: go_back / go_forward in one action
        if action == "history" {
            let dir = input.get("direction").and_then(|v| v.as_str()).unwrap_or("back");
            let tool = match dir {
                "forward" => "go_forward",
                _ => "go_back",
            };
            let result = executor.execute(tool, &serde_json::json!({}), session_id).await;
            return match result {
                Ok(val) => {
                    let mut text = val.get("text").and_then(|v| v.as_str())
                        .unwrap_or("Done").to_string();
                    auto_snapshot(executor, session_id, &mut text, AUTO_SNAPSHOT_MAX_CHARS).await;
                    ToolResult::ok(text)
                }
                Err(e) => ToolResult::error(friendly_browser_error("history", &e.to_string())),
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
        let mut tool_name = match map_action_to_tool(action) {
            Some(t) => t,
            None => {
                return ToolResult::error(format!(
                    "Browser action '{}' is not supported. Available: navigate, read_page, click, \
                     hover, fill, type, select, screenshot, scroll, press, drag, wait, evaluate, \
                     list_tabs, new_tab, close_tab, history, find, file_upload, \
                     fill_form, browser_batch",
                    action
                ));
            }
        };

        // Resolve consolidated click → extension tool name based on params
        if tool_name == "click" {
            let click_count = input.get("click_count").and_then(|v| v.as_u64()).unwrap_or(1);
            let button = input.get("button").and_then(|v| v.as_str()).unwrap_or("left");
            tool_name = match (click_count, button) {
                (_, "right") => "right_click",
                (3, _) => "triple_click",
                (2, _) => "double_click",
                _ => "click",
            };
        }

        // Resolve consolidated scroll → scroll_to when ref is present
        if tool_name == "scroll" && input.get("ref").is_some() && input.get("direction").is_none() {
            tool_name = "scroll_to";
        }

        // Build args for the extension tool
        let args = build_extension_args(action, input);

        // Execute with auto-retry for read_page character limit errors.
        // The extension (at parity with Claude) returns an error when output > maxChars.
        // Nebo handles this by retrying with tighter params so the agent always gets content.
        tracing::info!(
            tool = %tool_name,
            action = %action,
            session_id = ?session_id,
            args_keys = ?args.as_object().map(|o| o.keys().collect::<Vec<_>>()),
            "browser extension execute"
        );
        let result = executor.execute(tool_name, &args, session_id).await;
        match &result {
            Ok(val) => {
                let has_page_content = val.get("pageContent").and_then(|v| v.as_str()).map(|s| s.len());
                let has_text = val.get("text").and_then(|v| v.as_str()).map(|s| s.len());
                let has_screenshot = val.get("screenshot").is_some();
                tracing::info!(
                    tool = %tool_name,
                    action = %action,
                    has_page_content = ?has_page_content,
                    has_text = ?has_text,
                    has_screenshot = has_screenshot,
                    result_keys = ?val.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                    "browser extension result OK"
                );
            }
            Err(e) => {
                tracing::warn!(
                    tool = %tool_name,
                    action = %action,
                    error = %e,
                    "browser extension result ERROR"
                );
            }
        }

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
                                let content = if page_content.len() > MAX_INLINE_CHARS {
                                    truncate_snapshot(page_content, MAX_INLINE_CHARS)
                                } else {
                                    page_content.to_string()
                                };
                                return ToolResult {
                                    content,
                                    is_error: false,
                                    image_url: None,
                                    http_status: None,
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
                let (mut text_result, mut screenshot_b64) =
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

                // Auto-snapshot: append compact page state after any mutation action.
                // This is the key pattern from Playwright MCP — the model sees
                // what changed without needing a separate read_page call.
                const SNAPSHOT_ACTIONS: &[&str] = &[
                    "navigate", "click", "double_click", "triple_click", "right_click",
                    "type", "fill", "form_input", "select", "press",
                    "scroll", "scroll_to", "drag", "hover", "file_upload",
                    "go_back", "go_forward",
                ];
                if SNAPSHOT_ACTIONS.contains(&action) {
                    auto_snapshot(executor, session_id, &mut text_result, AUTO_SNAPSHOT_MAX_CHARS).await;

                    // Auto-screenshot after navigate
                    if action == "navigate" && screenshot_b64.is_none() {
                        let shot_args = serde_json::json!({});
                        if let Ok(shot_result) = executor.execute("screenshot", &shot_args, session_id).await {
                            screenshot_b64 = shot_result.get("screenshot").and_then(|s| {
                                let data = s.get("data")?.as_str()?;
                                let fmt = s.get("format").and_then(|f| f.as_str()).unwrap_or("jpeg");
                                Some(format!("data:image/{};base64,{}", fmt, data))
                            });
                        }
                    }
                }

                // Navigate-specific: error page + auth detection + loop detection
                if action == "navigate" {
                    let nav_url = input
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some(warning) = detect_error_page(&text_result) {
                        text_result = format!("{}\n\n{}", warning, text_result);
                    } else if let Some(warning) = detect_auth_page(nav_url, &text_result) {
                        text_result = format!("{}\n\n{}", warning, text_result);
                    }

                    if !nav_url.is_empty() {
                        let origin = extract_origin(nav_url);
                        if !origin.is_empty() {
                            let sid = session_id.unwrap_or("default").to_string();
                            let count = {
                                let mut history = self.nav_history.lock().unwrap();
                                let session_map = history.entry(sid).or_default();
                                let c = session_map.entry(origin).or_insert(0);
                                *c += 1;
                                *c
                            };
                            if count >= 3 {
                                text_result.push_str(&format!(
                                    "\n\n⚠ You have navigated to this site {} times in this session. \
                                     If you're not making progress, STOP and try a different approach: \
                                     use web(action: search) to find an alternative source, or \
                                     use wait(duration: 3) before read_page if content is loading slowly.",
                                    count
                                ));
                            }
                        }
                    }
                }

                // Check read_page content for login pages
                if matches!(action, "snapshot" | "read_page") {
                    if let Some(warning) = detect_auth_page("", &text_result) {
                        text_result = format!("{}\n\n{}", warning, text_result);
                    }
                }

                // Truncate large results inline — never spill to files.
                if matches!(action, "evaluate" | "snapshot" | "read_page") {
                    if text_result.len() > MAX_INLINE_CHARS {
                        text_result = truncate_snapshot(&text_result, MAX_INLINE_CHARS);
                    }
                }

                ToolResult {
                    content: text_result,
                    is_error: false,
                    image_url: screenshot_b64,
                    http_status: None,
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
        "Web operations — HTTP requests, search, and browser automation.\n\n\
         Decision: API/static HTML → fetch/search. Rendered page or user sessions → browser actions.\n\n\
         ## Browser — Controls the user's real Chrome browser\n\
         Every mutation action (click, type, fill, press, scroll, etc.) returns a page snapshot automatically — \
         you do NOT need to call read_page after actions. The snapshot shows interactive elements with refs.\n\n\
         Actions: navigate, read_page, click, hover, fill, type, select, screenshot, scroll, press, drag, \
         wait, evaluate, history, find, file_upload, fill_form, browser_batch\n\n\
         Batching: browser_batch chains 2+ predictable steps in one round trip. fill_form fills multiple \
         form fields at once. USE THESE for multi-step sequences.\n\n\
         ## Rules\n\
         - read_page FIRST before interacting — see what's on screen\n\
         - Scroll down to find content below the fold — read_page only shows the viewport\n\
         - For text inputs: click → press(key: cmd+a) → type. fill is for dropdowns/checkboxes only\n\
         - NEVER navigate with search query params (triggers anti-bot). Navigate to the site, find the search box, type your query\n\
         - Do NOT click file upload buttons. Use file_upload(ref) instead\n\
         - After search results appear, extract data from results BEFORE visiting individual pages\n\
         - When you have enough info, STOP and respond. Don't keep browsing to be thorough"
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
                    "description": "The operation to perform on the selected resource.",
                    "enum": ["fetch", "sanitize",
                             "search",
                             "navigate", "read_page", "click", "hover", "fill",
                             "type", "select", "screenshot", "scroll", "press",
                             "drag", "wait", "evaluate",
                             "list_tabs", "new_tab", "close_tab",
                             "history", "find", "file_upload",
                             "fill_form", "browser_batch",
                             "read_console_messages", "read_network_requests", "resize_window",
                             "status", "console"]
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
                    "description": "For search: write ONE short keyword query (≤ ~10 words). Do NOT chain \
                        `site:` operators or paste lists of domains — search engines reject long queries and \
                        return nothing. To dig deeper, run a NEW query with different keywords, not more filters. \
                        For find: a natural-language description of the element(s) to locate."
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
                "click_count": {
                    "type": "integer",
                    "description": "For click: number of clicks (1=single, 2=double, 3=triple). Default 1."
                },
                "button": {
                    "type": "string",
                    "description": "For click: mouse button. Default left.",
                    "enum": ["left", "right"]
                },
                "direction": {
                    "type": "string",
                    "description": "For scroll: up/down/left/right. For history: back/forward.",
                    "enum": ["up", "down", "left", "right", "back", "forward"]
                },
                "fields": {
                    "type": "array",
                    "description": "For fill_form: array of fields to fill. Each field: {ref, value}. Text inputs use click+type, selects/checkboxes use fill.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "ref": { "type": "string" },
                            "value": { "type": ["string", "boolean", "number"] }
                        },
                        "required": ["ref", "value"]
                    }
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
                "chunk_size": {
                    "type": "integer",
                    "description": "Max characters per chunk for sanitize (default 4000)"
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
            let session_key = &ctx.session_key;
            let group_key = Self::session_group_key(session_key);
            tracing::info!(session_id = %session_id, resource = %resource, group = %group_key, "web_tool session scoping");

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
                "search" => self.handle_search(&input, session_id, &group_key).await,
                "browser" => self.handle_browser(&input, session_id, &group_key).await,
                "devtools" => self.handle_devtools(&input, session_id).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: http, search, browser, devtools",
                    other
                )),
            }
        })
    }
}

/// Append a compact page snapshot after a mutation action.
/// The model sees the updated page state without needing a separate read_page call.
async fn auto_snapshot(
    executor: &browser::ActionExecutor,
    session_id: Option<&str>,
    text_result: &mut String,
    max_chars: usize,
) {
    let snap_args = serde_json::json!({"filter": "interactive"});
    match executor.execute("read_page", &snap_args, session_id).await {
        Ok(snap_result) => {
            let snapshot_text = snap_result
                .get("pageContent")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if !snapshot_text.is_empty() {
                let truncated = truncate_snapshot(snapshot_text, max_chars);
                text_result.push_str("\n\n## Page Snapshot (interactive elements)\n");
                text_result.push_str(&truncated);
            }
        }
        Err(_) => {} // page may have navigated away — silently skip
    }
}

/// Map a web tool action name to the corresponding extension tool name.
/// Returns None for actions that don't map (status, new_tab validation, etc.)
fn map_action_to_tool(action: &str) -> Option<&'static str> {
    // Canonical model actions only → extension tool name. Variants (double/right click,
    // scroll-to-element) are resolved from params by the caller, not accepted as aliases here.
    match action {
        "read_page" => Some("read_page"),
        "navigate" => Some("navigate"),
        "click" => Some("click"),
        "hover" => Some("hover"),
        "fill" => Some("form_input"),
        "type" => Some("type"),
        "select" => Some("select"),
        "screenshot" => Some("screenshot"),
        "scroll" => Some("scroll"),
        "press" => Some("press"),
        "drag" => Some("drag"),
        "history" => None, // handled specially in handle_browser_via_extension (direction → go_back/go_forward)
        "wait" => Some("wait"),
        "evaluate" => Some("evaluate"),
        "list_tabs" => Some("list_tabs"),
        "new_tab" => Some("new_tab"),
        "close_tab" => Some("close_tab"),
        "read_console_messages" => Some("read_console_messages"),
        "read_network_requests" => Some("read_network_requests"),
        "resize_window" => Some("resize_window"),
        "file_upload" => Some("file_upload"),
        "find" => Some("find"),
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
        "click" => vec!["ref", "selector", "coordinate", "modifiers", "click_count", "button"],
        "hover" => vec!["ref", "coordinate"],
        "fill" => vec!["ref", "selector", "value"],
        "type" => vec!["text"],
        "select" => vec!["ref", "selector", "value"],
        "scroll" => vec!["direction", "amount", "coordinate", "ref"],
        "press" => vec!["key", "text", "repeat"],
        "drag" => vec!["start_coordinate", "coordinate"],
        "wait" => vec!["ms"],
        "evaluate" => vec!["expression", "text"],
        "read_page" => vec!["filter", "depth", "maxChars", "refId"],
        "close_tab" => vec!["tabId", "tabIds"],
        "read_console_messages" => vec!["onlyErrors", "clear", "pattern", "limit"],
        "read_network_requests" => vec!["urlPattern", "clear", "limit"],
        "resize_window" => vec!["width", "height"],
        "file_upload" => vec!["paths", "ref"],
        "find" => vec!["query"],
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
    let safe_max = types::strutil::floor_char_boundary(text, max_chars);
    let truncated = &text[..safe_max];
    let last_newline = truncated.rfind('\n').unwrap_or(safe_max);
    let clean = &text[..last_newline];
    let omitted = text.len() - last_newline;
    format!(
        "{}\n\n[...{} chars truncated. Use read_page with refId to zoom into a section, \
         or filter: \"interactive\" for only interactive elements.]",
        clean, omitted
    )
}

/// Detect if page content indicates an authentication/login page.
/// Returns a warning string if auth signals are found, None otherwise.
/// Uses a two-signal threshold to avoid false positives on pages that merely
/// mention passwords or have a "sign in" link in the header.
fn detect_auth_page(url: &str, content: &str) -> Option<String> {
    let url_lower = url.to_lowercase();
    let content_lower = content.to_lowercase();

    let url_is_auth = [
        "/login",
        "/signin",
        "/sign-in",
        "/sign_in",
        "/auth/",
        "/sso/",
        "/oauth/",
        "/flow/login",
        "/accounts/login",
        "/session/new",
    ]
    .iter()
    .any(|p| url_lower.contains(p));

    let has_password_field = content_lower.contains("type=\"password\"")
        || content_lower.contains("type='password'");

    let has_auth_heading = content_lower.contains("sign in to")
        || content_lower.contains("log in to")
        || content_lower.contains("heading \"sign in")
        || content_lower.contains("heading \"log in");

    let has_oauth =
        content_lower.contains("sign in with") || content_lower.contains("continue with google");

    let has_forgot_password = content_lower.contains("forgot password");

    let signals = [
        url_is_auth,
        has_password_field,
        has_auth_heading,
        has_oauth,
        has_forgot_password,
    ]
    .iter()
    .filter(|&&b| b)
    .count();

    if signals >= 2 {
        Some(
            "⚠️ AUTHENTICATION REQUIRED — This page is a login/sign-in form. \
             You do not have credentials for this service and cannot authenticate. \
             Do NOT attempt to fill login forms, click sign-in buttons, or interact \
             with OAuth prompts — these actions will fail. Instead, report to the user \
             that this task requires authentication and suggest they: \
             (1) log in manually, (2) install a skill/plugin for this service, or \
             (3) provide the content directly."
                .to_string(),
        )
    } else {
        None
    }
}

/// Detect HTTP error pages (404, 503, etc.) from navigate results.
/// Returns a warning hint if the page title or content indicates an error page.
fn detect_error_page(content: &str) -> Option<String> {
    let content_lower = content.to_lowercase();

    let title_error = content_lower.contains("title: \"404")
        || content_lower.contains("title: \"not found")
        || content_lower.contains("title: \"page not found")
        || content_lower.contains("title: \"error")
        || content_lower.contains("title: \"403")
        || content_lower.contains("title: \"503")
        || content_lower.contains("title: \"502")
        || content_lower.contains("title: \"access denied")
        || content_lower.contains("title: \"server error");

    let body_error = content_lower.contains("oops! we are having trouble")
        || content_lower.contains("this page isn't available")
        || content_lower.contains("this page can't be found")
        || content_lower.contains("the page you requested was not found");

    if title_error || body_error {
        Some(
            "⚠ ERROR PAGE — This URL returned a 404/error page. Do NOT call read_page on this page. \
             Instead: navigate to the site's homepage and use their search function, \
             or use web(action: \"search\") to find a working URL."
                .to_string(),
        )
    } else {
        None
    }
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

/// Extract scheme + host from a URL string (e.g. "https://example.com").
fn extract_origin(url: &str) -> String {
    if let Some(after_scheme) = url.find("://") {
        let host_start = after_scheme + 3;
        let host_end = url[host_start..]
            .find('/')
            .map(|i| host_start + i)
            .unwrap_or(url.len());
        url[..host_end].to_string()
    } else {
        String::new()
    }
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
/// Generic search-results extractor. Works on ANY engine's results page by harvesting external
/// result links + their anchor text — there are NO per-engine class selectors to rot when a site
/// changes its markup (every organic result is fundamentally `<a href="external">title</a>`).
/// Decodes DuckDuckGo's `uddg=` redirect wrapper, drops the engine's own + social/nav links, and
/// dedups by normalized URL. This mirrors the reference harness's "generic extraction, no
/// hardcoded selectors" approach (its WebFetch returns clean text the same way).
fn extract_search_links(html: &str, engine_host: &str) -> Vec<SearchResult> {
    const JUNK_HOSTS: &[&str] = &[
        "duckduckgo.com",
        "brave.com",
        "bing.com",
        "google.com",
        "microsoft.com",
        "facebook.com",
        "twitter.com",
        "x.com",
        "instagram.com",
        "youtube.com",
        "pinterest.com",
        "tiktok.com",
    ];
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for piece in html.split("<a ").skip(1) {
        let Some(tag_end) = piece.find('>') else {
            continue;
        };
        let tag = &piece[..tag_end];
        let inner = &piece[tag_end + 1..];

        // Raw href value.
        let Some(h0) = tag.find("href=\"") else {
            continue;
        };
        let after = &tag[h0 + 6..];
        let Some(h1) = after.find('"') else {
            continue;
        };
        let mut url = after[..h1].replace("&amp;", "&");

        // Decode DuckDuckGo's redirect wrapper: //duckduckgo.com/l/?uddg=ENCODED&...
        if let Some(i) = url.find("uddg=") {
            let enc = &url[i + 5..];
            let end = enc.find('&').unwrap_or(enc.len());
            if let Ok(dec) = urlencoding::decode(&enc[..end]) {
                url = dec.into_owned();
            }
        }
        if let Some(rest) = url.strip_prefix("//") {
            url = format!("https://{rest}");
        }
        if !url.starts_with("http") {
            continue;
        }

        // Host: drop the engine's own links + obvious social/nav junk.
        let host = url
            .split("://")
            .nth(1)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("")
            .trim_start_matches("www.")
            .to_ascii_lowercase();
        if host.is_empty()
            || host == engine_host
            || host.ends_with(&format!(".{engine_host}"))
            || JUNK_HOSTS
                .iter()
                .any(|j| host == *j || host.ends_with(&format!(".{j}")))
        {
            continue;
        }

        // Title = the anchor's inner text, tags stripped + whitespace collapsed.
        let raw_title = inner.split("</a>").next().unwrap_or("");
        let title = strip_html(raw_title)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if title.len() < 3 || title.len() > 300 {
            continue;
        }

        // Dedup by URL without query/fragment/trailing slash.
        let key = url
            .split(['?', '#'])
            .next()
            .unwrap_or(&url)
            .trim_end_matches('/')
            .to_ascii_lowercase();
        if !seen.insert(key) {
            continue;
        }

        results.push(SearchResult {
            title,
            url,
            snippet: String::new(),
        });
        if results.len() >= 10 {
            break;
        }
    }

    results
}

/// Normalize a model-written search query into something a keyword engine accepts.
/// Weak models stuff queries with stacked `site:` operators and run them hundreds of chars
/// long; DuckDuckGo rejects those ("Search query entered was too long") and returns nothing.
/// We strip excessive `site:` filters (2+ is the spam pattern, not a real intent) and clamp
/// the length at a word boundary.
fn normalize_search_query(raw: &str) -> String {
    let trimmed = raw.trim();
    let cleaned = if trimmed.matches("site:").count() >= 2 {
        trimmed
            .split_whitespace()
            .filter(|tok| {
                let t = tok.trim_matches(|c| c == '(' || c == ')' || c == '"');
                !t.starts_with("site:") && !t.eq_ignore_ascii_case("OR")
            })
            .collect::<Vec<&str>>()
            .join(" ")
    } else {
        trimmed.to_string()
    };

    const MAX_CHARS: usize = 400;
    if cleaned.chars().count() <= MAX_CHARS {
        return cleaned;
    }
    let mut out = String::new();
    for word in cleaned.split_whitespace() {
        if out.chars().count() + word.chars().count() + 1 > MAX_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// If a URL clearly points to a downloadable binary file (by path extension), return that
/// extension. Navigating the user's real browser to such a URL only triggers a download + OS
/// save dialog (it can't render it), so callers skip the navigation instead.
fn file_download_ext(url: &str) -> Option<&'static str> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let lower = path.to_ascii_lowercase();
    const EXTS: &[&str] = &[
        "pdf", "doc", "docx", "ppt", "pptx", "xls", "xlsx", "zip", "rar", "7z", "tar", "gz",
        "dmg", "exe", "csv", "epub", "mp4", "mp3", "wav", "mov",
    ];
    EXTS.iter()
        .find(|ext| lower.ends_with(format!(".{ext}").as_str()))
        .copied()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_auth_page_twitter_login() {
        let url = "https://x.com/i/flow/login";
        let content = r#"heading "Sign in to X" [ref_1]
link "Sign in with Google" [ref_2]
textbox "Phone, email, or username" [ref_3]
link "Forgot password?" [ref_4]
button "Next" [ref_5]"#;
        let result = detect_auth_page(url, content);
        assert!(result.is_some(), "should detect Twitter login page");
        assert!(result.unwrap().contains("AUTHENTICATION REQUIRED"));
    }

    #[test]
    fn test_detect_auth_page_github_login() {
        let url = "https://github.com/login";
        let content = r#"heading "Sign in to GitHub" [ref_1]
textbox "Username or email address" [ref_2]
input [ref_3] type="password"
button "Sign in" [ref_4]
link "Forgot password?" [ref_5]"#;
        let result = detect_auth_page(url, content);
        assert!(result.is_some(), "should detect GitHub login page");
    }

    #[test]
    fn test_detect_auth_page_normal_page() {
        let url = "https://docs.rust-lang.org/book/ch01-01-installation.html";
        let content = r#"heading "Installation" [ref_1]
paragraph "The first step is to install Rust."
link "rustup" [ref_2]
code "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
heading "Troubleshooting" [ref_3]"#;
        let result = detect_auth_page(url, content);
        assert!(result.is_none(), "should not flag normal documentation page");
    }

    #[test]
    fn test_detect_auth_page_settings_with_password_mention() {
        let url = "https://example.com/settings/security";
        let content = r#"heading "Security Settings" [ref_1]
paragraph "Change your password"
link "Update password" [ref_2]
link "Two-factor authentication" [ref_3]"#;
        let result = detect_auth_page(url, content);
        assert!(
            result.is_none(),
            "should not flag settings page that merely mentions password"
        );
    }

    #[test]
    fn test_detect_auth_page_oauth_redirect() {
        let url = "https://accounts.google.com/signin/oauth";
        let content = r#"heading "Sign in" [ref_1]
textbox "Email or phone" [ref_2]
link "Forgot email?" [ref_3]
button "Next" [ref_4]
link "Create account" [ref_5]"#;
        let result = detect_auth_page(url, content);
        assert!(result.is_some(), "should detect Google OAuth login");
    }
}
