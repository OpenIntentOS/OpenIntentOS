//! MCP (Model Context Protocol) server implementation.
//!
//! Implements the MCP JSON-RPC 2.0 protocol over HTTP, exposing all registered
//! OpenIntentOS adapters as MCP tools.  Supports the `initialize`,
//! `tools/list`, `tools/call`, and `ping` methods.
//!
//! The MCP specification version targeted is `2024-11-05`.

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use openintent_adapters::Adapter;

use crate::state::AppState;

// ---------------------------------------------------------------------------
// MCP protocol version
// ---------------------------------------------------------------------------

/// The MCP protocol version this server implements.
const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// The server name reported during initialization.
const SERVER_NAME: &str = "OpenIntentOS";

/// The server version reported during initialization.
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// JSON-RPC types
// ---------------------------------------------------------------------------

/// A JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    /// Must be `"2.0"`.
    pub jsonrpc: String,
    /// Request identifier.  May be a number, string, or null for
    /// notifications.
    #[serde(default)]
    pub id: Option<Value>,
    /// The method to invoke.
    pub method: String,
    /// Method parameters (defaults to `null` if absent).
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Echoed from the request.
    pub id: Option<Value>,
    /// Present on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Present on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// A JSON-RPC 2.0 error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    /// Error code (negative numbers are reserved by JSON-RPC).
    pub code: i32,
    /// Human-readable error message.
    pub message: String,
    /// Optional structured data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

// Standard JSON-RPC error codes.
const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;

impl JsonRpcResponse {
    /// Construct a success response.
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Construct an error response.
    pub fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }

    /// Construct an error response with additional data.
    pub fn error_with_data(
        id: Option<Value>,
        code: i32,
        message: impl Into<String>,
        data: Value,
    ) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: Some(data),
            }),
        }
    }
}

// ---------------------------------------------------------------------------
// MCP-specific types
// ---------------------------------------------------------------------------

/// An MCP tool definition returned by `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDefinition {
    /// The machine-readable tool name.
    pub name: String,
    /// Human-readable description of the tool.
    pub description: String,
    /// JSON Schema describing the tool's input parameters.
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// The result of an MCP `tools/call` invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    /// The content blocks returned by the tool.
    pub content: Vec<McpContent>,
    /// Whether the tool call resulted in an error.
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// A single content block within an MCP tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpContent {
    /// The content type (e.g. `"text"`).
    #[serde(rename = "type")]
    pub content_type: String,
    /// The textual content.
    pub text: String,
}

impl McpContent {
    /// Create a text content block.
    pub fn text(value: impl Into<String>) -> Self {
        Self {
            content_type: "text".into(),
            text: value.into(),
        }
    }
}

impl McpToolResult {
    /// Create a successful tool result with a single text block.
    pub fn success(text: impl Into<String>) -> Self {
        Self {
            content: vec![McpContent::text(text)],
            is_error: None,
        }
    }

    /// Create an error tool result with a single text block.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![McpContent::text(text)],
            is_error: Some(true),
        }
    }
}

// ---------------------------------------------------------------------------
// McpServer
// ---------------------------------------------------------------------------

/// MCP protocol server that exposes adapters as tools.
pub struct McpServer {
    adapters: Vec<Arc<dyn Adapter>>,
}

impl McpServer {
    /// Create a new MCP server backed by the given adapters.
    pub fn new(adapters: Vec<Arc<dyn Adapter>>) -> Self {
        Self { adapters }
    }

    /// Handle a single JSON-RPC request and return a response.
    pub async fn handle_request(&self, request: JsonRpcRequest) -> JsonRpcResponse {
        tracing::debug!(method = %request.method, "MCP request received");

        match request.method.as_str() {
            "initialize" => self.handle_initialize(request.id),
            "ping" => JsonRpcResponse::success(request.id, json!({})),
            "tools/list" => self.handle_tools_list(request.id),
            "tools/call" => self.handle_tools_call(request.id, request.params).await,
            other => {
                tracing::warn!(method = %other, "unknown MCP method");
                JsonRpcResponse::error(
                    request.id,
                    METHOD_NOT_FOUND,
                    format!("method not found: {other}"),
                )
            }
        }
    }

    /// Handle the `initialize` handshake.
    fn handle_initialize(&self, id: Option<Value>) -> JsonRpcResponse {
        JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION
                }
            }),
        )
    }

    /// Handle `tools/list` by collecting tool definitions from all adapters.
    fn handle_tools_list(&self, id: Option<Value>) -> JsonRpcResponse {
        let tools = self.list_tools();
        match serde_json::to_value(&tools) {
            Ok(tools_value) => JsonRpcResponse::success(id, json!({ "tools": tools_value })),
            Err(e) => {
                tracing::error!(error = %e, "failed to serialize tool list");
                JsonRpcResponse::error(id, INTERNAL_ERROR, "failed to serialize tool list")
            }
        }
    }

    /// Handle `tools/call` by dispatching to the appropriate adapter.
    async fn handle_tools_call(&self, id: Option<Value>, params: Value) -> JsonRpcResponse {
        let name = match params.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_owned(),
            None => {
                return JsonRpcResponse::error(
                    id,
                    INVALID_PARAMS,
                    "missing required field `name` in params",
                );
            }
        };

        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        match self.call_tool(&name, arguments).await {
            Ok(result) => match serde_json::to_value(&result) {
                Ok(v) => JsonRpcResponse::success(id, v),
                Err(e) => {
                    tracing::error!(error = %e, "failed to serialize tool result");
                    JsonRpcResponse::error(id, INTERNAL_ERROR, "failed to serialize tool result")
                }
            },
            Err(msg) => {
                let result = McpToolResult::error(&msg);
                match serde_json::to_value(&result) {
                    Ok(v) => JsonRpcResponse::success(id, v),
                    Err(e) => {
                        tracing::error!(error = %e, "failed to serialize error result");
                        JsonRpcResponse::error(
                            id,
                            INTERNAL_ERROR,
                            "failed to serialize error result",
                        )
                    }
                }
            }
        }
    }

    /// Build the tool list from all adapters.
    fn list_tools(&self) -> Vec<McpToolDefinition> {
        self.adapters
            .iter()
            .flat_map(|adapter| {
                adapter.tools().into_iter().map(|t| McpToolDefinition {
                    name: t.name,
                    description: t.description,
                    input_schema: t.parameters,
                })
            })
            .collect()
    }

    /// Execute a tool call by finding the adapter that owns the tool.
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<McpToolResult, String> {
        // Find the adapter that owns this tool.
        let adapter = self
            .adapters
            .iter()
            .find(|a| a.tools().iter().any(|t| t.name == name));

        let adapter = match adapter {
            Some(a) => a,
            None => return Err(format!("unknown tool: {name}")),
        };

        match adapter.execute_tool(name, arguments).await {
            Ok(value) => {
                // Convert the JSON result to a text content block.
                let text = match value {
                    Value::String(s) => s,
                    other => {
                        serde_json::to_string_pretty(&other).unwrap_or_else(|_| other.to_string())
                    }
                };
                Ok(McpToolResult::success(text))
            }
            Err(e) => Err(format!("tool execution failed: {e}")),
        }
    }
}

// ---------------------------------------------------------------------------
// Axum handlers
// ---------------------------------------------------------------------------

/// Handle a single MCP JSON-RPC request.
///
/// Accepts `POST /mcp` with a JSON body that is either a single JSON-RPC
/// request object or an array of request objects (batch mode).
pub async fn handle_mcp_request(State(state): State<Arc<AppState>>, body: String) -> Json<Value> {
    let mcp = McpServer::new(state.adapters.clone());

    // Try to parse as an array first (batch request), then as a single request.
    if let Ok(batch) = serde_json::from_str::<Vec<JsonRpcRequest>>(&body) {
        if batch.is_empty() {
            return Json(json!(JsonRpcResponse::error(
                None,
                INVALID_REQUEST,
                "empty batch request",
            )));
        }
        let mut responses = Vec::with_capacity(batch.len());
        for req in batch {
            responses.push(mcp.handle_request(req).await);
        }
        return Json(json!(responses));
    }

    match serde_json::from_str::<JsonRpcRequest>(&body) {
        Ok(request) => {
            let response = mcp.handle_request(request).await;
            Json(json!(response))
        }
        Err(e) => Json(json!(JsonRpcResponse::error(
            None,
            PARSE_ERROR,
            format!("failed to parse JSON-RPC request: {e}"),
        ))),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use openintent_adapters::{AdapterType, AuthRequirement, HealthStatus, ToolDefinition};

    // -- Mock adapter for testing ------------------------------------------------

    struct MockAdapter {
        id: String,
        tool_defs: Vec<ToolDefinition>,
    }

    impl MockAdapter {
        fn new(id: &str, tools: Vec<ToolDefinition>) -> Self {
            Self {
                id: id.to_owned(),
                tool_defs: tools,
            }
        }
    }

    #[async_trait]
    impl Adapter for MockAdapter {
        fn id(&self) -> &str {
            &self.id
        }

        fn adapter_type(&self) -> AdapterType {
            AdapterType::System
        }

        async fn connect(&mut self) -> openintent_adapters::Result<()> {
            Ok(())
        }

        async fn disconnect(&mut self) -> openintent_adapters::Result<()> {
            Ok(())
        }

        async fn health_check(&self) -> openintent_adapters::Result<HealthStatus> {
            Ok(HealthStatus::Healthy)
        }

        fn tools(&self) -> Vec<ToolDefinition> {
            self.tool_defs.clone()
        }

        async fn execute_tool(
            &self,
            name: &str,
            _params: Value,
        ) -> openintent_adapters::Result<Value> {
            match name {
                "mock_echo" => Ok(json!({"echo": "hello"})),
                "mock_fail" => Err(openintent_adapters::AdapterError::ExecutionFailed {
                    tool_name: name.to_owned(),
                    reason: "intentional test failure".into(),
                }),
                _ => Err(openintent_adapters::AdapterError::ToolNotFound {
                    adapter_id: self.id.clone(),
                    tool_name: name.to_owned(),
                }),
            }
        }

        fn required_auth(&self) -> Option<AuthRequirement> {
            None
        }
    }

    fn mock_tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_owned(),
            description: description.to_owned(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "input": { "type": "string" }
                },
                "required": ["input"]
            }),
        }
    }

    fn mock_adapters() -> Vec<Arc<dyn Adapter>> {
        vec![
            Arc::new(MockAdapter::new(
                "mock1",
                vec![
                    mock_tool("mock_echo", "Echoes input back"),
                    mock_tool("mock_fail", "Always fails"),
                ],
            )),
            Arc::new(MockAdapter::new(
                "mock2",
                vec![mock_tool("mock_other", "Another tool")],
            )),
        ]
    }

    fn make_request(id: Value, method: &str, params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(id),
            method: method.into(),
            params,
        }
    }

    // -- Test 1: JsonRpcRequest parsing ------------------------------------------

    #[test]
    fn test_json_rpc_request_parsing() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        }"#;

        let req: JsonRpcRequest =
            serde_json::from_str(json_str).expect("should parse valid JSON-RPC request");
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.id, Some(json!(1)));
        assert_eq!(req.method, "initialize");
        assert!(req.params.is_object());
    }

    #[test]
    fn test_json_rpc_request_parsing_without_params() {
        let json_str = r#"{
            "jsonrpc": "2.0",
            "id": "abc",
            "method": "ping"
        }"#;

        let req: JsonRpcRequest =
            serde_json::from_str(json_str).expect("should parse request without params");
        assert_eq!(req.method, "ping");
        assert!(req.params.is_null());
    }

    // -- Test 2: JsonRpcResponse serialization -----------------------------------

    #[test]
    fn test_json_rpc_response_serialization_success() {
        let resp = JsonRpcResponse::success(Some(json!(1)), json!({"key": "value"}));
        let serialized = serde_json::to_value(&resp).expect("should serialize");

        assert_eq!(serialized["jsonrpc"], "2.0");
        assert_eq!(serialized["id"], 1);
        assert_eq!(serialized["result"]["key"], "value");
        assert!(serialized.get("error").is_none());
    }

    #[test]
    fn test_json_rpc_response_serialization_error() {
        let resp = JsonRpcResponse::error(Some(json!(2)), METHOD_NOT_FOUND, "not found");
        let serialized = serde_json::to_value(&resp).expect("should serialize");

        assert_eq!(serialized["jsonrpc"], "2.0");
        assert_eq!(serialized["id"], 2);
        assert!(serialized.get("result").is_none());
        assert_eq!(serialized["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(serialized["error"]["message"], "not found");
    }

    // -- Test 3: Initialize method handling --------------------------------------

    #[tokio::test]
    async fn test_initialize_method() {
        let server = McpServer::new(vec![]);
        let req = make_request(
            json!(1),
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0" }
            }),
        );

        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result = resp.result.expect("should have result");
        assert_eq!(result["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
        assert!(result["capabilities"]["tools"].is_object());
    }

    // -- Test 4: Ping method handling --------------------------------------------

    #[tokio::test]
    async fn test_ping_method() {
        let server = McpServer::new(vec![]);
        let req = make_request(json!(42), "ping", json!(null));

        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result = resp.result.expect("should have result");
        assert!(result.is_object());
        assert_eq!(result, json!({}));
    }

    // -- Test 5: tools/list with mock adapters -----------------------------------

    #[tokio::test]
    async fn test_tools_list_with_mock_adapters() {
        let server = McpServer::new(mock_adapters());
        let req = make_request(json!(3), "tools/list", json!(null));

        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result = resp.result.expect("should have result");
        let tools = result["tools"].as_array().expect("tools should be array");

        assert_eq!(tools.len(), 3);

        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("name should be string"))
            .collect();
        assert!(names.contains(&"mock_echo"));
        assert!(names.contains(&"mock_fail"));
        assert!(names.contains(&"mock_other"));

        // Verify each tool has required fields.
        for tool in tools {
            assert!(tool.get("name").is_some());
            assert!(tool.get("description").is_some());
            assert!(tool.get("inputSchema").is_some());
        }
    }

    #[tokio::test]
    async fn test_tools_list_empty_adapters() {
        let server = McpServer::new(vec![]);
        let req = make_request(json!(4), "tools/list", json!(null));

        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result = resp.result.expect("should have result");
        let tools = result["tools"].as_array().expect("tools should be array");
        assert!(tools.is_empty());
    }

    // -- Test 6: tools/call with mock adapter ------------------------------------

    #[tokio::test]
    async fn test_tools_call_success() {
        let server = McpServer::new(mock_adapters());
        let req = make_request(
            json!(5),
            "tools/call",
            json!({
                "name": "mock_echo",
                "arguments": { "input": "hello" }
            }),
        );

        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result = resp.result.expect("should have result");
        let content = result["content"]
            .as_array()
            .expect("content should be array");
        assert!(!content.is_empty());
        assert_eq!(content[0]["type"], "text");
        // The result should contain the echoed JSON.
        let text = content[0]["text"].as_str().expect("text should be string");
        assert!(text.contains("echo"));
        // isError should not be present on success.
        assert!(result.get("isError").is_none());
    }

    #[tokio::test]
    async fn test_tools_call_execution_failure() {
        let server = McpServer::new(mock_adapters());
        let req = make_request(
            json!(6),
            "tools/call",
            json!({
                "name": "mock_fail",
                "arguments": {}
            }),
        );

        let resp = server.handle_request(req).await;
        // Tool execution failures are still a successful JSON-RPC response
        // with isError=true in the content.
        assert!(resp.error.is_none());
        let result = resp.result.expect("should have result");
        assert_eq!(result["isError"], true);
        let content = result["content"]
            .as_array()
            .expect("content should be array");
        let text = content[0]["text"].as_str().expect("text should be string");
        assert!(text.contains("tool execution failed"));
    }

    #[tokio::test]
    async fn test_tools_call_unknown_tool() {
        let server = McpServer::new(mock_adapters());
        let req = make_request(
            json!(7),
            "tools/call",
            json!({
                "name": "nonexistent_tool",
                "arguments": {}
            }),
        );

        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        let result = resp.result.expect("should have result");
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"]
            .as_str()
            .expect("text should be string");
        assert!(text.contains("unknown tool"));
    }

    // -- Test 7: Unknown method returns error ------------------------------------

    #[tokio::test]
    async fn test_unknown_method_returns_error() {
        let server = McpServer::new(vec![]);
        let req = make_request(json!(8), "nonexistent/method", json!(null));

        let resp = server.handle_request(req).await;
        assert!(resp.result.is_none());
        let err = resp.error.expect("should have error");
        assert_eq!(err.code, METHOD_NOT_FOUND);
        assert!(err.message.contains("nonexistent/method"));
    }

    // -- Test 8: Invalid params returns error ------------------------------------

    #[tokio::test]
    async fn test_tools_call_missing_name_param() {
        let server = McpServer::new(mock_adapters());
        let req = make_request(json!(9), "tools/call", json!({ "arguments": {} }));

        let resp = server.handle_request(req).await;
        assert!(resp.result.is_none());
        let err = resp.error.expect("should have error");
        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.message.contains("name"));
    }

    #[tokio::test]
    async fn test_tools_call_missing_arguments_defaults_to_empty() {
        let server = McpServer::new(mock_adapters());
        let req = make_request(json!(10), "tools/call", json!({ "name": "mock_echo" }));

        let resp = server.handle_request(req).await;
        // Should succeed -- missing arguments defaults to {}.
        assert!(resp.error.is_none());
    }

    // -- Test 9: McpToolDefinition from adapter tool -----------------------------

    #[test]
    fn test_mcp_tool_definition_from_adapter_tool() {
        let adapter_tool = mock_tool("test_tool", "A test tool");
        let mcp_tool = McpToolDefinition {
            name: adapter_tool.name.clone(),
            description: adapter_tool.description.clone(),
            input_schema: adapter_tool.parameters.clone(),
        };

        assert_eq!(mcp_tool.name, "test_tool");
        assert_eq!(mcp_tool.description, "A test tool");

        // Verify serialization uses camelCase for inputSchema.
        let serialized = serde_json::to_value(&mcp_tool).expect("should serialize");
        assert!(serialized.get("inputSchema").is_some());
        assert!(serialized.get("input_schema").is_none());
    }

    // -- Test 10: McpContent construction ----------------------------------------

    #[test]
    fn test_mcp_content_construction() {
        let content = McpContent::text("Hello, world!");
        assert_eq!(content.content_type, "text");
        assert_eq!(content.text, "Hello, world!");

        // Verify serialization uses "type" field name.
        let serialized = serde_json::to_value(&content).expect("should serialize");
        assert_eq!(serialized["type"], "text");
        assert_eq!(serialized["text"], "Hello, world!");
    }

    #[test]
    fn test_mcp_tool_result_success() {
        let result = McpToolResult::success("result data");
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].text, "result data");
        assert!(result.is_error.is_none());
    }

    #[test]
    fn test_mcp_tool_result_error() {
        let result = McpToolResult::error("something went wrong");
        assert_eq!(result.content.len(), 1);
        assert_eq!(result.content[0].text, "something went wrong");
        assert_eq!(result.is_error, Some(true));
    }

    // -- Test 11: Error response construction ------------------------------------

    #[test]
    fn test_error_response_construction() {
        let resp = JsonRpcResponse::error(Some(json!(99)), INTERNAL_ERROR, "internal error");
        assert_eq!(resp.jsonrpc, "2.0");
        assert_eq!(resp.id, Some(json!(99)));
        assert!(resp.result.is_none());
        let err = resp.error.expect("should have error");
        assert_eq!(err.code, INTERNAL_ERROR);
        assert_eq!(err.message, "internal error");
        assert!(err.data.is_none());
    }

    #[test]
    fn test_error_response_with_data() {
        let resp = JsonRpcResponse::error_with_data(
            Some(json!("abc")),
            PARSE_ERROR,
            "parse error",
            json!({"detail": "unexpected token"}),
        );
        let err = resp.error.expect("should have error");
        assert_eq!(err.code, PARSE_ERROR);
        assert!(err.data.is_some());
        assert_eq!(err.data.as_ref().unwrap()["detail"], "unexpected token");
    }

    // -- Test 12: Batch request handling -----------------------------------------

    #[tokio::test]
    async fn test_batch_request_handling() {
        let server = McpServer::new(mock_adapters());

        // Simulate what the handler does with a batch request.
        let batch = vec![
            make_request(json!(1), "ping", json!(null)),
            make_request(json!(2), "tools/list", json!(null)),
            make_request(json!(3), "nonexistent", json!(null)),
        ];

        let mut responses = Vec::new();
        for req in batch {
            responses.push(server.handle_request(req).await);
        }

        assert_eq!(responses.len(), 3);

        // First: ping succeeds.
        assert!(responses[0].error.is_none());
        assert_eq!(responses[0].id, Some(json!(1)));

        // Second: tools/list succeeds.
        assert!(responses[1].error.is_none());
        assert_eq!(responses[1].id, Some(json!(2)));

        // Third: unknown method fails.
        assert!(responses[2].error.is_some());
        assert_eq!(responses[2].id, Some(json!(3)));
        assert_eq!(responses[2].error.as_ref().unwrap().code, METHOD_NOT_FOUND);
    }

    // -- Test 13 (bonus): Null request id ----------------------------------------

    #[tokio::test]
    async fn test_null_request_id() {
        let server = McpServer::new(vec![]);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: None,
            method: "ping".into(),
            params: json!(null),
        };

        let resp = server.handle_request(req).await;
        assert!(resp.error.is_none());
        assert_eq!(resp.id, None);
    }

    // -- Test 14 (bonus): Full round-trip parse handler body ---------------------

    #[test]
    fn test_parse_error_on_invalid_json() {
        // Simulate what the handler does when it receives invalid JSON.
        let body = "not valid json!!!";
        match serde_json::from_str::<JsonRpcRequest>(body) {
            Ok(_) => panic!("should not parse invalid JSON"),
            Err(e) => {
                let resp = JsonRpcResponse::error(
                    None,
                    PARSE_ERROR,
                    format!("failed to parse JSON-RPC request: {e}"),
                );
                assert!(resp.error.is_some());
                let err = resp.error.as_ref().unwrap();
                assert_eq!(err.code, PARSE_ERROR);
                assert!(err.message.contains("failed to parse"));
            }
        }

        // Also verify that a batch with invalid items fails to parse.
        let body = r#"[{"not":"valid"}]"#;
        let result = serde_json::from_str::<Vec<JsonRpcRequest>>(body);
        assert!(result.is_err());
    }
}
