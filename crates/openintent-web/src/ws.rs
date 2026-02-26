//! WebSocket handler for streaming chat.
//!
//! Clients connect to `/ws` and exchange JSON messages.  Inbound messages
//! carry user chat input with a session_id; outbound messages stream the
//! agent's reasoning, tool invocations, and final text response.
//! Messages are persisted to the session store.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use openintent_adapters::Adapter;
use openintent_agent::runtime::ToolAdapter;
use openintent_agent::{
    AgentConfig, ChatRequest, LlmResponse, ToolDefinition, compact_messages, needs_compaction,
};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Adapter bridge: openintent_adapters::Adapter -> openintent_agent::ToolAdapter
// ---------------------------------------------------------------------------

/// Bridges the [`openintent_adapters::Adapter`] trait to the
/// [`openintent_agent::runtime::ToolAdapter`] trait so that adapters can be
/// used directly in the agent's ReAct loop.
pub struct AdapterBridge(pub Arc<dyn Adapter>);

#[async_trait]
impl ToolAdapter for AdapterBridge {
    fn adapter_id(&self) -> &str {
        self.0.id()
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.0
            .tools()
            .into_iter()
            .map(|t| ToolDefinition {
                name: t.name,
                description: t.description,
                input_schema: t.parameters,
            })
            .collect()
    }

    async fn execute(&self, tool_name: &str, arguments: Value) -> openintent_agent::Result<String> {
        let result = self
            .0
            .execute_tool(tool_name, arguments)
            .await
            .map_err(|e| openintent_agent::AgentError::ToolExecutionFailed {
                tool_name: tool_name.to_owned(),
                reason: e.to_string(),
            })?;
        Ok(serde_json::to_string(&result)?)
    }
}

// ---------------------------------------------------------------------------
// WebSocket message types
// ---------------------------------------------------------------------------

/// Inbound message from the client.
#[derive(Deserialize)]
struct InboundMessage {
    /// Message type.  Currently only `"chat"` is supported.
    #[serde(rename = "type")]
    msg_type: String,
    /// The user's message content.
    content: String,
    /// Session ID for this conversation.
    session_id: Option<String>,
}

/// Outbound message sent to the client.
#[derive(Serialize)]
struct OutboundMessage {
    /// Message type: `"text"`, `"tool_start"`, `"tool_end"`, `"error"`, or
    /// `"done"`.
    #[serde(rename = "type")]
    msg_type: String,

    /// Payload -- interpretation depends on `msg_type`.
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,

    /// Tool name (present for `tool_start`).
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<String>,

    /// Tool execution result (present for `tool_end`).
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
}

impl OutboundMessage {
    fn text(content: impl Into<String>) -> Self {
        Self {
            msg_type: "text".into(),
            content: Some(content.into()),
            tool: None,
            result: None,
        }
    }

    fn tool_start(name: impl Into<String>) -> Self {
        Self {
            msg_type: "tool_start".into(),
            content: None,
            tool: Some(name.into()),
            result: None,
        }
    }

    fn tool_end(result: impl Into<String>) -> Self {
        Self {
            msg_type: "tool_end".into(),
            content: None,
            tool: None,
            result: Some(result.into()),
        }
    }

    fn done() -> Self {
        Self {
            msg_type: "done".into(),
            content: None,
            tool: None,
            result: None,
        }
    }

    fn error(msg: impl Into<String>) -> Self {
        Self {
            msg_type: "error".into(),
            content: Some(msg.into()),
            tool: None,
            result: None,
        }
    }

    fn text_delta(content: impl Into<String>) -> Self {
        Self {
            msg_type: "text_delta".into(),
            content: Some(content.into()),
            tool: None,
            result: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Axum handler that upgrades the HTTP connection to a WebSocket.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

/// Process a single WebSocket connection.
async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    tracing::info!("WebSocket client connected");

    while let Some(Ok(msg)) = socket.recv().await {
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };

        let inbound: InboundMessage = match serde_json::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                let _ = send(&mut socket, &OutboundMessage::error(e.to_string())).await;
                continue;
            }
        };

        if inbound.msg_type != "chat" {
            let _ = send(
                &mut socket,
                &OutboundMessage::error(format!("unknown message type: {}", inbound.msg_type)),
            )
            .await;
            continue;
        }

        let session_id = inbound.session_id.clone();

        if let Err(e) =
            handle_chat_message(&mut socket, &state, session_id.as_deref(), &inbound.content).await
        {
            let _ = send(&mut socket, &OutboundMessage::error(e.to_string())).await;
        }

        let _ = send(&mut socket, &OutboundMessage::done()).await;
    }

    tracing::info!("WebSocket client disconnected");
}

/// Run the agent ReAct loop for a single user message, streaming results
/// back over the WebSocket and persisting messages to the session store.
async fn handle_chat_message(
    socket: &mut WebSocket,
    state: &Arc<AppState>,
    session_id: Option<&str>,
    user_message: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let llm = &state.llm;
    let adapters = &state.adapters;
    let sessions = &state.sessions;

    // Persist user message to session if session_id is provided.
    if let Some(sid) = session_id {
        let _ = sessions
            .append_message(sid, "user", user_message, None, None)
            .await;
    }

    // Bridge adapters.
    let tool_adapters: Vec<Arc<dyn ToolAdapter>> = adapters
        .iter()
        .map(|a| Arc::new(AdapterBridge(Arc::clone(a))) as Arc<dyn ToolAdapter>)
        .collect();

    let tools: Vec<ToolDefinition> = tool_adapters
        .iter()
        .flat_map(|a| a.tool_definitions())
        .collect();

    // Load system prompt (hot-reloadable).
    let system_prompt = state.system_prompt.read().await.clone();
    let mut messages = vec![openintent_agent::Message::system(&system_prompt)];

    // If we have a session, load recent history for context.
    if let Some(sid) = session_id
        && let Ok(history) = sessions.get_messages(sid, Some(20)).await
    {
        for msg in &history {
            match msg.role.as_str() {
                "user" => messages.push(openintent_agent::Message::user(&msg.content)),
                "assistant" => messages.push(openintent_agent::Message::assistant(&msg.content)),
                _ => {}
            }
        }
    }

    // Add current user message (if not already loaded from history).
    if session_id.is_none() {
        messages.push(openintent_agent::Message::user(user_message));
    }

    let config = AgentConfig::default();
    let max_turns = config.max_turns;
    let compaction_config = config.compaction.clone();

    for _turn in 0..max_turns {
        // Check if context compaction is needed before the LLM call.
        if needs_compaction(&messages, &compaction_config) {
            tracing::info!(
                message_count = messages.len(),
                "WebSocket handler: context compaction triggered"
            );
            match compact_messages(&messages, llm, &compaction_config).await {
                Ok(compacted) => {
                    tracing::info!(
                        original = messages.len(),
                        compacted = compacted.len(),
                        "WebSocket handler: context compaction succeeded"
                    );
                    messages = compacted;
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "WebSocket handler: context compaction failed, continuing with uncompacted messages"
                    );
                }
            }
        }

        let request = ChatRequest {
            model: config.model.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            stream: true,
        };

        // Use streaming callback to send text deltas over WebSocket in real-time.
        // A channel bridges the sync callback to the async WebSocket sender.
        let (delta_tx, mut delta_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        let llm_clone = llm.clone();
        let request_clone = request.clone();
        let stream_handle = tokio::spawn(async move {
            llm_clone
                .stream_chat_with_callback(&request_clone, |delta| {
                    let _ = delta_tx.send(delta.to_owned());
                })
                .await
        });

        // Forward deltas to the WebSocket client as they arrive.
        while let Some(delta) = delta_rx.recv().await {
            let _ = send(socket, &OutboundMessage::text_delta(&delta)).await;
        }

        // Await the completed response (destructure to discard usage at this layer).
        let (response, _usage) = stream_handle.await.map_err(|e| {
            Box::new(std::io::Error::other(format!(
                "LLM stream task panicked: {e}"
            ))) as Box<dyn std::error::Error + Send + Sync>
        })??;

        match response {
            LlmResponse::Text(text) => {
                messages.push(openintent_agent::Message::assistant(&text));
                send(socket, &OutboundMessage::text(&text)).await?;

                // Evolution: analyze response for signs of inability.
                if let Some(ref evo) = state.evolution {
                    let mut evo = evo.lock().await;
                    if let Some(issue_url) = evo
                        .analyze_response(user_message, &text, "web", _turn + 1)
                        .await
                    {
                        let evo_msg = format!("A feature request has been auto-filed: {issue_url}");
                        send(socket, &OutboundMessage::text(&evo_msg)).await?;
                    }
                }

                // Persist assistant response.
                if let Some(sid) = session_id {
                    let _ = sessions
                        .append_message(sid, "assistant", &text, None, None)
                        .await;
                }
                return Ok(());
            }
            LlmResponse::ToolCalls(calls) => {
                messages.push(openintent_agent::Message::assistant_tool_calls(
                    calls.clone(),
                ));

                for call in &calls {
                    send(socket, &OutboundMessage::tool_start(&call.name)).await?;

                    let adapter = tool_adapters
                        .iter()
                        .find(|a| a.tool_definitions().iter().any(|td| td.name == call.name));

                    let result_str = match adapter {
                        Some(a) => match a.execute(&call.name, call.arguments.clone()).await {
                            Ok(r) => r,
                            Err(e) => format!("Error: {e}"),
                        },
                        None => format!("Error: unknown tool `{}`", call.name),
                    };

                    send(socket, &OutboundMessage::tool_end(&result_str)).await?;
                    messages.push(openintent_agent::Message::tool_result(
                        &call.id,
                        &result_str,
                    ));
                }
            }
        }
    }

    // Evolution: report max-turns exceeded as unhandled intent.
    if let Some(ref evo) = state.evolution {
        let mut evo = evo.lock().await;
        let error = openintent_agent::AgentError::MaxTurnsExceeded {
            task_id: uuid::Uuid::nil(),
            max_turns: config.max_turns,
        };
        if let Some(issue_url) = evo.report_error(user_message, "web", &error).await {
            send(
                socket,
                &OutboundMessage::text(format!(
                    "Reached maximum reasoning turns. A feature request has been auto-filed: {issue_url}"
                )),
            )
            .await?;
        } else {
            send(
                socket,
                &OutboundMessage::text("Reached maximum number of reasoning turns."),
            )
            .await?;
        }
    } else {
        send(
            socket,
            &OutboundMessage::text("Reached maximum number of reasoning turns."),
        )
        .await?;
    }
    Ok(())
}

/// Serialize and send a JSON message over the WebSocket.
async fn send(
    socket: &mut WebSocket,
    msg: &OutboundMessage,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_string(msg)?;
    socket.send(Message::Text(json.into())).await?;
    Ok(())
}
