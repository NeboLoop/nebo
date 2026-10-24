//! MCP Apps (`io.modelcontextprotocol/ui`, spec 2026-01-26): a tool's
//! result shown as an interactive view, from a `ui://` resource the server
//! declares on the tool.
//!
//! This module holds the extension's wire types as the spec names them, the
//! tool metadata that links a tool to its view and says who may call it
//! (`_meta.ui.resourceUri`, `_meta.ui.visibility`), the resource metadata
//! that says what the view may reach (`csp`, `permissions`, `domain`,
//! `prefersBorder`), and the ONE builder of the Content-Security-Policy a
//! view runs under ([`csp_policy`]), which every client uses.

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::{McpError, McpToolDef};

/// The extension's identifier, advertised in the client's capabilities.
pub const EXTENSION_ID: &str = "io.modelcontextprotocol/ui";

/// The one content type a view may have.
pub const RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

/// Where a view's resource lives: the `ui://` scheme.
pub const UI_SCHEME: &str = "ui://";

/// This client's entry in `capabilities.extensions` of `initialize`.
pub fn client_extension() -> serde_json::Value {
    serde_json::json!({ "mimeTypes": [RESOURCE_MIME_TYPE] })
}

/// Who may call a tool (`_meta.ui.visibility`): the employee (`model`), a
/// view from the same server (`app`), or both, the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visibility {
    pub model: bool,
    pub app: bool,
}

impl Default for Visibility {
    fn default() -> Self {
        Self { model: true, app: true }
    }
}

/// The origins a view may reach (`_meta.ui.csp`), as the server declares them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiCsp {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub connect_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frame_domains: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub base_uri_domains: Vec<String>,
}

/// The browser features a view asks for (`_meta.ui.permissions`); each is
/// asked for by being present (`{}`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPermissions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geolocation: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clipboard_write: Option<serde_json::Value>,
}

/// A view resource's `_meta.ui`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiResourceMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csp: Option<UiCsp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<UiPermissions>,
    /// The server's wish for a dedicated origin. Nebo serves every view from
    /// its one view origin, so it is kept and passed on, never acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefers_border: Option<bool>,
}

/// A view, as `resources/read` returned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiResource {
    pub uri: String,
    pub html: String,
    pub meta: UiResourceMeta,
}

impl McpToolDef {
    fn ui_meta(&self) -> Option<&serde_json::Map<String, serde_json::Value>> {
        self.meta.as_ref()?.get("ui")?.as_object()
    }

    /// The `ui://` resource the tool's results are shown with:
    /// `_meta.ui.resourceUri`, else the deprecated flat
    /// `_meta["ui/resourceUri"]`. `None` for a tool without a view.
    pub fn ui_resource_uri(&self) -> Option<String> {
        let nested = self.ui_meta().and_then(|ui| ui.get("resourceUri"));
        let flat = || self.meta.as_ref()?.get("ui/resourceUri");
        nested
            .or_else(flat)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|uri| uri.starts_with(UI_SCHEME) && uri.len() > UI_SCHEME.len())
            .map(str::to_string)
    }

    /// Who may call the tool. An absent or unreadable `visibility` is the
    /// default, both.
    pub fn visibility(&self) -> Visibility {
        let Some(list) = self.ui_meta().and_then(|ui| ui.get("visibility")).and_then(|v| v.as_array()) else {
            return Visibility::default();
        };
        let has = |who: &str| list.iter().any(|v| v.as_str() == Some(who));
        Visibility { model: has("model"), app: has("app") }
    }
}

/// Whether `mime` is the view content type, written with any case or
/// spacing around its parameter.
pub fn is_view_mime(mime: &str) -> bool {
    let normalized: String = mime.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    normalized == RESOURCE_MIME_TYPE
}

/// The view in a `resources/read` result for `uri`: the content item with
/// that URI (the first one when none names it), which must be
/// `text/html;profile=mcp-app` and carry its HTML as `text` or base64 `blob`.
pub fn view_from_read(uri: &str, result: &serde_json::Value) -> Result<UiResource, McpError> {
    let contents = result
        .get("contents")
        .and_then(|c| c.as_array())
        .ok_or_else(|| McpError::Other(format!("resources/read {uri}: no contents")))?;
    let item = contents
        .iter()
        .find(|c| c.get("uri").and_then(|u| u.as_str()) == Some(uri))
        .or_else(|| contents.first())
        .ok_or_else(|| McpError::Other(format!("resources/read {uri}: no contents")))?;
    let mime = item.get("mimeType").and_then(|m| m.as_str()).unwrap_or("");
    if !is_view_mime(mime) {
        return Err(McpError::Other(format!(
            "resources/read {uri}: content type {mime:?} is not {RESOURCE_MIME_TYPE}"
        )));
    }
    let html = match (item.get("text").and_then(|t| t.as_str()), item.get("blob").and_then(|b| b.as_str())) {
        (Some(text), _) => text.to_string(),
        (None, Some(blob)) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(blob.trim())
                .map_err(|e| McpError::Other(format!("resources/read {uri}: blob is not base64: {e}")))?;
            String::from_utf8(bytes).map_err(|e| McpError::Other(format!("resources/read {uri}: blob is not UTF-8: {e}")))?
        }
        (None, None) => return Err(McpError::Other(format!("resources/read {uri}: no text or blob"))),
    };
    if html.trim().is_empty() {
        return Err(McpError::Other(format!("resources/read {uri}: empty view")));
    }
    let meta = item
        .get("_meta")
        .and_then(|m| m.get("ui"))
        .cloned()
        .and_then(|ui| serde_json::from_value::<UiResourceMeta>(ui).ok())
        .unwrap_or_default();
    Ok(UiResource { uri: uri.to_string(), html, meta })
}

/// The sources of one declared list a view may use: each an origin
/// (`scheme://host[:port]`, a wildcard subdomain `https://*.example.com`
/// allowed) with an http, https, ws or wss scheme. Anything that could
/// change the policy's meaning is dropped: whitespace, `;`, `,` or quotes
/// (a new directive or a keyword), a bare scheme or `*` (everything), and
/// this computer or its network (loopback, private and link-local hosts),
/// where Nebo itself and the owner's devices answer. The spec lets a host
/// narrow what a server declares; it never widens it.
pub fn allowed_sources(declared: &[String]) -> Vec<String> {
    declared.iter().map(|d| d.trim()).filter(|d| is_allowed_source(d)).map(str::to_string).collect()
}

fn is_allowed_source(source: &str) -> bool {
    if source.is_empty() || source.chars().any(|c| c.is_whitespace() || matches!(c, ';' | ',' | '\'' | '"')) {
        return false;
    }
    let Some((scheme, rest)) = source.split_once("://") else { return false };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "ws" | "wss") {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or("");
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => authority.rsplit_once(':').map_or(authority, |(h, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) || port == "*" { h } else { authority }
        }),
    };
    let host = host.to_ascii_lowercase();
    let name = host.strip_prefix("*.").unwrap_or(&host);
    !name.is_empty() && !name.contains('*') && !is_local_host(name)
}

/// This computer or its network: where a view must never reach.
fn is_local_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host == "0.0.0.0" {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified()
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            let first = ip.segments()[0];
            ip.is_loopback() || ip.is_unspecified() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

/// The Content-Security-Policy a view runs under, as the spec's Security
/// section builds it from the resource's `_meta.ui.csp`:
///
/// ```text
/// default-src 'none';
/// script-src 'self' 'unsafe-inline' <resourceDomains>;
/// style-src 'self' 'unsafe-inline' <resourceDomains>;
/// connect-src 'self' <connectDomains>;
/// img-src 'self' data: <resourceDomains>;
/// font-src 'self' <resourceDomains>;
/// media-src 'self' data: <resourceDomains>;
/// frame-src <frameDomains> | 'none';
/// object-src 'none';
/// base-uri <baseUriDomains> | 'self'
/// ```
///
/// A resource that declares no `csp` gets the spec's restrictive default
/// (`connect-src 'none'`). Every declared source passes
/// [`allowed_sources`] first, so an undeclared domain is never reachable
/// and a declared one can't loosen the policy. The ONE builder: the
/// desktop's view origin serves it as the view's header, and the stored
/// result carries it for every other client.
pub fn csp_policy(csp: Option<&UiCsp>) -> String {
    let list = |pick: fn(&UiCsp) -> &Vec<String>| csp.map(|c| allowed_sources(pick(c))).unwrap_or_default();
    let resources = list(|c| &c.resource_domains);
    let connects = list(|c| &c.connect_domains);
    let frames = list(|c| &c.frame_domains);
    let bases = list(|c| &c.base_uri_domains);
    let with = |head: &str, extra: &[String]| {
        if extra.is_empty() { head.to_string() } else { format!("{head} {}", extra.join(" ")) }
    };
    let connect = if csp.is_none() { "connect-src 'none'".to_string() } else { with("connect-src 'self'", &connects) };
    let frame = if frames.is_empty() { "frame-src 'none'".to_string() } else { with("frame-src", &frames) };
    let base = if bases.is_empty() { "base-uri 'self'".to_string() } else { with("base-uri", &bases) };
    [
        "default-src 'none'".to_string(),
        with("script-src 'self' 'unsafe-inline'", &resources),
        with("style-src 'self' 'unsafe-inline'", &resources),
        connect,
        with("img-src 'self' data:", &resources),
        with("font-src 'self'", &resources),
        with("media-src 'self' data:", &resources),
        frame,
        "object-src 'none'".to_string(),
        base,
    ]
    .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def(meta: serde_json::Value) -> McpToolDef {
        serde_json::from_value(json!({"name": "show-map", "_meta": meta})).unwrap()
    }

    #[test]
    fn a_tools_view_is_its_nested_uri_then_the_deprecated_flat_one() {
        assert_eq!(def(json!({"ui": {"resourceUri": "ui://map/app.html"}})).ui_resource_uri().as_deref(), Some("ui://map/app.html"));
        assert_eq!(def(json!({"ui/resourceUri": "ui://map/old.html"})).ui_resource_uri().as_deref(), Some("ui://map/old.html"));
        assert_eq!(
            def(json!({"ui": {"resourceUri": "ui://map/new.html"}, "ui/resourceUri": "ui://map/old.html"})).ui_resource_uri().as_deref(),
            Some("ui://map/new.html")
        );
        assert_eq!(def(json!({"ui": {"resourceUri": "https://map/app.html"}})).ui_resource_uri(), None, "only ui://");
        let plain: McpToolDef = serde_json::from_value(json!({"name": "geocode"})).unwrap();
        assert_eq!(plain.ui_resource_uri(), None);
    }

    #[test]
    fn visibility_defaults_to_both_and_reads_what_is_listed() {
        let plain: McpToolDef = serde_json::from_value(json!({"name": "geocode"})).unwrap();
        assert_eq!(plain.visibility(), Visibility { model: true, app: true });
        assert_eq!(def(json!({"ui": {"visibility": ["app"]}})).visibility(), Visibility { model: false, app: true });
        assert_eq!(def(json!({"ui": {"visibility": ["model"]}})).visibility(), Visibility { model: true, app: false });
        assert_eq!(def(json!({"ui": {"visibility": ["model", "app"]}})).visibility(), Visibility { model: true, app: true });
    }

    #[test]
    fn a_read_view_must_be_the_mcp_app_type_with_text_or_blob() {
        let meta = json!({"ui": {"csp": {"connectDomains": ["https://api.example.com"]}, "permissions": {"geolocation": {}}, "domain": "x.example", "prefersBorder": true}});
        let read = json!({"contents": [{"uri": "ui://m/a", "mimeType": "text/html;profile=mcp-app", "text": "<!doctype html><p>hi", "_meta": meta}]});
        let view = view_from_read("ui://m/a", &read).unwrap();
        assert_eq!(view.html, "<!doctype html><p>hi");
        assert_eq!(view.meta.csp.as_ref().unwrap().connect_domains, vec!["https://api.example.com"]);
        assert!(view.meta.permissions.as_ref().unwrap().geolocation.is_some());
        assert!(view.meta.permissions.as_ref().unwrap().camera.is_none());
        assert_eq!(view.meta.domain.as_deref(), Some("x.example"));
        assert_eq!(view.meta.prefers_border, Some(true));

        let blob = base64::engine::general_purpose::STANDARD.encode("<html>b</html>");
        let read = json!({"contents": [{"uri": "ui://m/a", "mimeType": "text/html; profile=mcp-app", "blob": blob}]});
        assert_eq!(view_from_read("ui://m/a", &read).unwrap().html, "<html>b</html>");

        let wrong = json!({"contents": [{"uri": "ui://m/a", "mimeType": "text/html", "text": "<p>"}]});
        assert!(view_from_read("ui://m/a", &wrong).is_err(), "plain HTML is not a view");
        assert!(view_from_read("ui://m/a", &json!({"contents": []})).is_err());
    }

    #[test]
    fn the_policy_is_the_specs_with_no_csp_declared() {
        assert_eq!(
            csp_policy(None),
            "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
             connect-src 'none'; img-src 'self' data:; font-src 'self'; media-src 'self' data:; \
             frame-src 'none'; object-src 'none'; base-uri 'self'"
        );
        assert_eq!(
            csp_policy(Some(&UiCsp::default())),
            "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
             connect-src 'self'; img-src 'self' data:; font-src 'self'; media-src 'self' data:; \
             frame-src 'none'; object-src 'none'; base-uri 'self'"
        );
    }

    #[test]
    fn the_policy_puts_each_declared_list_where_the_spec_says() {
        let csp = UiCsp {
            connect_domains: vec!["https://*.openstreetmap.org".into(), "wss://live.example.com".into()],
            resource_domains: vec!["https://cdn.example.com".into()],
            frame_domains: vec!["https://www.youtube.com".into()],
            base_uri_domains: vec!["https://cdn.example.com".into()],
        };
        assert_eq!(
            csp_policy(Some(&csp)),
            "default-src 'none'; script-src 'self' 'unsafe-inline' https://cdn.example.com; \
             style-src 'self' 'unsafe-inline' https://cdn.example.com; \
             connect-src 'self' https://*.openstreetmap.org wss://live.example.com; \
             img-src 'self' data: https://cdn.example.com; font-src 'self' https://cdn.example.com; \
             media-src 'self' data: https://cdn.example.com; frame-src https://www.youtube.com; \
             object-src 'none'; base-uri https://cdn.example.com"
        );
    }

    #[test]
    fn a_declared_source_never_loosens_the_policy() {
        let csp = UiCsp {
            connect_domains: vec![
                "https://ok.example.com".into(),
                "https://a.example.com; script-src *".into(),
                "'unsafe-eval'".into(),
                "*".into(),
                "https:".into(),
                "https://*".into(),
                "http://localhost:27895".into(),
                "http://127.0.0.1:27895".into(),
                "http://[::1]:80".into(),
                "http://192.168.1.10".into(),
                "http://10.0.0.2:8080".into(),
                "http://tauri.localhost".into(),
                "data:".into(),
                "javascript://x".into(),
            ],
            ..Default::default()
        };
        let policy = csp_policy(Some(&csp));
        assert!(policy.contains("connect-src 'self' https://ok.example.com;"), "{policy}");
        for gone in ["script-src *", "unsafe-eval", "localhost", "127.0.0.1", "::1", "192.168", "10.0.0.2", "javascript"] {
            assert!(!policy.contains(gone), "{gone} in {policy}");
        }
        assert_eq!(policy.matches("connect-src").count(), 1);
    }
}
