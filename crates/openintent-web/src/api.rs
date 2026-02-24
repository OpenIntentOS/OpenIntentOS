//! REST API route handlers.
//!
//! Provides endpoints for system status, adapter discovery, session management,
//! and one-shot (non-streaming) chat.

use std::sync::Arc;

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
}

/// Return basic system status information.
pub async fn status(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    Json(StatusResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        adapter_count: state.adapters.len(),
    })
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
/// return the final text response.
pub async fn chat(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ChatBody>,
) -> impl IntoResponse {
    let tool_adapters: Vec<Arc<dyn openintent_agent::ToolAdapter>> = state
        .adapters
        .iter()
        .map(|a| Arc::new(AdapterBridge(Arc::clone(a))) as Arc<dyn openintent_agent::ToolAdapter>)
        .collect();

    let config = AgentConfig::default();
    let mut ctx = AgentContext::new(Arc::clone(&state.llm), tool_adapters, config)
        .with_system_prompt(
            "You are OpenIntentOS, an AI assistant with access to system tools. \
             Be concise and helpful.",
        )
        .with_user_message(&body.message);

    match react_loop(&mut ctx).await {
        Ok(response) => (
            StatusCode::OK,
            Json(json!({
                "text": response.text,
                "turns_used": response.turns_used,
                "task_id": response.task_id.to_string(),
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": e.to_string(),
            })),
        ),
    }
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
