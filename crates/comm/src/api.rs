//! NeboLoop REST API client covering all 5 hierarchy layers + loops.
//!
//! Independent of the WebSocket client — uses the owner's OAuth JWT directly.

use std::sync::RwLock;

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::debug;

use crate::api_types::*;
use crate::CommError;

/// NeboLoop REST API client.
pub struct NeboLoopApi {
    api_server: String,
    bot_id: String,
    token: RwLock<String>,
    client: Client,
}

/// Default production API server.
pub const DEFAULT_API_SERVER: &str = "https://api.neboloop.com";

impl NeboLoopApi {
    /// Create a new API client.
    pub fn new(api_server: String, bot_id: String, token: String) -> Self {
        Self {
            api_server,
            bot_id,
            token: RwLock::new(token),
            client: Client::new(),
        }
    }

    /// Create from a settings map (keys: api_server, bot_id, token).
    pub fn from_settings(settings: &std::collections::HashMap<String, String>) -> Result<Self, CommError> {
        let api_server = settings
            .get("api_server")
            .ok_or_else(|| CommError::Other("api_server not configured".into()))?
            .clone();
        let bot_id = settings
            .get("bot_id")
            .ok_or_else(|| CommError::Other("bot_id not configured".into()))?
            .clone();
        let token = settings
            .get("token")
            .ok_or_else(|| CommError::Other("token not configured".into()))?
            .clone();
        Ok(Self::new(api_server, bot_id, token))
    }

    pub fn api_server(&self) -> &str {
        &self.api_server
    }

    pub fn bot_id(&self) -> &str {
        &self.bot_id
    }

    /// Update the auth token (e.g. after token refresh).
    pub fn set_token(&self, token: String) {
        *self.token.write().unwrap_or_else(|p| p.into_inner()) = token;
    }

    // ── Internal helpers ────────────────────────────────────────────

    fn token(&self) -> String {
        self.token.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    async fn do_json<T: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T, CommError> {
        let url = format!("{}{}", self.api_server, path);
        debug!(method = %method, url = %url, "neboloop api");

        let mut req = self.client.request(method, &url)
            .bearer_auth(self.token());

        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req.send().await.map_err(|e| CommError::Other(format!("request failed: {}", e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!("NeboLoop returned {}: {}", status, body)));
        }

        resp.json::<T>()
            .await
            .map_err(|e| CommError::Other(format!("decode response: {}", e)))
    }

    async fn do_void(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<(), CommError> {
        let url = format!("{}{}", self.api_server, path);
        let mut req = self.client.request(method, &url)
            .bearer_auth(self.token());

        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req.send().await.map_err(|e| CommError::Other(format!("request failed: {}", e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!("NeboLoop returned {}: {}", status, body)));
        }
        Ok(())
    }

    // ── Apps / Tools ────────────────────────────────────────────────

    /// List apps from NeboLoop catalog.
    pub async fn list_apps(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<AppsResponse, CommError> {
        let path = format!("/api/v1/apps{}", build_query(query, category, page, page_size));
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Get a single app with manifest.
    pub async fn get_app(&self, id: &str) -> Result<AppDetail, CommError> {
        self.do_json(reqwest::Method::GET, &format!("/api/v1/apps/{}", id), None::<&()>).await
    }

    /// Get reviews for an app.
    pub async fn get_app_reviews(
        &self,
        id: &str,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<ReviewsResponse, CommError> {
        let path = format!("/api/v1/apps/{}/reviews{}", id, build_query(None, None, page, page_size));
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Install an app for this bot.
    pub async fn install_app(&self, id: &str) -> Result<InstallResponse, CommError> {
        let body = serde_json::json!({ "botId": self.bot_id });
        self.do_json(reqwest::Method::POST, &format!("/api/v1/apps/{}/install", id), Some(&body)).await
    }

    /// Uninstall an app for this bot.
    pub async fn uninstall_app(&self, id: &str) -> Result<(), CommError> {
        let body = serde_json::json!({ "botId": self.bot_id });
        self.do_void(reqwest::Method::DELETE, &format!("/api/v1/apps/{}/install", id), Some(&body)).await
    }

    // ── Skills ──────────────────────────────────────────────────────

    /// List skills from NeboLoop catalog.
    pub async fn list_skills(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<SkillsResponse, CommError> {
        let path = format!("/api/v1/skills{}", build_query(query, category, page, page_size));
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Get a single skill with manifest.
    pub async fn get_skill(&self, id: &str) -> Result<SkillDetail, CommError> {
        self.do_json(reqwest::Method::GET, &format!("/api/v1/skills/{}", id), None::<&()>).await
    }

    // ── Universal Code Redemption ────────────────────────────────────

    /// Redeem any marketplace code (SKIL-*, WORK-*, ROLE-*) via the universal endpoint.
    pub async fn redeem_code(&self, code: &str) -> Result<CodeRedeemResponse, CommError> {
        let body = serde_json::json!({
            "code": code,
            "botIds": [self.bot_id],
        });
        self.do_json(reqwest::Method::POST, "/api/v1/codes/redeem", Some(&body)).await
    }

    /// Install a skill for this bot via universal code redemption.
    pub async fn install_skill(&self, code: &str) -> Result<CodeRedeemResponse, CommError> {
        self.redeem_code(code).await
    }

    /// Download a sealed .napp archive from a URL.
    ///
    /// The URL can be absolute (CDN) or relative (API path like `/api/v1/artifacts/{id}/download`).
    /// Returns the raw bytes of the .napp tar.gz archive.
    pub async fn download_napp(&self, url: &str) -> Result<Vec<u8>, CommError> {
        let full_url = if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            format!("{}{}", self.api_server, url)
        };
        debug!(url = %full_url, "downloading .napp archive");
        self.fetch_raw(&full_url).await
    }

    /// Uninstall a skill for this bot.
    pub async fn uninstall_skill(&self, id: &str) -> Result<(), CommError> {
        self.do_void(reqwest::Method::DELETE, &format!("/api/v1/skills/{}/install/{}", id, self.bot_id), None::<&()>).await
    }

    // ── Workflows ───────────────────────────────────────────────────

    /// Install a workflow for this bot via universal code redemption.
    pub async fn install_workflow(&self, code: &str) -> Result<CodeRedeemResponse, CommError> {
        self.redeem_code(code).await
    }

    /// Uninstall a workflow for this bot.
    pub async fn uninstall_workflow(&self, id: &str) -> Result<(), CommError> {
        self.do_void(reqwest::Method::DELETE, &format!("/api/v1/workflows/{}/install/{}", id, self.bot_id), None::<&()>).await
    }

    /// List workflows from NeboLoop catalog.
    pub async fn list_workflows(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<WorkflowsResponse, CommError> {
        let path = format!("/api/v1/workflows{}", build_query(query, category, page, page_size));
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    // ── Roles ───────────────────────────────────────────────────────

    /// Install a role for this bot via universal code redemption.
    pub async fn install_role(&self, code: &str) -> Result<CodeRedeemResponse, CommError> {
        self.redeem_code(code).await
    }

    /// Uninstall a role for this bot.
    pub async fn uninstall_role(&self, id: &str) -> Result<(), CommError> {
        self.do_void(reqwest::Method::DELETE, &format!("/api/v1/roles/{}/install/{}", id, self.bot_id), None::<&()>).await
    }

    // ── Bot Identity ────────────────────────────────────────────────

    /// Push bot name and role to NeboLoop.
    pub async fn update_bot_identity(&self, name: &str, role: &str) -> Result<(), CommError> {
        let body = UpdateBotIdentityRequest {
            name: name.into(),
            role: role.into(),
        };
        self.do_void(reqwest::Method::PUT, &format!("/api/v1/bots/{}", self.bot_id), Some(&body)).await
    }

    // ── Loops ───────────────────────────────────────────────────────

    /// Join the bot to a loop using an invite code.
    pub async fn join_loop(&self, code: &str) -> Result<JoinLoopResponse, CommError> {
        let body = JoinLoopRequest {
            code: code.into(),
            bot_id: self.bot_id.clone(),
        };
        self.do_json(reqwest::Method::POST, "/api/v1/loops/join", Some(&body)).await
    }

    /// List all loops this bot belongs to.
    pub async fn list_bot_loops(&self) -> Result<Vec<crate::api_types::Loop>, CommError> {
        let resp: LoopsResponse = self.do_json(reqwest::Method::GET, &format!("/api/v1/bots/{}/loops", self.bot_id), None::<&()>).await?;
        Ok(resp.loops)
    }

    /// Get a single loop by ID.
    pub async fn get_loop(&self, loop_id: &str) -> Result<crate::api_types::Loop, CommError> {
        self.do_json(reqwest::Method::GET, &format!("/api/v1/bots/{}/loops/{}", self.bot_id, loop_id), None::<&()>).await
    }

    /// List members of a loop with online presence.
    pub async fn list_loop_members(&self, loop_id: &str) -> Result<Vec<LoopMember>, CommError> {
        let resp: LoopMembersResponse = self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/bots/{}/loops/{}/members", self.bot_id, loop_id),
            None::<&()>,
        ).await?;
        Ok(resp.members)
    }

    // ── Channels ────────────────────────────────────────────────────

    /// List all channels this bot belongs to across all loops.
    pub async fn list_bot_channels(&self) -> Result<Vec<LoopChannel>, CommError> {
        let resp: LoopChannelsResponse = self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/bots/{}/channels", self.bot_id),
            None::<&()>,
        ).await?;
        Ok(resp.channels)
    }

    /// List members of a channel.
    pub async fn list_channel_members(&self, channel_id: &str) -> Result<Vec<ChannelMember>, CommError> {
        let resp: ChannelMembersResponse = self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/bots/{}/channels/{}/members", self.bot_id, channel_id),
            None::<&()>,
        ).await?;
        Ok(resp.members)
    }

    /// Fetch recent messages from a channel (normalized).
    pub async fn list_channel_messages(&self, channel_id: &str, limit: Option<i64>) -> Result<Vec<NormalizedChannelMessage>, CommError> {
        let mut path = format!("/api/v1/bots/{}/channels/{}/messages", self.bot_id, channel_id);
        if let Some(l) = limit {
            path.push_str(&format!("?limit={}", l));
        }
        let resp: ChannelMessagesResponse = self.do_json(reqwest::Method::GET, &path, None::<&()>).await?;
        Ok(resp.normalize())
    }

    // ── Raw Fetch ───────────────────────────────────────────────────

    /// Download raw content from a URL using the client's auth header.
    pub async fn fetch_raw(&self, url: &str) -> Result<Vec<u8>, CommError> {
        let resp = self
            .client
            .get(url)
            .bearer_auth(self.token())
            .send()
            .await
            .map_err(|e| CommError::Other(format!("fetch failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!("NeboLoop returned {}: {}", status, body)));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| CommError::Other(format!("read body: {}", e)))
    }

}

// ── Standalone functions (pre-auth, no client instance needed) ───────

/// Redeem a connection code for bot credentials.
/// Unauthenticated — used during initial setup.
pub async fn redeem_code(
    api_server: &str,
    code: &str,
    name: &str,
    purpose: &str,
    bot_id: &str,
) -> Result<RedeemCodeResponse, CommError> {
    let client = Client::new();
    let url = format!("{}/api/v1/bots/connect/redeem", api_server);
    let body = RedeemCodeRequest {
        code: code.into(),
        name: name.into(),
        purpose: purpose.into(),
        bot_id: bot_id.into(),
    };

    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| CommError::Other(format!("request failed: {}", e)))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(CommError::Other(format!("NeboLoop returned {}: {}", status, body)));
    }

    resp.json::<RedeemCodeResponse>()
        .await
        .map_err(|e| CommError::Other(format!("decode response: {}", e)))
}

// ── Query builder ───────────────────────────────────────────────────

fn build_query(
    query: Option<&str>,
    category: Option<&str>,
    page: Option<i64>,
    page_size: Option<i64>,
) -> String {
    let mut params = Vec::new();
    if let Some(q) = query {
        if !q.is_empty() {
            params.push(format!("q={}", urlencoding::encode(q)));
        }
    }
    if let Some(c) = category {
        if !c.is_empty() {
            params.push(format!("category={}", urlencoding::encode(c)));
        }
    }
    if let Some(p) = page {
        if p > 0 {
            params.push(format!("page={}", p));
        }
    }
    if let Some(ps) = page_size {
        if ps > 0 {
            params.push(format!("pageSize={}", ps));
        }
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}
