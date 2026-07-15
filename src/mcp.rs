//! MCP client integration that exposes remote MCP tools as [`crate::Tool`]s.

use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    future::Future,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::{
    RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, ContentBlock,
        Implementation, ResourceContents, ServerInfo, Tool as RemoteTool,
    },
    service::{RunningService, ServerSink},
    transport::{
        IntoTransport, StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::{
    error::{McpError, ToolError},
    tool::{
        Tool, ToolOutput,
        builtins::{
            DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES,
            truncate::{TruncatedBy, truncate_head},
        },
    },
    types::ToolDefinition,
};

pub const DEFAULT_MCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_MCP_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_MCP_IMAGE_BASE64_CHARS: usize = 28 * 1024 * 1024;

/// Options shared by stdio and Streamable HTTP MCP connections.
#[derive(Clone, Debug)]
pub struct McpClientOptions {
    connect_timeout: Duration,
    request_timeout: Option<Duration>,
    tool_name_prefix: Option<String>,
    max_output_lines: usize,
    max_output_bytes: usize,
}

impl Default for McpClientOptions {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_MCP_CONNECT_TIMEOUT,
            request_timeout: Some(DEFAULT_MCP_REQUEST_TIMEOUT),
            tool_name_prefix: None,
            max_output_lines: DEFAULT_MAX_LINES,
            max_output_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

impl McpClientOptions {
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = nonzero_duration(timeout);
        self
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = Some(nonzero_duration(timeout));
        self
    }

    pub fn without_request_timeout(mut self) -> Self {
        self.request_timeout = None;
        self
    }

    /// Prefixes every exposed tool as `<prefix>__<remote-name>`.
    pub fn tool_name_prefix(mut self, prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        self.tool_name_prefix = (!prefix.is_empty()).then_some(prefix);
        self
    }

    pub fn output_limits(mut self, max_lines: usize, max_bytes: usize) -> Self {
        self.max_output_lines = max_lines.max(1);
        self.max_output_bytes = max_bytes.max(1);
        self
    }
}

/// Configuration for starting an MCP server as a child process over stdio.
#[derive(Clone)]
pub struct McpStdioConfig {
    command: OsString,
    args: Vec<OsString>,
    current_dir: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    clear_env: bool,
    options: McpClientOptions,
}

impl McpStdioConfig {
    pub fn new(command: impl Into<OsString>) -> Self {
        Self {
            command: command.into(),
            args: Vec::new(),
            current_dir: None,
            env: Vec::new(),
            clear_env: false,
            options: McpClientOptions::default(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn current_dir(mut self, current_dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(current_dir.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn envs<I, K, V>(mut self, env: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.env.extend(
            env.into_iter()
                .map(|(key, value)| (key.into(), value.into())),
        );
        self
    }

    pub fn clear_env(mut self) -> Self {
        self.clear_env = true;
        self
    }

    pub fn options(mut self, options: McpClientOptions) -> Self {
        self.options = options;
        self
    }

    pub fn tool_name_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.options = self.options.tool_name_prefix(prefix);
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.options = self.options.connect_timeout(timeout);
        self
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.options = self.options.request_timeout(timeout);
        self
    }

    pub fn output_limits(mut self, max_lines: usize, max_bytes: usize) -> Self {
        self.options = self.options.output_limits(max_lines, max_bytes);
        self
    }
}

/// Configuration for an MCP Streamable HTTP endpoint.
#[derive(Clone)]
pub struct McpHttpConfig {
    uri: String,
    bearer_token: Option<String>,
    headers: Vec<(String, String)>,
    allow_stateless: bool,
    reinitialize_on_expired_session: bool,
    options: McpClientOptions,
}

impl McpHttpConfig {
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            uri: uri.into(),
            bearer_token: None,
            headers: Vec::new(),
            allow_stateless: true,
            reinitialize_on_expired_session: true,
            options: McpClientOptions::default(),
        }
    }

    /// Sets a bearer token without the `Bearer ` prefix.
    pub fn bearer_token(mut self, token: impl Into<String>) -> Self {
        self.bearer_token = Some(token.into());
        self
    }

    /// Adds a custom header. Header names and values are validated during connect.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn allow_stateless(mut self, allow: bool) -> Self {
        self.allow_stateless = allow;
        self
    }

    pub fn reinitialize_on_expired_session(mut self, enabled: bool) -> Self {
        self.reinitialize_on_expired_session = enabled;
        self
    }

    pub fn options(mut self, options: McpClientOptions) -> Self {
        self.options = options;
        self
    }

    pub fn tool_name_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.options = self.options.tool_name_prefix(prefix);
        self
    }

    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.options = self.options.connect_timeout(timeout);
        self
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.options = self.options.request_timeout(timeout);
        self
    }

    pub fn output_limits(mut self, max_lines: usize, max_bytes: usize) -> Self {
        self.options = self.options.output_limits(max_lines, max_bytes);
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpServerInfo {
    pub protocol_version: String,
    pub name: String,
    pub version: String,
    pub instructions: Option<String>,
}

/// A connected MCP server and the snapshot of tools discovered at connect time.
#[derive(Clone)]
pub struct McpClient {
    session: Arc<McpSession>,
    tools: Vec<Arc<dyn Tool>>,
    server_info: McpServerInfo,
}

impl McpClient {
    pub async fn connect_stdio(config: McpStdioConfig) -> Result<Self, McpError> {
        let McpStdioConfig {
            command,
            args,
            current_dir,
            env,
            clear_env,
            options,
        } = config;

        let mut command = tokio::process::Command::new(command);
        command.args(args);
        if let Some(current_dir) = current_dir {
            command.current_dir(current_dir);
        }
        if clear_env {
            command.env_clear();
        }
        command.envs(env);

        let transport = TokioChildProcess::new(command).map_err(McpError::Spawn)?;
        let running = connect_transport(transport, options.connect_timeout).await?;
        Self::from_running(running, options).await
    }

    pub async fn connect_http(config: McpHttpConfig) -> Result<Self, McpError> {
        let McpHttpConfig {
            uri,
            bearer_token,
            headers,
            allow_stateless,
            reinitialize_on_expired_session,
            options,
        } = config;

        let mut custom_headers = HashMap::with_capacity(headers.len());
        for (name, value) in headers {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                McpError::InvalidHttpHeader {
                    name: name.clone(),
                    message: error.to_string(),
                }
            })?;
            let header_value =
                HeaderValue::from_str(&value).map_err(|error| McpError::InvalidHttpHeader {
                    name: name.clone(),
                    message: error.to_string(),
                })?;
            custom_headers.insert(header_name, header_value);
        }

        let mut transport_config = StreamableHttpClientTransportConfig::with_uri(uri);
        transport_config.auth_header = bearer_token;
        transport_config.custom_headers = custom_headers;
        transport_config.allow_stateless = allow_stateless;
        transport_config.reinit_on_expired_session = reinitialize_on_expired_session;

        let transport = StreamableHttpClientTransport::from_config(transport_config);
        let running = connect_transport(transport, options.connect_timeout).await?;
        Self::from_running(running, options).await
    }

    pub fn server_info(&self) -> &McpServerInfo {
        &self.server_info
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|tool| tool.definition()).collect()
    }

    pub fn is_closed(&self) -> bool {
        self.session.peer.is_transport_closed()
    }

    /// Gracefully closes the shared connection. Calls through cloned clients or agents fail after
    /// this returns.
    pub async fn close(&self) -> Result<(), McpError> {
        self.session.close().await
    }

    pub(crate) fn into_tools(self) -> Vec<Arc<dyn Tool>> {
        self.tools
    }

    async fn from_running(
        running: RunningService<RoleClient, ClientInfo>,
        options: McpClientOptions,
    ) -> Result<Self, McpError> {
        let raw_server_info = running.peer_info().ok_or(McpError::MissingServerInfo)?;
        let server_info = McpServerInfo::from(raw_server_info.as_ref());

        let remote_tools = if raw_server_info.capabilities.tools.is_some() {
            request_with_timeout(
                "tools/list",
                options.request_timeout,
                running.list_all_tools(),
            )
            .await?
        } else {
            Vec::new()
        };

        let session = Arc::new(McpSession {
            peer: running.peer().clone(),
            running: Mutex::new(Some(running)),
            request_timeout: options.request_timeout,
        });
        let mut names = HashSet::with_capacity(remote_tools.len());
        let mut tools: Vec<Arc<dyn Tool>> = Vec::with_capacity(remote_tools.len());
        for remote_tool in remote_tools {
            let (remote_name, definition) =
                tool_definition(&remote_tool, options.tool_name_prefix.as_deref());
            if !names.insert(definition.name.clone()) {
                return Err(McpError::DuplicateToolName(definition.name));
            }
            tools.push(Arc::new(McpTool {
                remote_name,
                definition,
                session: Arc::clone(&session),
                max_output_lines: options.max_output_lines,
                max_output_bytes: options.max_output_bytes,
            }));
        }

        Ok(Self {
            session,
            tools,
            server_info,
        })
    }
}

impl From<&ServerInfo> for McpServerInfo {
    fn from(info: &ServerInfo) -> Self {
        Self {
            protocol_version: info.protocol_version.as_str().to_owned(),
            name: info.server_info.name.clone(),
            version: info.server_info.version.clone(),
            instructions: info.instructions.clone(),
        }
    }
}

struct McpSession {
    peer: ServerSink,
    running: Mutex<Option<RunningService<RoleClient, ClientInfo>>>,
    request_timeout: Option<Duration>,
}

impl McpSession {
    async fn call_tool(&self, params: CallToolRequestParams) -> Result<CallToolResult, ToolError> {
        let call = self.peer.call_tool(params);
        match self.request_timeout {
            Some(timeout) => match tokio::time::timeout(timeout, call).await {
                Ok(result) => result
                    .map_err(|error| ToolError::new(format!("MCP tools/call failed: {error}"))),
                Err(_) => Err(ToolError::new(format!(
                    "MCP tools/call timed out after {timeout:?}"
                ))),
            },
            None => call
                .await
                .map_err(|error| ToolError::new(format!("MCP tools/call failed: {error}"))),
        }
    }

    async fn close(&self) -> Result<(), McpError> {
        let running = self.running.lock().await.take();
        if let Some(running) = running {
            running
                .cancel()
                .await
                .map_err(|error| McpError::Close(error.to_string()))?;
        }
        Ok(())
    }
}

struct McpTool {
    remote_name: String,
    definition: ToolDefinition,
    session: Arc<McpSession>,
    max_output_lines: usize,
    max_output_bytes: usize,
}

#[async_trait]
impl Tool for McpTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn execute(&self, arguments: Value) -> Result<ToolOutput, ToolError> {
        let arguments = match arguments {
            Value::Object(arguments) => Some(arguments),
            Value::Null => None,
            _ => {
                return Err(ToolError::new(format!(
                    "invalid arguments for MCP tool `{}`: expected an object",
                    self.remote_name
                )));
            }
        };

        let mut params = CallToolRequestParams::new(self.remote_name.clone());
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }
        let result = self.session.call_tool(params).await?;
        Ok(format_tool_result(
            result,
            self.max_output_lines,
            self.max_output_bytes,
        ))
    }
}

async fn connect_transport<T, E, A>(
    transport: T,
    timeout: Duration,
) -> Result<RunningService<RoleClient, ClientInfo>, McpError>
where
    T: IntoTransport<RoleClient, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("phi", env!("CARGO_PKG_VERSION")),
    );
    match tokio::time::timeout(timeout, client_info.serve(transport)).await {
        Ok(Ok(running)) => Ok(running),
        Ok(Err(error)) => Err(McpError::Initialize {
            message: error.to_string(),
        }),
        Err(_) => Err(McpError::ConnectTimeout { timeout }),
    }
}

async fn request_with_timeout<T, E, F>(
    operation: &'static str,
    timeout: Option<Duration>,
    request: F,
) -> Result<T, McpError>
where
    E: std::fmt::Display,
    F: Future<Output = Result<T, E>>,
{
    match timeout {
        Some(timeout) => match tokio::time::timeout(timeout, request).await {
            Ok(result) => result.map_err(|error| McpError::Request {
                operation,
                message: error.to_string(),
            }),
            Err(_) => Err(McpError::RequestTimeout { operation, timeout }),
        },
        None => request.await.map_err(|error| McpError::Request {
            operation,
            message: error.to_string(),
        }),
    }
}

fn tool_definition(remote_tool: &RemoteTool, prefix: Option<&str>) -> (String, ToolDefinition) {
    let remote_name = remote_tool.name.to_string();
    let exposed_name = prefix.map_or_else(
        || remote_name.clone(),
        |prefix| format!("{prefix}__{remote_name}"),
    );
    let definition = ToolDefinition::new(
        exposed_name,
        remote_tool
            .description
            .as_deref()
            .unwrap_or("MCP server tool"),
        remote_tool.schema_as_json_value(),
    );
    (remote_name, definition)
}

fn format_tool_result(result: CallToolResult, max_lines: usize, max_bytes: usize) -> ToolOutput {
    let is_error = result.is_error.unwrap_or(false);
    let mut chunks = Vec::new();
    let mut content_parts = Vec::new();
    for block in result.content {
        match block {
            ContentBlock::Image(image) => {
                if image.data.len() <= MAX_MCP_IMAGE_BASE64_CHARS {
                    content_parts.push(crate::types::ContentPart::image_url(format!(
                        "data:{};base64,{}",
                        image.mime_type, image.data
                    )));
                    chunks.push(format!("[MCP image: {}]", image.mime_type));
                } else {
                    chunks.push(format!(
                        "[MCP image omitted: {}, {} base64 characters exceeds attachment limit]",
                        image.mime_type,
                        image.data.len()
                    ));
                }
            }
            block => {
                let chunk = render_content_block(block);
                if !chunk.is_empty() {
                    chunks.push(chunk);
                }
            }
        }
    }

    let structured_content = result.structured_content;
    if let Some(structured) = structured_content.as_ref()
        && !chunks.iter().any(|chunk| {
            serde_json::from_str::<Value>(chunk).is_ok_and(|content| content == *structured)
        })
    {
        let structured =
            serde_json::to_string_pretty(&structured).unwrap_or_else(|_| structured.to_string());
        chunks.push(format!("[MCP structured content]\n{structured}"));
    }

    let content = if chunks.is_empty() {
        "(MCP tool returned no content)".to_owned()
    } else {
        chunks.join("\n\n")
    };
    let content = truncate_tool_output(&content, max_lines, max_bytes);
    let mut output = if is_error {
        ToolOutput::error(content)
    } else {
        ToolOutput::success(content)
    }
    .with_content_parts(content_parts);
    if let Some(structured) = structured_content {
        let structured_bytes = serde_json::to_vec(&structured)
            .map_or(max_bytes.saturating_add(1), |encoded| encoded.len());
        output = if structured_bytes <= max_bytes {
            output.with_metadata(serde_json::json!({
                "mcp_structured_content": structured
            }))
        } else {
            output.with_metadata(serde_json::json!({
                "mcp_structured_content_truncated": true,
                "original_bytes": structured_bytes
            }))
        };
    }
    output
}

fn render_content_block(content: ContentBlock) -> String {
    match content {
        ContentBlock::Text(text) => text.text,
        ContentBlock::Image(image) => format!("[MCP image: {}]", image.mime_type),
        ContentBlock::Audio(audio) => format!(
            "[MCP audio: {}, {} base64 characters omitted]",
            audio.mime_type,
            audio.data.len()
        ),
        ContentBlock::Resource(resource) => match resource.resource {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                ..
            } => format!(
                "[MCP resource: {uri}{}]\n{text}",
                mime_type.map_or_else(String::new, |mime| format!(", {mime}"))
            ),
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => format!(
                "[MCP resource: {uri}{}, {} base64 characters omitted]",
                mime_type.map_or_else(String::new, |mime| format!(", {mime}")),
                blob.len()
            ),
            _ => "[MCP resource with unsupported content]".to_owned(),
        },
        ContentBlock::ResourceLink(link) => {
            let link = serde_json::to_string(&link).unwrap_or_else(|_| link.uri.clone());
            format!("[MCP resource link]\n{link}")
        }
        _ => "[MCP content block with unsupported type]".to_owned(),
    }
}

fn truncate_tool_output(content: &str, max_lines: usize, max_bytes: usize) -> String {
    let truncated = truncate_head(content, max_lines, max_bytes);
    if !truncated.truncated {
        return truncated.content;
    }

    let (shown, shown_lines) = if truncated.first_line_exceeds_limit {
        (prefix_at_char_boundary(content, max_bytes), 1)
    } else {
        (truncated.content, truncated.output_lines)
    };
    let reason = if truncated.truncated_by == Some(TruncatedBy::Lines) {
        format!("{max_lines} line limit")
    } else {
        format!("{max_bytes} byte limit")
    };
    format!(
        "{shown}\n\n[MCP output truncated by {reason}; showing {shown_lines} of {} lines and at most {} of {} bytes.]",
        truncated.total_lines,
        shown.len(),
        truncated.total_bytes
    )
}

fn prefix_at_char_boundary(content: &str, max_bytes: usize) -> String {
    let mut end = content.len().min(max_bytes);
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    content[..end].to_owned()
}

fn nonzero_duration(duration: Duration) -> Duration {
    duration.max(Duration::from_millis(1))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rmcp::{
        ServerHandler,
        model::{
            CallToolRequestParams, ContentBlock, ErrorData, Implementation, ListToolsResult,
            PaginatedRequestParams, ServerCapabilities, ServerInfo,
        },
        service::{RequestContext, RoleServer},
    };
    use serde_json::json;

    use super::*;

    #[test]
    fn prefixes_remote_tool_names() {
        let remote = RemoteTool::new("echo", "Echo text", serde_json::Map::new());
        let (remote_name, definition) = tool_definition(&remote, Some("demo"));

        assert_eq!(remote_name, "echo");
        assert_eq!(definition.name, "demo__echo");
        assert_eq!(definition.description, "Echo text");
        assert_eq!(definition.parameters, json!({}));
    }

    #[test]
    fn preserves_tool_level_errors_and_structured_content() {
        let mut result = CallToolResult::error(vec![ContentBlock::text("lookup failed")]);
        result.structured_content = Some(json!({ "code": "NOT_FOUND" }));

        let output = format_tool_result(result, 100, 10_000);

        assert!(output.is_error);
        assert!(output.content.contains("lookup failed"));
        assert!(output.content.contains("NOT_FOUND"));
        assert_eq!(
            output.metadata,
            Some(json!({
                "mcp_structured_content": { "code": "NOT_FOUND" }
            }))
        );
    }

    #[test]
    fn does_not_duplicate_equivalent_structured_json() {
        let output = format_tool_result(
            CallToolResult::structured(json!({ "value": 3 })),
            100,
            10_000,
        );

        assert_eq!(output.content.matches("\"value\"").count(), 1);
    }

    #[test]
    fn bounds_structured_metadata_independently_of_the_text_fallback() {
        let output = format_tool_result(
            CallToolResult::structured(json!({ "value": "x".repeat(1_000) })),
            100,
            64,
        );

        assert_eq!(
            output.metadata.as_ref().unwrap()["mcp_structured_content_truncated"],
            true
        );
        assert!(
            output.metadata.as_ref().unwrap()["original_bytes"]
                .as_u64()
                .unwrap()
                > 64
        );
        assert!(output.content.len() < 256);
    }

    #[test]
    fn preserves_images_as_rich_content_with_a_safe_text_fallback() {
        let output = format_tool_result(
            CallToolResult::success(vec![ContentBlock::image("secret-base64", "image/png")]),
            100,
            10_000,
        );

        assert!(output.content.contains("MCP image"));
        assert!(!output.content.contains("secret-base64"));
        assert_eq!(output.content_parts.len(), 1);
        assert_eq!(
            output.content_parts[0],
            crate::types::ContentPart::image_url("data:image/png;base64,secret-base64")
        );
    }

    #[test]
    fn truncates_long_single_line_on_a_utf8_boundary() {
        let output = truncate_tool_output("前缀-abcdefghijklmnopqrstuvwxyz", 100, 10);

        assert!(output.starts_with("前缀-abc"));
        assert!(output.contains("MCP output truncated"));
    }

    #[tokio::test]
    async fn rejects_invalid_http_headers_before_connecting() {
        let error = McpClient::connect_http(
            McpHttpConfig::new("http://127.0.0.1:1/mcp").header("bad header", "value"),
        )
        .await
        .err()
        .unwrap();

        assert!(matches!(error, McpError::InvalidHttpHeader { .. }));
    }

    #[test]
    fn environment_builder_accepts_multiple_values() {
        let config =
            McpStdioConfig::new("server").envs(HashMap::from([("ONE", "1"), ("TWO", "2")]));

        assert_eq!(config.env.len(), 2);
    }

    #[derive(Clone)]
    struct TestMcpServer;

    impl ServerHandler for TestMcpServer {
        fn get_info(&self) -> ServerInfo {
            ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
                .with_server_info(Implementation::new("test-mcp", "1.0.0"))
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            Ok(ListToolsResult::with_all_items(vec![RemoteTool::new(
                "echo",
                "Echo a string",
                serde_json::from_value::<serde_json::Map<String, Value>>(json!({
                    "type": "object",
                    "properties": { "text": { "type": "string" } },
                    "required": ["text"]
                }))
                .unwrap(),
            )]))
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResult, ErrorData> {
            let text = request
                .arguments
                .as_ref()
                .and_then(|arguments| arguments.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                "echo:{text}"
            ))]))
        }
    }

    #[tokio::test]
    async fn discovers_calls_and_closes_over_an_mcp_transport() {
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1_024);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (server_read, server_write) = tokio::io::split(server_stream);
        let server_task = tokio::spawn(async move {
            TestMcpServer
                .serve((server_read, server_write))
                .await
                .unwrap()
        });

        let running = connect_transport((client_read, client_write), DEFAULT_MCP_CONNECT_TIMEOUT)
            .await
            .unwrap();
        let client = McpClient::from_running(
            running,
            McpClientOptions::default().tool_name_prefix("test"),
        )
        .await
        .unwrap();
        let server = server_task.await.unwrap();

        assert_eq!(client.server_info().name, "test-mcp");
        assert_eq!(client.tool_definitions()[0].name, "test__echo");
        let output = client.tools[0]
            .execute(json!({ "text": "hello" }))
            .await
            .unwrap();
        assert_eq!(output, ToolOutput::success("echo:hello"));

        client.close().await.unwrap();
        server.cancel().await.unwrap();
        assert!(client.is_closed());
    }
}
