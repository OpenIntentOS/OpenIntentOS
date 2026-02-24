//! REST API route handlers.
//!
//! Provides endpoints for system status, adapter discovery, and one-shot
//! (non-streaming) chat.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
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
    // Bridge each Adapter into a ToolAdapter for the agent runtime.
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
