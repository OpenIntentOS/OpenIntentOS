//! WebSocket handler for streaming chat.
//!
//! Clients connect to `/ws` and exchange JSON messages.  Inbound messages
//! carry user chat input; outbound messages stream the agent's reasoning,
//! tool invocations, and final text response.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use openintent_adapters::Adapter;
use openintent_agent::runtime::ToolAdapter;
use openintent_agent::{AgentConfig, ChatRequest, LlmClient, LlmResponse, ToolDefinition};

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
        // Serialize the adapter's JSON output into a string for the LLM.
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
            // Ignore binary, ping, pong.
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

        if let Err(e) =
            handle_chat_message(&mut socket, &state.llm, &state.adapters, &inbound.content).await
        {
            let _ = send(&mut socket, &OutboundMessage::error(e.to_string())).await;
        }

        // Signal completion of this turn.
        let _ = send(&mut socket, &OutboundMessage::done()).await;
    }

    tracing::info!("WebSocket client disconnected");
}

/// Run the agent ReAct loop for a single user message, streaming results
/// back over the WebSocket.
async fn handle_chat_message(
    socket: &mut WebSocket,
    llm: &Arc<LlmClient>,
    adapters: &[Arc<dyn Adapter>],
    user_message: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Bridge adapters.
    let tool_adapters: Vec<Arc<dyn ToolAdapter>> = adapters
        .iter()
        .map(|a| Arc::new(AdapterBridge(Arc::clone(a))) as Arc<dyn ToolAdapter>)
        .collect();

    // Collect tool definitions.
    let tools: Vec<ToolDefinition> = tool_adapters
        .iter()
        .flat_map(|a| a.tool_definitions())
        .collect();

    let mut messages = vec![
        openintent_agent::Message::system(
            "You are OpenIntentOS, an AI assistant with access to system tools. \
             Be concise and helpful.",
        ),
        openintent_agent::Message::user(user_message),
    ];

    let config = AgentConfig::default();
    let max_turns = config.max_turns;

    for _turn in 0..max_turns {
        let request = ChatRequest {
            model: config.model.clone(),
            messages: messages.clone(),
            tools: tools.clone(),
            temperature: config.temperature,
            max_tokens: config.max_tokens,
            stream: true,
        };

        let response = llm.stream_chat(&request).await?;

        match response {
            LlmResponse::Text(text) => {
                messages.push(openintent_agent::Message::assistant(&text));
                send(socket, &OutboundMessage::text(&text)).await?;
                return Ok(());
            }
            LlmResponse::ToolCalls(calls) => {
                // Record the assistant's tool-call message.
                messages.push(openintent_agent::Message::assistant_tool_calls(
                    calls.clone(),
                ));

                // Execute each tool call and stream progress.
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

    send(
        socket,
        &OutboundMessage::text("Reached maximum number of reasoning turns."),
    )
    .await?;
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
