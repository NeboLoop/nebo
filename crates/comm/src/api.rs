//! NeboAI REST API client covering all 5 hierarchy layers + loops.
//!
//! Independent of the WebSocket client — uses the owner's OAuth JWT directly.

use std::sync::RwLock;

use reqwest::Client;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tracing::debug;

use crate::CommError;
use crate::api_types::*;

/// NeboAI REST API client.
pub struct NeboAIApi {
    api_server: String,
    bot_id: String,
    token: RwLock<String>,
    client: Client,
}

/// Default production API server.
pub const DEFAULT_API_SERVER: &str = "https://api.neboai.com";

impl NeboAIApi {
    /// Create a new API client.
    pub fn new(api_server: String, bot_id: String, token: String) -> Self {
        Self {
            api_server,
            bot_id,
            token: RwLock::new(token),
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| Client::new()),
        }
    }

    /// Create from a settings map (keys: api_server, bot_id, token).
    pub fn from_settings(
        settings: &std::collections::HashMap<String, String>,
    ) -> Result<Self, CommError> {
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
        debug!(method = %method, url = %url, "neboai api");

        let mut req = self.client.request(method, &url).bearer_auth(self.token());

        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CommError::Other(format!("request failed: {}", e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!(
                "NeboAI returned {}: {}",
                status, body
            )));
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
        let mut req = self.client.request(method, &url).bearer_auth(self.token());

        if let Some(b) = body {
            req = req.json(b);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| CommError::Other(format!("request failed: {}", e)))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!(
                "NeboAI returned {}: {}",
                status, body
            )));
        }
        Ok(())
    }

    // ── Products (unified) ─────────────────────────────────────────

    /// List products from NeboAI catalog (agents, skills, workflows).
    /// Returns `{ "skills": [...] }` regardless of type.
    pub async fn list_products(
        &self,
        artifact_type: Option<&str>,
        query: Option<&str>,
        category: Option<&str>,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<serde_json::Value, CommError> {
        let mut qs = build_query(query, category, page, page_size);
        if let Some(t) = artifact_type {
            let sep = if qs.is_empty() { "?" } else { "&" };
            qs.push_str(&format!("{}type={}", sep, urlencoding::encode(t)));
        }
        let path = format!("/api/v1/products{}", qs);
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// List marketplace collections (curated bundles). Returns NeboAI's
    /// `{ "collections": [...] }` envelope verbatim.
    pub async fn list_collections(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(reqwest::Method::GET, "/api/v1/collections", None::<&()>)
            .await
    }

    /// Get the curated marketplace reorganization map — the Employees/Tools/
    /// Collections placement keyed by artifact Code, generated from one source
    /// on NeboAI. Returns `{ departments, toolCategories, entries, responsibilities }`.
    pub async fn get_marketplace_map(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(reqwest::Method::GET, "/api/v1/marketplace/map", None::<&()>)
            .await
    }

    /// List the organizations (namespaces) the authenticated bot's owner belongs
    /// to, each with its non-public artifacts. Backs the marketplace "Shared" tab.
    /// Returns NeboAI's `{ "orgs": [...] }` envelope verbatim.
    pub async fn list_orgs(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(reqwest::Method::GET, "/api/v1/store/orgs", None::<&()>)
            .await
    }

    /// Get a single collection (with its items) by id.
    pub async fn get_collection(&self, id: &str) -> Result<serde_json::Value, CommError> {
        let path = format!("/api/v1/collections/{}", urlencoding::encode(id));
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    // ── Apps / Tools ────────────────────────────────────────────────

    /// List apps from NeboAI catalog.
    pub async fn list_apps(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<AppsResponse, CommError> {
        let path = format!(
            "/api/v1/apps{}",
            build_query(query, category, page, page_size)
        );
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Get a single app with manifest.
    pub async fn get_app(&self, id: &str) -> Result<AppDetail, CommError> {
        self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/apps/{}", id),
            None::<&()>,
        )
        .await
    }

    /// Get reviews for an app.
    pub async fn get_app_reviews(
        &self,
        id: &str,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<ReviewsResponse, CommError> {
        let path = format!(
            "/api/v1/apps/{}/reviews{}",
            id,
            build_query(None, None, page, page_size)
        );
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Install an app for this bot.
    pub async fn install_app(&self, id: &str) -> Result<InstallResponse, CommError> {
        let body = serde_json::json!({ "botId": self.bot_id });
        self.do_json(
            reqwest::Method::POST,
            &format!("/api/v1/apps/{}/install", id),
            Some(&body),
        )
        .await
    }

    /// Uninstall an app for this bot.
    pub async fn uninstall_app(&self, id: &str) -> Result<(), CommError> {
        let body = serde_json::json!({ "botId": self.bot_id });
        self.do_void(
            reqwest::Method::DELETE,
            &format!("/api/v1/apps/{}/install", id),
            Some(&body),
        )
        .await
    }

    // ── Skills ──────────────────────────────────────────────────────

    /// List skills from NeboAI catalog.
    pub async fn list_skills(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<SkillsResponse, CommError> {
        let path = format!(
            "/api/v1/skills{}",
            build_query(query, category, page, page_size)
        );
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Get a single skill with manifest.
    pub async fn get_skill(&self, id: &str) -> Result<SkillDetail, CommError> {
        self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/skills/{}", id),
            None::<&()>,
        )
        .await
    }

    /// List top/popular skills.
    pub async fn list_top_skills(
        &self,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<serde_json::Value, CommError> {
        let path = format!(
            "/api/v1/skills/top{}",
            build_query(None, None, page, page_size)
        );
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Get reviews for a skill/product.
    pub async fn get_skill_reviews(
        &self,
        id: &str,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<ReviewsResponse, CommError> {
        let path = format!(
            "/api/v1/skills/{}/reviews{}",
            id,
            build_query(None, None, page, page_size)
        );
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Submit a review for a skill/product.
    pub async fn submit_skill_review(
        &self,
        id: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::POST,
            &format!("/api/v1/skills/{}/reviews", id),
            Some(body),
        )
        .await
    }

    /// Get media (screenshots, videos) for a skill/product.
    pub async fn get_skill_media(&self, id: &str) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/skills/{}/media", id),
            None::<&()>,
        )
        .await
    }

    /// Get feedback for a skill/product.
    pub async fn get_skill_feedback(
        &self,
        id: &str,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<serde_json::Value, CommError> {
        let path = format!(
            "/api/v1/skills/{}/feedback{}",
            id,
            build_query(None, None, page, page_size)
        );
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Submit feedback for a skill/product.
    pub async fn submit_skill_feedback(
        &self,
        id: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::POST,
            &format!("/api/v1/skills/{}/feedback", id),
            Some(body),
        )
        .await
    }

    /// Get similar products for an app/product.
    pub async fn get_similar_apps(&self, id: &str) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/apps/{}/similar", id),
            None::<&()>,
        )
        .await
    }

    /// Get featured apps/products.
    pub async fn get_featured(
        &self,
        artifact_type: Option<&str>,
    ) -> Result<serde_json::Value, CommError> {
        let mut path = "/api/v1/apps/featured".to_string();
        if let Some(t) = artifact_type {
            if !t.is_empty() {
                path.push_str(&format!("?type={}", urlencoding::encode(t)));
            }
        }
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// List marketplace categories with counts.
    pub async fn list_categories(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            "/api/v1/marketplace/categories",
            None::<&()>,
        )
        .await
    }

    /// Get screenshots for a product type.
    pub async fn get_screenshots(
        &self,
        screenshot_type: &str,
    ) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/screenshots/{}", screenshot_type),
            None::<&()>,
        )
        .await
    }

    // ── Universal Code Redemption ────────────────────────────────────

    /// Redeem any marketplace code (SKIL-*, WORK-*, AGNT-*, PLUG-*) via the universal endpoint.
    /// Includes platform so the server returns a resolved, platform-specific download URL.
    pub async fn redeem_code(&self, code: &str) -> Result<CodeRedeemResponse, CommError> {
        let body = serde_json::json!({
            "code": code,
            "botIds": [self.bot_id],
            "platform": napp::plugin::current_platform_key(),
        });
        self.do_json(reqwest::Method::POST, "/api/v1/codes/redeem", Some(&body))
            .await
    }

    /// Install a skill for this bot via universal code redemption.
    pub async fn install_skill(&self, code: &str) -> Result<CodeRedeemResponse, CommError> {
        self.redeem_code(code).await
    }

    /// Install a product (skill/agent/workflow) for this bot by product ID.
    /// NeboAI may return JSON or an empty body on success.
    pub async fn install_product(&self, id: &str) -> Result<serde_json::Value, CommError> {
        let body = serde_json::json!({ "botId": self.bot_id });
        let url = format!("{}/api/v1/products/{}/install", self.api_server, id);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(self.token())
            .json(&body)
            .send()
            .await
            .map_err(|e| CommError::Other(format!("request failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!(
                "NeboAI returned {}: {}",
                status, text
            )));
        }

        let text = resp.text().await.unwrap_or_default();
        if text.is_empty() {
            Ok(serde_json::json!({ "success": true }))
        } else {
            Ok(serde_json::from_str(&text)
                .unwrap_or_else(|_| serde_json::json!({ "success": true })))
        }
    }

    /// Uninstall a product for this bot by product ID.
    pub async fn uninstall_product(&self, id: &str) -> Result<(), CommError> {
        let body = serde_json::json!({ "botId": self.bot_id });
        self.do_void(
            reqwest::Method::DELETE,
            &format!("/api/v1/products/{}/install", id),
            Some(&body),
        )
        .await
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
        // The plugin CDN (EdgeLB) intermittently drops connections — a single send
        // failure shouldn't kill an install when a retry succeeds in ~1s. Retry with a
        // short backoff, but ONLY for transient failures (send errors, 5xx). A 4xx like
        // 404 "binary not found" is permanent — retrying it just wastes time.
        let mut last_err: Option<CommError> = None;
        for attempt in 1..=3u32 {
            match self.resolve_and_fetch_napp(&full_url).await {
                Ok(bytes) => return Ok(bytes),
                Err(e) => {
                    let permanent = e.to_string().contains("returned 4");
                    tracing::warn!(url = %full_url, attempt, permanent, error = %e, "napp download attempt failed");
                    last_err = Some(e);
                    if permanent || attempt >= 3 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64))
                        .await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| CommError::Other("download failed".into())))
    }

    /// Resolve one download attempt to `.napp` bytes.
    ///
    /// The API download endpoint now returns `{ "downloadUrl": ... }` JSON (a CDN
    /// link) instead of streaming the package; older servers and direct CDN links
    /// return the `.napp` bytes directly. This tolerates both during the cutover.
    ///
    /// Auth: the bearer is attached only when calling our own API host. The CDN
    /// blob is fetched WITHOUT it — the token must never cross to another host.
    /// (That is also why the server returns a 200 JSON URL rather than a 302
    /// redirect, which reqwest would follow with the Authorization header attached.)
    async fn resolve_and_fetch_napp(&self, full_url: &str) -> Result<Vec<u8>, CommError> {
        let same_origin = full_url.starts_with(self.api_server.as_str());
        let mut req = self.client.get(full_url);
        if same_origin {
            req = req.bearer_auth(self.token());
        }
        let resp = req
            .send()
            .await
            .map_err(|e| CommError::Other(format!("fetch failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!(
                "NeboAI returned {}: {}",
                status, body
            )));
        }

        let is_json = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("application/json"))
            .unwrap_or(false);

        if is_json {
            #[derive(serde::Deserialize)]
            struct DownloadUrlResponse {
                #[serde(rename = "downloadUrl")]
                download_url: String,
            }
            let parsed: DownloadUrlResponse = resp
                .json()
                .await
                .map_err(|e| CommError::Other(format!("parse download url: {}", e)))?;
            return self.fetch_no_auth(&parsed.download_url).await;
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| CommError::Other(format!("read body: {}", e)))
    }

    /// GET raw bytes with no Authorization header — for cross-host CDN blob URLs.
    async fn fetch_no_auth(&self, url: &str) -> Result<Vec<u8>, CommError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| CommError::Other(format!("cdn fetch failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!(
                "CDN returned {}: {}",
                status, body
            )));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| CommError::Other(format!("read cdn body: {}", e)))
    }

    /// Uninstall a skill for this bot.
    pub async fn uninstall_skill(&self, id: &str) -> Result<(), CommError> {
        self.do_void(
            reqwest::Method::DELETE,
            &format!("/api/v1/skills/{}/install/{}", id, self.bot_id),
            None::<&()>,
        )
        .await
    }

    // ── Workflows ───────────────────────────────────────────────────

    /// Install a workflow for this bot via universal code redemption.
    pub async fn install_workflow(&self, code: &str) -> Result<CodeRedeemResponse, CommError> {
        self.redeem_code(code).await
    }

    /// Uninstall a workflow for this bot.
    pub async fn uninstall_workflow(&self, id: &str) -> Result<(), CommError> {
        self.do_void(
            reqwest::Method::DELETE,
            &format!("/api/v1/workflows/{}/install/{}", id, self.bot_id),
            None::<&()>,
        )
        .await
    }

    /// List workflows from NeboAI catalog.
    pub async fn list_workflows(
        &self,
        query: Option<&str>,
        category: Option<&str>,
        page: Option<i64>,
        page_size: Option<i64>,
    ) -> Result<WorkflowsResponse, CommError> {
        let path = format!(
            "/api/v1/workflows{}",
            build_query(query, category, page, page_size)
        );
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    // ── Agents (marketplace) ────────────────────────────────────────

    /// Install an agent for this bot via universal code redemption.
    pub async fn install_agent(&self, code: &str) -> Result<CodeRedeemResponse, CommError> {
        self.redeem_code(code).await
    }

    /// Get agent detail from NeboAI (persona, workflows, download URL).
    pub async fn get_agent(&self, slug: &str) -> Result<AgentDetailResponse, CommError> {
        let path = format!("/api/v1/agents/{}", urlencoding::encode(slug));
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Uninstall an agent for this bot.
    pub async fn uninstall_agent(&self, id: &str) -> Result<(), CommError> {
        self.do_void(
            reqwest::Method::DELETE,
            &format!("/api/v1/agents/{}/install/{}", id, self.bot_id),
            None::<&()>,
        )
        .await
    }

    // ── Publishing ────────────────────────────────────────────────

    /// Create or update a skill artifact on NeboAI.
    pub async fn publish_skill(
        &self,
        name: &str,
        description: &str,
        manifest_content: &str,
        version: &str,
        visibility: &str,
    ) -> Result<serde_json::Value, CommError> {
        let body = serde_json::json!({
            "name": name,
            "description": description,
            "type": "skill",
            "manifestContent": manifest_content,
            "version": version,
            "visibility": visibility,
        });
        self.do_json(reqwest::Method::POST, "/api/v1/skills", Some(&body))
            .await
    }

    /// Create or update an agent artifact on NeboAI.
    pub async fn publish_agent(
        &self,
        name: &str,
        description: &str,
        manifest_content: &str,
        version: &str,
        visibility: &str,
        agent_json: Option<&str>,
    ) -> Result<serde_json::Value, CommError> {
        let mut body = serde_json::json!({
            "name": name,
            "description": description,
            "type": "agent",
            "manifestContent": manifest_content,
            "version": version,
            "visibility": visibility,
        });
        if let Some(aj) = agent_json {
            body["typeConfig"] = serde_json::from_str(aj).unwrap_or(serde_json::json!({}));
        }
        self.do_json(reqwest::Method::POST, "/api/v1/skills", Some(&body))
            .await
    }

    /// Submit an artifact for marketplace review.
    pub async fn submit_for_review(
        &self,
        artifact_id: &str,
        version: &str,
    ) -> Result<serde_json::Value, CommError> {
        let body = serde_json::json!({ "version": version });
        self.do_json(
            reqwest::Method::POST,
            &format!("/api/v1/skills/{}/submit", artifact_id),
            Some(&body),
        )
        .await
    }

    // ── Bot Identity ────────────────────────────────────────────────

    /// Push bot name and agent info to NeboAI.
    pub async fn update_bot_identity(&self, name: &str, role: &str) -> Result<(), CommError> {
        let body = UpdateBotIdentityRequest {
            name: name.into(),
            role: role.into(),
        };
        self.do_void(
            reqwest::Method::PUT,
            &format!("/api/v1/bots/{}", self.bot_id),
            Some(&body),
        )
        .await
    }

    // ── Loops ───────────────────────────────────────────────────────

    /// Join the bot to a loop using an invite code.
    pub async fn join_loop(&self, code: &str) -> Result<JoinLoopResponse, CommError> {
        let body = JoinLoopRequest {
            code: code.into(),
            bot_id: self.bot_id.clone(),
        };
        self.do_json(reqwest::Method::POST, "/api/v1/loops/join", Some(&body))
            .await
    }

    /// List all loops this bot belongs to.
    pub async fn list_bot_loops(&self) -> Result<Vec<crate::api_types::Loop>, CommError> {
        let resp: LoopsResponse = self
            .do_json(
                reqwest::Method::GET,
                &format!("/api/v1/bots/{}/loops", self.bot_id),
                None::<&()>,
            )
            .await?;
        Ok(resp.loops)
    }

    /// Get a single loop by ID.
    pub async fn get_loop(&self, loop_id: &str) -> Result<crate::api_types::Loop, CommError> {
        self.do_json(
            reqwest::Method::GET,
            &format!("/api/v1/bots/{}/loops/{}", self.bot_id, loop_id),
            None::<&()>,
        )
        .await
    }

    /// List members of a loop with online presence.
    pub async fn list_loop_members(&self, loop_id: &str) -> Result<Vec<LoopMember>, CommError> {
        let resp: LoopMembersResponse = self
            .do_json(
                reqwest::Method::GET,
                &format!("/api/v1/bots/{}/loops/{}/members", self.bot_id, loop_id),
                None::<&()>,
            )
            .await?;
        Ok(resp.members)
    }

    // ── Agents ──────────────────────────────────────────────────────

    /// Register an agent in a loop. The gateway auto-creates
    /// an agent space conversation and subscribes the bot to it.
    pub async fn register_agent(
        &self,
        loop_id: &str,
        agent_name: &str,
        agent_slug: &str,
        description: Option<&str>,
    ) -> Result<serde_json::Value, CommError> {
        let body = AgentActivateRequest {
            bot_id: self.bot_id.clone(),
            agent_name: agent_name.to_string(),
            agent_slug: agent_slug.to_string(),
            description: description.map(|s| s.to_string()),
        };
        tracing::info!(
            target: "neboai_identity",
            bot_id = %self.bot_id,
            loop_id = %loop_id,
            sending_name = %agent_name,
            sending_slug = %agent_slug,
            "register_agent: REQUEST to loop"
        );
        let resp = self
            .do_json::<serde_json::Value>(
                reqwest::Method::POST,
                &format!("/api/v1/loops/{}/agents", loop_id),
                Some(&body),
            )
            .await;
        match &resp {
            Ok(v) => tracing::info!(
                target: "neboai_identity",
                sent_slug = %agent_slug,
                returned_id = ?v.get("id").and_then(|x| x.as_str()),
                returned_name = ?v.get("name").and_then(|x| x.as_str()),
                returned_slug = ?v.get("slug").and_then(|x| x.as_str()),
                returned_conv = ?v.get("conversationId").and_then(|x| x.as_str()),
                "register_agent: RESPONSE from loop"
            ),
            Err(e) => tracing::warn!(target: "neboai_identity", sent_slug = %agent_slug, error = %e, "register_agent: FAILED"),
        }
        resp
    }

    /// Publish this bot's desktop chat list for one loop agent (additive
    /// upsert server-side). Each chat becomes its own loop conversation —
    /// the remote emulates the local Threads tab.
    pub async fn sync_agent_chats(
        &self,
        loop_agent_id: &str,
        chats: &[AgentChatSync],
    ) -> Result<Vec<AgentChatSyncResult>, CommError> {
        #[derive(serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Req<'a> {
            chats: &'a [AgentChatSync],
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            #[serde(default)]
            chats: Vec<AgentChatSyncResult>,
        }
        let resp: Resp = self
            .do_json(
                reqwest::Method::PUT,
                &format!("/api/v1/agents/{}/chats/sync", loop_agent_id),
                Some(&Req { chats }),
            )
            .await?;
        Ok(resp.chats)
    }

    /// List agents registered by this bot in a loop.
    pub async fn list_agents(&self, loop_id: &str) -> Result<Vec<AgentInfo>, CommError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            agents: Vec<AgentInfo>,
        }
        let resp: Resp = self
            .do_json(
                reqwest::Method::GET,
                &format!("/api/v1/loops/{}/agents", loop_id),
                None::<&()>,
            )
            .await?;
        // Filter to only this bot's agents
        let mine: Vec<AgentInfo> = resp
            .agents
            .into_iter()
            .filter(|a| a.bot_id == self.bot_id)
            .collect();
        Ok(mine)
    }

    /// Check whether a handle (the `bot_<chosen>` routing identity) is globally
    /// available across all agents. This bot is excluded so its own current
    /// handle is never reported as taken. Returns `true` when available.
    pub async fn handle_available(&self, handle: &str) -> Result<bool, CommError> {
        #[derive(serde::Deserialize)]
        struct Resp {
            available: bool,
        }
        let path = format!(
            "/api/v1/agents/handle-available?handle={}&botId={}",
            urlencoding::encode(handle),
            urlencoding::encode(&self.bot_id),
        );
        let resp: Resp = self
            .do_json(reqwest::Method::GET, &path, None::<&()>)
            .await?;
        Ok(resp.available)
    }

    /// Deregister an agent from a loop.
    pub async fn deregister_agent(
        &self,
        loop_id: &str,
        agent_id: &str,
    ) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::DELETE,
            &format!("/api/v1/loops/{}/agents/{}", loop_id, agent_id),
            None::<&()>,
        )
        .await
    }

    // ── Channels ────────────────────────────────────────────────────

    /// List all channels this bot belongs to across all loops.
    pub async fn list_bot_channels(&self) -> Result<Vec<LoopChannel>, CommError> {
        let resp: LoopChannelsResponse = self
            .do_json(
                reqwest::Method::GET,
                &format!("/api/v1/bots/{}/channels", self.bot_id),
                None::<&()>,
            )
            .await?;
        Ok(resp.channels)
    }

    /// Create a channel in a loop. NeboLoop sanitizes the name and auto-adds the
    /// bot's agents as members, so the bot can post immediately after.
    pub async fn create_channel(
        &self,
        loop_id: &str,
        name: &str,
        description: Option<&str>,
    ) -> Result<(), CommError> {
        #[derive(serde::Serialize)]
        struct CreateChannelBody<'a> {
            name: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            description: Option<&'a str>,
        }
        self.do_json::<serde_json::Value>(
            reqwest::Method::POST,
            &format!("/api/v1/loops/{}/channels", loop_id),
            Some(&CreateChannelBody { name, description }),
        )
        .await?;
        Ok(())
    }

    /// List members of a channel.
    pub async fn list_channel_members(
        &self,
        channel_id: &str,
    ) -> Result<Vec<ChannelMember>, CommError> {
        let resp: ChannelMembersResponse = self
            .do_json(
                reqwest::Method::GET,
                &format!(
                    "/api/v1/bots/{}/channels/{}/members",
                    self.bot_id, channel_id
                ),
                None::<&()>,
            )
            .await?;
        Ok(resp.members)
    }

    /// Fetch recent messages from a channel (normalized).
    pub async fn list_channel_messages(
        &self,
        channel_id: &str,
        limit: Option<i64>,
    ) -> Result<Vec<NormalizedChannelMessage>, CommError> {
        let mut path = format!(
            "/api/v1/bots/{}/channels/{}/messages",
            self.bot_id, channel_id
        );
        if let Some(l) = limit {
            path.push_str(&format!("?limit={}", l));
        }
        let resp: ChannelMessagesResponse = self
            .do_json(reqwest::Method::GET, &path, None::<&()>)
            .await?;
        Ok(resp.normalize())
    }

    // ── Referral ──────────────────────────────────────────────────

    /// Get or create the user's referral/invite code.
    pub async fn referral_code(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            "/api/v1/owners/me/referral-code",
            None::<&()>,
        )
        .await
    }

    // ── Billing ────────────────────────────────────────────────────

    /// List billing prices/plans.
    pub async fn billing_prices(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(reqwest::Method::GET, "/api/v1/billing/prices", None::<&()>)
            .await
    }

    /// Get current subscription status.
    pub async fn billing_subscription(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            "/api/v1/billing/subscription",
            None::<&()>,
        )
        .await
    }

    /// Create a Stripe checkout session for a given price.
    pub async fn billing_checkout(&self, price_id: &str) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::POST,
            "/api/v1/billing/checkout",
            Some(&serde_json::json!({"priceId": price_id})),
        )
        .await
    }

    /// Create a Stripe checkout session with multiple prices (plan + boost).
    /// Pass `ui_mode` = "embedded" to get a clientSecret for embedded checkout instead of a redirect URL.
    pub async fn billing_checkout_multi(
        &self,
        price_ids: &[String],
        ui_mode: Option<&str>,
    ) -> Result<serde_json::Value, CommError> {
        let mut body = serde_json::json!({"priceIds": price_ids});
        if let Some(mode) = ui_mode {
            body["uiMode"] = serde_json::json!(mode);
        }
        self.do_json(
            reqwest::Method::POST,
            "/api/v1/billing/checkout",
            Some(&body),
        )
        .await
    }

    /// Create an inline subscription (returns clientSecret for PaymentElement).
    pub async fn billing_subscribe(
        &self,
        price_ids: &[String],
    ) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::POST,
            "/api/v1/billing/subscribe",
            Some(&serde_json::json!({"priceIds": price_ids})),
        )
        .await
    }

    /// Create a Stripe customer portal session.
    pub async fn billing_portal(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(reqwest::Method::POST, "/api/v1/billing/portal", None::<&()>)
            .await
    }

    /// Create a Stripe SetupIntent for in-app payment method collection.
    pub async fn billing_setup_intent(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::POST,
            "/api/v1/billing/setup-intent",
            None::<&()>,
        )
        .await
    }

    /// Cancel a subscription.
    pub async fn billing_cancel(
        &self,
        subscription_id: &str,
    ) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::POST,
            "/api/v1/billing/cancel-subscription",
            Some(&serde_json::json!({"subscriptionId": subscription_id})),
        )
        .await
    }

    /// List invoices (owner-scoped).
    pub async fn billing_invoices(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            "/api/v1/owners/me/invoices",
            None::<&()>,
        )
        .await
    }

    /// List payment methods (owner-scoped).
    pub async fn billing_payment_methods(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            "/api/v1/owners/me/payment-methods",
            None::<&()>,
        )
        .await
    }

    // ── Marketplace Subscriptions ──────────────────────────────────

    /// Create a marketplace subscription (triggers Stripe Checkout).
    pub async fn marketplace_create_subscription(
        &self,
        target_id: &str,
        target_type: &str,
        bot_count: i32,
    ) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::POST,
            "/api/v1/marketplace/subscriptions",
            Some(&serde_json::json!({
                "targetId": target_id,
                "targetType": target_type,
                "botCount": bot_count,
            })),
        )
        .await
    }

    /// List active marketplace subscriptions for the current owner.
    pub async fn marketplace_list_subscriptions(
        &self,
    ) -> Result<serde_json::Value, CommError> {
        self.do_json(
            reqwest::Method::GET,
            "/api/v1/marketplace/subscriptions",
            None::<&()>,
        )
        .await
    }

    /// Cancel a marketplace subscription.
    pub async fn marketplace_cancel_subscription(
        &self,
        id: &str,
    ) -> Result<serde_json::Value, CommError> {
        let path = format!("/api/v1/marketplace/subscriptions/{}/cancel", id);
        self.do_json(reqwest::Method::POST, &path, None::<&()>)
            .await
    }

    /// List the owner's active entitlements ("restore purchases") — what this
    /// account owns and may re-fetch license keys for. Stable "what do I own"
    /// list, decoupled from subscription-management details.
    pub async fn entitlements(&self) -> Result<serde_json::Value, CommError> {
        self.do_json(reqwest::Method::GET, "/api/v1/entitlements", None::<&()>)
            .await
    }

    // ── Plugins ─────────────────────────────────────────────────────

    /// Get plugin manifest from NeboAI for a specific platform.
    ///
    /// Returns the full `PluginManifest` which includes per-platform binary entries.
    pub async fn get_plugin(
        &self,
        slug: &str,
        platform: &str,
    ) -> Result<napp::plugin::PluginManifest, CommError> {
        let path = format!(
            "/api/v1/plugins/{}?platform={}",
            urlencoding::encode(slug),
            urlencoding::encode(platform),
        );
        self.do_json(reqwest::Method::GET, &path, None::<&()>).await
    }

    /// Download a plugin binary from a URL.
    ///
    /// The URL can be absolute (CDN) or relative (API path).
    pub async fn download_plugin_binary(&self, url: &str) -> Result<Vec<u8>, CommError> {
        let full_url = if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            format!("{}{}", self.api_server, url)
        };
        self.fetch_raw(&full_url).await
    }

    // ── Content Protection ─────────────────────────────────────────

    /// Register this bot with NeboAI for content protection.
    ///
    /// Called on startup after authentication. Idempotent — re-registering
    /// the same bot_id updates last_seen and platform info.
    pub async fn register_bot(
        &self,
        platform: &str,
        app_version: &str,
    ) -> Result<serde_json::Value, CommError> {
        let body = serde_json::json!({
            "bot_id": self.bot_id,
            "platform": platform,
            "app_version": app_version,
        });
        self.do_json(reqwest::Method::POST, "/api/v1/bots", Some(&body))
            .await
    }

    /// Fetch license keys for sealed .napp artifacts.
    ///
    /// Returns decryption keys for all artifacts this bot is licensed to access.
    /// Keys are base64-encoded 32-byte AES-256-GCM keys with a TTL.
    pub async fn fetch_license_keys(
        &self,
        artifact_ids: &[String],
    ) -> Result<LicenseKeysResponse, CommError> {
        let body = serde_json::json!({
            "bot_id": self.bot_id,
            "artifact_ids": artifact_ids,
        });
        self.do_json(reqwest::Method::POST, "/api/v1/licenses/keys", Some(&body))
            .await
    }

    // ── File Upload / Download ────────────────────────────────────────

    /// Upload a file to NeboAI for attachment to a message.
    /// Returns attachment metadata including the file_id and download URL.
    pub async fn upload_file(
        &self,
        filename: &str,
        mime_type: &str,
        data: Vec<u8>,
    ) -> Result<crate::wire::Attachment, CommError> {
        let part = reqwest::multipart::Part::bytes(data)
            .file_name(filename.to_string())
            .mime_str(mime_type)
            .map_err(|e| CommError::Other(format!("invalid mime type: {}", e)))?;

        let form = reqwest::multipart::Form::new().part("file", part);

        let url = format!("{}/api/v1/files/upload", self.api_server);
        debug!(url = %url, filename = %filename, "uploading file");

        // Use a client with a longer timeout for uploads
        let upload_client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| Client::new());

        let resp = upload_client
            .post(&url)
            .bearer_auth(self.token())
            .multipart(form)
            .send()
            .await
            .map_err(|e| CommError::Other(format!("upload failed: {}", e)))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(CommError::Other(format!(
                "upload returned {}: {}",
                status, body
            )));
        }

        resp.json::<crate::wire::Attachment>()
            .await
            .map_err(|e| CommError::Other(format!("decode upload response: {}", e)))
    }

    /// Download a file by ID. Returns raw bytes.
    pub async fn download_file(&self, file_id: &str) -> Result<Vec<u8>, CommError> {
        let url = format!("{}/api/v1/files/{}", self.api_server, file_id);
        self.fetch_raw(&url).await
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
            return Err(CommError::Other(format!(
                "NeboAI returned {}: {}",
                status, body
            )));
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
        return Err(CommError::Other(format!(
            "NeboAI returned {}: {}",
            status, body
        )));
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
    // NeboAI paginates with `limit`/`offset` (not page/pageSize). Translate so the
    // client can actually page the full catalog instead of always getting page 1.
    let limit = page_size.filter(|&ps| ps > 0);
    if let Some(ps) = limit {
        params.push(format!("limit={}", ps));
    }
    if let (Some(p), Some(ps)) = (page, limit) {
        if p > 1 {
            params.push(format!("offset={}", (p - 1) * ps));
        }
    }
    if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    }
}

/// One chat in the chats/sync RESPONSE: `created` marks a conversation the
/// server just made — the bridge backfills its desktop history exactly once.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatSyncResult {
    pub chat_id: String,
    pub conversation_id: String,
    #[serde(default)]
    pub created: bool,
    /// Conversation head seq — 0 means the conversation is still empty
    /// (synced before backfill existed) and needs backfilling too.
    #[serde(default)]
    pub head_seq: u64,
    /// Tombstoned on the loop — delete the local copy, never backfill.
    #[serde(default)]
    pub deleted: bool,
}

/// One desktop chat in a chats/sync publish.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentChatSync {
    pub chat_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
}
