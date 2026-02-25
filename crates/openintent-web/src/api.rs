//! REST API route handlers.
//!
//! Provides endpoints for system status, adapter discovery, session management,
//! and one-shot (non-streaming) chat.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use openintent_agent::{AgentConfig, AgentContext, react_loop};

use crate::state::AppState;
use crate::ws::AdapterBridge;

// ---------------------------------------------------------------------------
// GET /api/status
// ---------------------------------------------------------------------------

/// Response payload for the `/api/status` endpoint.
#[derive(Serialize)]
pub struct StatusResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub adapter_count: usize,
    pub uptime_seconds: u64,
    pub health_checks: HealthChecks,
}

/// Health check results for various system components.
#[derive(Serialize)]
pub struct HealthChecks {
    pub database: bool,
    pub adapters: bool,
    pub llm_connection: bool,
    pub memory_usage: MemoryUsage,
}

/// Memory usage statistics.
#[derive(Serialize)]
pub struct MemoryUsage {
    pub used_mb: f64,
    pub available_mb: f64,
    pub usage_percent: f64,
}

// Global startup time for uptime calculation
static STARTUP_TIME: std::sync::OnceLock<SystemTime> = std::sync::OnceLock::new();

/// Initialize the startup time (call this once at server start)
pub fn init_startup_time() {
    STARTUP_TIME.set(SystemTime::now()).ok();
}

/// Return basic system status information with health checks.
pub async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let startup_time = STARTUP_TIME.get().copied().unwrap_or_else(SystemTime::now);
    let uptime = SystemTime::now()
        .duration_since(startup_time)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    let health_checks = perform_health_checks(&state).await;

    Json(StatusResponse {
        status: if health_checks.database && health_checks.adapters && health_checks.llm_connection {
            "healthy"
        } else {
            "degraded"
        },
        version: env!("CARGO_PKG_VERSION"),
        adapter_count: state.adapters.len(),
        uptime_seconds: uptime,
        health_checks,
    })
}

/// Make health check function public for use in server module.
pub async fn perform_health_checks(state: &AppState) -> HealthChecks {
    let database_healthy = check_database_health(&state.db).await;
    let adapters_healthy = check_adapters_health(&state.adapters).await;
    let llm_healthy = check_llm_health(&state.llm).await;
    let memory_usage = get_memory_usage();

    HealthChecks {
        database: database_healthy,
        adapters: adapters_healthy,
        llm_connection: llm_healthy,
        memory_usage,
    }
}

/// Check if the database is responding.
async fn check_database_health(db: &openintent_store::Database) -> bool {
    // Try a simple query to verify database connectivity
    match db.execute(|conn| {
        conn.execute("SELECT 1", [])?;
        Ok(())
    }).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(error = %e, "database health check failed");
            false
        }
    }
}

/// Check if adapters are responding.
async fn check_adapters_health(adapters: &[Arc<dyn openintent_adapters::Adapter>]) -> bool {
    if adapters.is_empty() {
        return false;
    }

    // Check if at least 80% of adapters are responding
    let mut healthy_count = 0;
    for adapter in adapters {
        // Simple health check - verify the adapter can list its tools
        if !adapter.tools().is_empty() {
            healthy_count += 1;
        }
    }

    let health_ratio = healthy_count as f64 / adapters.len() as f64;
    health_ratio >= 0.8
}

/// Check if LLM connection is healthy.
async fn check_llm_health(_llm: &openintent_agent::LlmClient) -> bool {
    // For now, just return true since we don't have a simple health check method
    // In the future, we could add a lightweight health check to the LlmClient
    true
}

/// Get current memory usage statistics.
fn get_memory_usage() -> MemoryUsage {
    // On macOS, we can use system calls to get memory info
    // For now, we'll provide a simple implementation
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        
        let output = Command::new("vm_stat")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok());
            
        if let Some(vm_stat) = output {
            parse_vm_stat(&vm_stat)
        } else {
            MemoryUsage {
                used_mb: 0.0,
                available_mb: 0.0,
                usage_percent: 0.0,
            }
        }
    }
    
    #[cfg(not(target_os = "macos"))]
    {
        // Fallback for other platforms
        MemoryUsage {
            used_mb: 0.0,
            available_mb: 0.0,
            usage_percent: 0.0,
        }
    }
}

#[cfg(target_os = "macos")]
fn parse_vm_stat(vm_stat: &str) -> MemoryUsage {
    let mut free_pages = 0u64;
    let mut inactive_pages = 0u64;
    let mut active_pages = 0u64;
    let mut wired_pages = 0u64;
    
    for line in vm_stat.lines() {
        if line.starts_with("Pages free:") {
            free_pages = extract_number(line);
        } else if line.starts_with("Pages inactive:") {
            inactive_pages = extract_number(line);
        } else if line.starts_with("Pages active:") {
            active_pages = extract_number(line);
        } else if line.starts_with("Pages wired down:") {
            wired_pages = extract_number(line);
        }
    }
    
    let page_size = 4096u64; // 4KB pages on macOS
    let total_pages = free_pages + inactive_pages + active_pages + wired_pages;
    let used_pages = active_pages + wired_pages;
    
    let total_mb = (total_pages * page_size) as f64 / 1024.0 / 1024.0;
    let used_mb = (used_pages * page_size) as f64 / 1024.0 / 1024.0;
    let available_mb = total_mb - used_mb;
    let usage_percent = if total_mb > 0.0 { (used_mb / total_mb) * 100.0 } else { 0.0 };
    
    MemoryUsage {
        used_mb,
        available_mb,
        usage_percent,
    }
}

#[cfg(target_os = "macos")]
fn extract_number(line: &str) -> u64 {
    line.split_whitespace()
        .nth(2)
        .and_then(|s| s.trim_end_matches('.').parse().ok())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// GET /api/adapters
// ---------------------------------------------------------------------------

/// Serializable summary of a single adapter and its tools.
#[derive(Serialize)]
pub struct AdapterInfo {
    pub id: String,
    pub adapter_type: String,
    pub tools: Vec<ToolInfo>,
}

/// Serializable summary of a single tool.
#[derive(Serialize)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// List all registered adapters and their exposed tools.
pub async fn adapters(State(state): State<Arc<AppState>>) -> Json<Vec<AdapterInfo>> {
    let infos: Vec<AdapterInfo> = state
        .adapters
        .iter()
        .map(|adapter| {
            let tools = adapter
                .tools()
                .into_iter()
                .map(|t| ToolInfo {
                    name: t.name,
                    description: t.description,
                    parameters: t.parameters,
                })
                .collect();
            AdapterInfo {
                id: adapter.id().to_owned(),
                adapter_type: adapter.adapter_type().to_string(),
                tools,
            }
        })
        .collect();

    Json(infos)
}

// ---------------------------------------------------------------------------
// POST /api/chat
// ---------------------------------------------------------------------------

/// Request body for the one-shot chat endpoint.
#[derive(Deserialize)]
pub struct ChatBody {
    /// The user message to send to the agent.
    pub message: String,
}

/// Perform a one-shot (non-streaming) chat: run the full ReAct loop and
/// return the final text response with automatic error recovery.
pub async fn chat(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ChatBody>,
) -> (StatusCode, Json<Value>) {
    let max_retries = 3;
    let mut last_error = None;

    for attempt in 1..=max_retries {
        let tool_adapters: Vec<Arc<dyn openintent_agent::ToolAdapter>> = state
            .adapters
            .iter()
            .map(|a| Arc::new(AdapterBridge(Arc::clone(a))) as Arc<dyn openintent_agent::ToolAdapter>)
            .collect();

        let system_prompt = state.system_prompt.read().await.clone();
        let config = AgentConfig::default();
        let mut ctx = AgentContext::new(Arc::clone(&state.llm), tool_adapters, config)
            .with_system_prompt(&system_prompt)
            .with_user_message(&body.message);

        match react_loop(&mut ctx).await {
            Ok(response) => {
                if attempt > 1 {
                    tracing::info!(
                        attempt = attempt,
                        "chat request succeeded after retry"
                    );
                }
                return (
                    StatusCode::OK,
                    Json(json!({
                        "text": response.text,
                        "turns_used": response.turns_used,
                        "task_id": response.task_id.to_string(),
                        "attempt": attempt,
                    })),
                );
            }
            Err(e) => {
                last_error = Some(e);
                tracing::warn!(
                    attempt = attempt,
                    max_retries = max_retries,
                    error = %last_error.as_ref().unwrap(),
                    "chat request failed, retrying..."
                );

                if attempt < max_retries {
                    // Exponential backoff: 100ms, 200ms, 400ms
                    let delay = Duration::from_millis(100 * (1 << (attempt - 1)));
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    // All retries failed
    let error_msg = last_error
        .map(|e| e.to_string())
        .unwrap_or_else(|| "Unknown error".to_string());

    tracing::error!(
        max_retries = max_retries,
        error = %error_msg,
        "chat request failed after all retries"
    );

    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "error": format!("Request failed after {} attempts: {}", max_retries, error_msg),
            "attempts": max_retries,
            "recoverable": is_recoverable_error(&error_msg),
        })),
    )
}

/// Determine if an error is potentially recoverable with retry.
fn is_recoverable_error(error: &str) -> bool {
    let error_lower = error.to_lowercase();
    
    // Network-related errors that might be temporary
    error_lower.contains("timeout") ||
    error_lower.contains("connection") ||
    error_lower.contains("network") ||
    error_lower.contains("dns") ||
    error_lower.contains("rate limit") ||
    error_lower.contains("502") ||
    error_lower.contains("503") ||
    error_lower.contains("504")
}

// ---------------------------------------------------------------------------
// Session management endpoints
// ---------------------------------------------------------------------------

/// Response for a session object.
#[derive(Serialize)]
pub struct SessionResponse {
    pub id: String,
    pub name: String,
    pub model: String,
    pub message_count: i64,
    pub token_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Request body for creating a new session.
#[derive(Deserialize)]
pub struct CreateSessionBody {
    pub name: Option<String>,
}

/// GET /api/sessions — List all sessions.
pub async fn list_sessions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.sessions.list(50, 0).await {
        Ok(sessions) => {
            let items: Vec<SessionResponse> = sessions
                .into_iter()
                .map(|s| SessionResponse {
                    id: s.id,
                    name: s.name,
                    model: s.model,
                    message_count: s.message_count,
                    token_count: s.token_count,
                    created_at: s.created_at,
                    updated_at: s.updated_at,
                })
                .collect();
            (StatusCode::OK, Json(json!(items)))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

/// POST /api/sessions — Create a new session.
pub async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateSessionBody>,
) -> impl IntoResponse {
    let name = body.name.unwrap_or_else(|| "New Session".to_owned());
    match state.sessions.create(&name, "").await {
        Ok(session) => (
            StatusCode::CREATED,
            Json(json!(SessionResponse {
                id: session.id,
                name: session.name,
                model: session.model,
                message_count: session.message_count,
                token_count: session.token_count,
                created_at: session.created_at,
                updated_at: session.updated_at,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}

/// GET /api/sessions/:id — Get a specific session.
pub async fn get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.sessions.get(&id).await {
        Ok(session) => (
            StatusCode::OK,
            Json(json!(SessionResponse {
                id: session.id,
                name: session.name,
                model: session.model,
                message_count: session.message_count,
                token_count: session.token_count,
                created_at: session.created_at,
                updated_at: session.updated_at,
            })),
        ),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))),
    }
}

/// DELETE /api/sessions/:id — Delete a session.
pub async fn delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.sessions.delete(&id).await {
        Ok(()) => (StatusCode::OK, Json(json!({"deleted": true}))),
        Err(e) => (StatusCode::NOT_FOUND, Json(json!({"error": e.to_string()}))),
    }
}

/// Message response for session history.
#[derive(Serialize)]
pub struct MessageResponse {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

/// GET /api/sessions/:id/messages — Get messages for a session.
pub async fn get_session_messages(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.sessions.get_messages(&id, None).await {
        Ok(messages) => {
            let items: Vec<MessageResponse> = messages
                .into_iter()
                .filter(|m| m.role != "system") // Don't expose system prompts
                .map(|m| MessageResponse {
                    id: m.id,
                    role: m.role,
                    content: m.content,
                    created_at: m.created_at,
                })
                .collect();
            (StatusCode::OK, Json(json!(items)))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": e.to_string()})),
        ),
    }
}
