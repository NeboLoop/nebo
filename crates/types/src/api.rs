use serde::{Deserialize, Serialize};

// --- Auth types ---

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: i64,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct RefreshTokenRequest {
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
pub struct ForgotPasswordRequest {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ResetPasswordRequest {
    pub token: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyEmailRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct ResendVerificationRequest {
    pub email: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    #[serde(rename = "currentPassword")]
    pub current_password: String,
    #[serde(rename = "newPassword")]
    pub new_password: String,
}

// --- Chat types ---

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatStreamResponse {
    #[serde(rename = "type")]
    pub msg_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(rename = "toolCall", skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<serde_json::Value>,
}

// --- Common response types ---

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

/// The live counters of a turn running on a session, for a page that opens
/// the thread mid-run and must show "working" at once instead of waiting for
/// the next event. Filled by the runner, carried on the thread's history
/// response, read by the app: one shape for all three.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTurnStatus {
    pub elapsed_secs: u64,
    pub tool_calls: u32,
    pub current_tool: String,
}

/// The Dashboard: the whole workforce on one page. Composed by the server
/// from the run registry, the pending approvals, the workflow run history
/// and the chat history; the app renders it and refreshes it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardResponse {
    pub employees: Vec<DashboardEmployee>,
    pub counts: DashboardCounts,
    pub approvals: Vec<DashboardApproval>,
    pub recent_runs: Vec<DashboardRun>,
    pub runs_by_day: Vec<DashboardDay>,
    pub runs_by_employee: Vec<DashboardEmployeeRuns>,
}

/// One employee card. `status` is one of working, waiting, idle, paused.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardEmployee {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub status: String,
    /// What it is on right now, or what it last did.
    pub task: String,
    /// The live detail under the task: the tool in use and how long, the
    /// next scheduled run, or what is waiting on the owner.
    pub activity: String,
    pub last_activity_at: Option<i64>,
    pub chat_id: Option<String>,
    pub tool_calls: u32,
    pub elapsed_secs: u64,
    /// Memory-isolated employee: one thread per matter, opened as a list.
    pub isolated: bool,
    /// Thread count; for an isolated employee, its matters.
    pub matters: u32,
    /// The live workflow run when the employee is on one: what "Open run" opens.
    pub run_id: Option<String>,
    /// Where it is in that run, only when the workflow declares its steps.
    pub step: Option<u32>,
    pub step_count: Option<u32>,
    /// The last workflow run that ended: how it went, and when.
    pub last_outcome: Option<String>,
    pub last_detail: Option<String>,
    pub last_run_at: Option<i64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardCounts {
    pub employees: u32,
    pub working: u32,
    pub paused: u32,
    pub waiting: u32,
    pub runs_today: u32,
    pub done_today: u32,
    /// Runs whose evaluator found nothing to do.
    pub skipped_today: u32,
    pub stopped_today: u32,
    pub chat_turns_today: u32,
}

/// Something waiting on the owner: a gated tool call in a chat (`kind` =
/// "tool") or a workflow parked at an approval step (`kind` = "workflow").
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardApproval {
    pub id: String,
    pub kind: String,
    pub agent_id: String,
    pub agent_name: String,
    pub summary: String,
    pub since: i64,
    pub chat_id: Option<String>,
}

/// A run for the recent-runs table. `outcome` is one of working, done,
/// skipped (nothing to do), stopped, waiting.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardRun {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub title: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub outcome: String,
    pub detail: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardDay {
    /// Local calendar day, YYYY-MM-DD.
    pub day: String,
    pub done: u32,
    pub skipped: u32,
    pub stopped: u32,
    pub waiting: u32,
    pub chat_turns: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DashboardEmployeeRuns {
    pub agent_id: String,
    pub agent_name: String,
    pub runs: u32,
}
