use std::sync::Arc;

use crate::domain::DomainInput;
use crate::origin::ToolContext;
use crate::registry::{DynTool, ToolResult};

/// WebTool consolidates web operations: HTTP fetch, search, and browser automation.
pub struct WebTool {
    client: reqwest::Client,
    browser: Option<Arc<browser::Manager>>,
}

impl WebTool {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("Nebo/1.0")
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            client,
            browser: None,
        }
    }

    pub fn with_browser(mut self, manager: Arc<browser::Manager>) -> Self {
        self.browser = Some(manager);
        self
    }

    fn infer_resource(&self, action: &str) -> &str {
        match action {
            "fetch" | "get" | "post" | "put" | "delete" | "head" => "http",
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

        // Use DuckDuckGo HTML search (no API key needed)
        let search_url = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(query)
        );

        match self.client.get(&search_url).send().await {
            Ok(resp) => match resp.text().await {
                Ok(html) => {
                    let results = parse_ddg_results(&html);
                    if results.is_empty() {
                        ToolResult::ok(format!("No results found for: {}", query))
                    } else {
                        let formatted: Vec<String> = results
                            .iter()
                            .enumerate()
                            .map(|(i, r)| {
                                format!("{}. {}\n   {}\n   {}", i + 1, r.title, r.url, r.snippet)
                            })
                            .collect();
                        ToolResult::ok(format!(
                            "Search results for: {}\n\n{}",
                            query,
                            formatted.join("\n\n")
                        ))
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
        "Web operations — HTTP requests, search, and browser automation.\n\n\
         Resources and Actions:\n\
         - http: fetch (HTTP request with any method)\n\
         - search: query (Web search via DuckDuckGo)\n\
         - browser: navigate, read_page, click, double_click, triple_click, right_click, hover, fill, form_input, type, select, screenshot, scroll, scroll_to, press, drag, go_back, go_forward, wait, evaluate\n\n\
         Browser workflow: read_page returns an accessibility tree with element refs. Use refs for click/fill/select.\n\
         For React/modern sites: use click on field first, then type to fill (character-by-character via real key events).\n\
         triple_click selects all text in a field, then type to replace it.\n\n\
         Examples:\n  \
         web(action: \"fetch\", url: \"https://example.com\")\n  \
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
         web(action: \"press\", key: \"ArrowDown ArrowDown Enter\") — key sequence"
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
                    "enum": ["fetch", "get", "post", "put", "delete", "head", "search", "query",
                             "navigate", "read_page", "snapshot", "click", "double_click",
                             "triple_click", "right_click", "hover", "fill", "form_input",
                             "type", "select", "screenshot", "scroll", "scroll_to", "press",
                             "key", "drag", "go_back", "go_forward", "wait", "evaluate",
                             "list_tabs", "new_tab", "close_tab", "status"]
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
                }
            },
            "required": ["action"]
        })
    }

    fn requires_approval(&self) -> bool {
        true
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
                    "Resource is required. Available: http, search, browser",
                );
            }

            match resource.as_str() {
                "http" => self.handle_http(&input).await,
                "search" => self.handle_search(&input).await,
                "browser" | "devtools" => self.handle_browser(&input).await,
                other => ToolResult::error(format!(
                    "Resource {:?} not available. Available: http, search, browser",
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

/// Parse DuckDuckGo HTML search results.
fn parse_ddg_results(html: &str) -> Vec<SearchResult> {
    let mut results = Vec::new();

    // DuckDuckGo HTML results are in <a class="result__a"> tags
    // with <a class="result__snippet"> for snippets
    for chunk in html.split("class=\"result__body\"") {
        if results.len() >= 10 {
            break;
        }

        let title = extract_between(chunk, "class=\"result__a\"", "</a>")
            .map(|s| strip_html(&s))
            .unwrap_or_default();

        let url = extract_attr(chunk, "class=\"result__a\"", "href")
            .unwrap_or_default();

        let snippet = extract_between(chunk, "class=\"result__snippet\"", "</a>")
            .or_else(|| extract_between(chunk, "class=\"result__snippet\"", "</td>"))
            .map(|s| strip_html(&s))
            .unwrap_or_default();

        if !title.is_empty() && !url.is_empty() {
            results.push(SearchResult {
                title: title.trim().to_string(),
                url: clean_ddg_url(&url),
                snippet: snippet.trim().to_string(),
            });
        }
    }

    results
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

fn extract_attr(html: &str, marker: &str, attr: &str) -> Option<String> {
    let start_idx = html.find(marker)?;
    let before_marker = &html[..start_idx];
    // Walk backward to find the opening < tag
    let tag_start = before_marker.rfind('<')?;
    let tag_content = &html[tag_start..start_idx + marker.len()];
    // Find the attribute
    let attr_pattern = format!("{}=\"", attr);
    let attr_idx = tag_content.find(&attr_pattern)?;
    let after_attr = &tag_content[attr_idx + attr_pattern.len()..];
    let end_quote = after_attr.find('"')?;
    Some(after_attr[..end_quote].to_string())
}

fn clean_ddg_url(url: &str) -> String {
    // DuckDuckGo wraps URLs in redirect: //duckduckgo.com/l/?uddg=ENCODED_URL
    if let Some(idx) = url.find("uddg=") {
        let encoded = &url[idx + 5..];
        let end = encoded.find('&').unwrap_or(encoded.len());
        urlencoding::decode(&encoded[..end])
            .unwrap_or_else(|_| encoded[..end].into())
            .to_string()
    } else {
        url.to_string()
    }
}
