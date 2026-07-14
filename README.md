# phi

`phi` 是一个受 [Pi agent-core](https://github.com/earendil-works/pi/tree/main/packages/agent) 启发的 Rust Agent SDK。Agent core 只处理规范化消息、工具调用和生成配置；模型、鉴权、URL 与厂商 wire format 全部封装在 provider adapter 中。

## 已实现

- 有状态的多轮 `Agent`
- `LlmProvider` 与 `Tool` trait
- OpenAI Chat Completions adapter
- OpenAI Responses adapter
- Anthropic Messages adapter
- 任意 base URL、API key 和 model 配置
- 跨协议 token usage 与上下文剩余量统计
- SSE 文本与工具参数流式输出，非流式 API 自动消费同一事件流
- 跨协议思考强度（reasoning effort）配置
- 可插拔 session 持久化，内置 in-memory 与 disk storage
- 规范化工具调用与增量参数事件
- 文本、图片 URL 和 base64 data URL 多模态 `content`
- 顺序或并行工具执行，工具结果按调用顺序写回上下文
- 默认关闭、可按需启用的 `read`、`bash`、`edit`、`write` 内置工具
- MCP client：支持 stdio 与 Streamable HTTP server 的工具发现和调用
- 可配置 HTTP 超时、错误重试和指数退避
- 可修改请求、响应和 turn 数据的异步生命周期 Hooks
- `agent_start`、turn、message、tool 和 error 生命周期事件
- 最大轮数保护与可复用对话历史

## 快速运行

复制配置模板并填入任意 OpenAI Chat Completions 兼容服务：

```bash
cp .env.example .env
cargo run --example agent
```

也可以传入自己的提示词：

```bash
cargo run --example agent -- "调用 character_count 统计 hello世界 的字符数"
```

## SDK 用法

```rust
use async_trait::async_trait;
use phi::{
    Agent, OpenAiChatProvider, ReasoningEffort, Tool, ToolDefinition, ToolError,
    ToolOutput,
};
use serde_json::json;

struct Echo;

#[async_trait]
impl Tool for Echo {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "echo",
            "Return the supplied text",
            json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }),
        )
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        let text = arguments["text"]
            .as_str()
            .ok_or_else(|| ToolError::new("text is required"))?;
        Ok(ToolOutput::success(text))
    }
}

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let provider = OpenAiChatProvider::new(
    std::env::var("LLM_API_KEY")?,
    std::env::var("LLM_BASE_URL")?,
    std::env::var("LLM_MODEL")?,
)?;
let mut agent = Agent::builder(provider)
    .system_prompt("Use tools when helpful.")
    .temperature(0.2)
    .max_tokens(4096)
    .reasoning_effort(ReasoningEffort::Medium)
    .max_context_tokens(128_000)
    .tool(Echo)
    .build();

let result = agent.prompt("Echo hello").await?;
println!("{}", result.text().unwrap_or_default());
# Ok(())
# }
```

任意 OpenAI 兼容服务都可以通过下列构造器接入：

```rust
# use phi::OpenAiChatProvider;
let provider = OpenAiChatProvider::new(
    "api-key",
    "https://example.com/v1",
    "model-name",
)?;
# Ok::<(), phi::ProviderError>(())
```

## 内置工具

内置工具默认全部关闭，不会因为构建 `Agent` 而自动获得文件系统或 shell 权限。需要在 Builder 中显式启用，并指定相对路径和命令使用的工作目录：

```rust
use phi::{Agent, BuiltinTools, OpenAiChatProvider};

# let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model")?;
let agent = Agent::builder(provider)
    .builtin_tools(BuiltinTools::all("/workspace/project"))
    .build();
# Ok::<(), phi::ProviderError>(())
```

也可以只开放部分能力：

```rust
use phi::{Agent, BuiltinTools, OpenAiChatProvider};

# let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model")?;
let tools = BuiltinTools::none("/workspace/project")
    .with_read()
    .with_edit();
let agent = Agent::builder(provider).builtin_tools(tools).build();
# Ok::<(), phi::ProviderError>(())
```

`AgentBuilder::all_builtin_tools(cwd)` 是启用全部四项能力的快捷方式。各工具类型也可单独注册并配置，例如 `.tool(BashTool::new(cwd).shell("/bin/zsh"))`。

- `read`：读取文本，支持 1-based `offset` 和 `limit`；默认保留头部最多 2000 行或 50KB，并返回继续读取提示。
- `bash`：在配置目录中执行 shell 命令，合并 stdout/stderr；可传入 `timeout` 秒数。默认保留尾部最多 2000 行或 50KB，发生截断时把完整输出保存到临时文件。非零退出码和超时会作为 tool error 返回。
- `edit`：一个调用可提交多个精确替换；每个 `oldText` 必须在原文件中唯一，各替换不能重叠。保留 UTF-8 BOM 和原有 CRLF/LF 换行风格。
- `write`：创建或完全覆盖文件，并递归创建父目录。

`edit` 与 `write` 会按目标文件串行化，因此即使 Agent 使用并行工具执行，同一文件也不会同时写入。不同文件仍可并行。

这些工具遵循 Pi 的本地工具语义：工作目录只用于解析相对路径，不是安全沙箱；绝对路径和包含 `..` 的路径仍可访问工作目录以外的位置，`bash` 也可以执行当前进程权限允许的任意命令。只应向可信 Agent 显式开放所需的最小能力。

## MCP Server

MCP 默认不会建立任何连接。可以让 Builder 启动一个 stdio server、完成 MCP 初始化、发现其 tools，并把这些远端 tools 注册到 Agent：

```rust
use std::time::Duration;
use phi::{Agent, McpStdioConfig, OpenAiChatProvider};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
# let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model")?;
let mcp = McpStdioConfig::new("npx")
    .args([
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/workspace/project",
    ])
    // 多个 MCP server 或本地工具可能重名，建议设置前缀。
    .tool_name_prefix("filesystem")
    .connect_timeout(Duration::from_secs(20))
    .request_timeout(Duration::from_secs(60));

let agent = Agent::builder(provider)
    .mcp_stdio(mcp)
    .await?
    .build();
# Ok(())
# }
```

远程 MCP 使用当前标准的 Streamable HTTP transport，支持 bearer token 和自定义 header：

```rust
use phi::{Agent, McpHttpConfig, OpenAiChatProvider};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
# let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model")?;
let mcp = McpHttpConfig::new("https://mcp.example.com/mcp")
    .bearer_token(std::env::var("MCP_TOKEN")?)
    .header("x-tenant-id", "tenant-1")
    .tool_name_prefix("remote");

let agent = Agent::builder(provider)
    .mcp_http(mcp)
    .await?
    .build();
# Ok(())
# }
```

也可以先通过 `McpClient::connect_stdio` 或 `McpClient::connect_http` 建连，检查 `server_info()` 与 `tool_definitions()` 后，再用 `.mcp_client(client)` 注册。一个 Agent 可以注册多个 MCP client。

- 默认连接超时为 30 秒，`tools/list` 和 `tools/call` 超时为 60 秒，均可配置；通过 `McpClientOptions::without_request_timeout()` 可以关闭请求超时。
- 工具列表是连接时的快照。MCP server 后续发送 `tools/list_changed` 时，需要重新连接并构建 Agent 才会更新。
- MCP 的 tool-level `isError`、JSON-RPC/transport 错误和超时都会转为现有 tool error，因此调用方会收到 `AgentEvent::ToolExecutionEnd { is_error: true, .. }`。
- 文本和 structured content 会写回模型上下文，默认限制为 2000 行或 50KB。由于当前 `ToolOutput` 是文本，image、audio 和二进制 resource 只返回类型与大小摘要，不把 base64 数据写进上下文。
- stdio 子进程和远端 server 都拥有其进程或服务端权限。工具 schema、描述和执行结果也属于不可信输入；只应连接可信 MCP server，并为不同 server 设置前缀以避免工具名冲突。

## Provider 边界

核心 trait 不暴露模型名、endpoint、鉴权或任何厂商 JSON：

```rust
#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    fn stream(&self, request: ProviderRequest) -> ProviderEventStream;

    async fn generate(
        &self,
        request: ProviderRequest,
    ) -> Result<ProviderResponse, ProviderError> {
        // 默认消费 stream，直到 ProviderEvent::Done。
    }
}
```

`ProviderRequest` 只包含：

- `messages: Vec<Message>`
- `tools: Vec<ToolDefinition>`
- `config: GenerationConfig`，目前包括 `temperature`、`max_tokens` 和 `reasoning_effort`

Provider 返回 `ProviderResponse`：规范化的 `AssistantMessage` 加可选 `TokenUsage`。Agent 不需要知道上游响应字段叫 `prompt_tokens`、`input_tokens` 还是缓存 token 字段。

三个 adapter 分别负责不同协议的无损转换：

```rust
use phi::{AnthropicMessagesProvider, OpenAiResponsesProvider};

let responses = OpenAiResponsesProvider::openai("openai-key", "model-name")?;
let claude = AnthropicMessagesProvider::new("anthropic-key", "model-name")?;
# Ok::<(), phi::ProviderError>(())
```

- Chat Completions：assistant `tool_calls` 与 `tool` role
- Responses：typed `message`、`function_call`、`function_call_output` Items
- Claude Messages：`tool_use` 与 `tool_result` content blocks，system message 转顶层 `system`

响应中的思考数据会作为 opaque `ProviderState` 随 assistant `Message` 保留，并在后续请求中按原协议回放：

- Chat Completions：`reasoning_content`、`reasoning` 与完整 `reasoning_details`
- Responses：原始 output Items，包括 `type: "reasoning"` Items
- Anthropic Messages：原始 `thinking`/`redacted_thinking` content blocks，包括 `signature`

工具续轮与普通下一轮都会保留这些数据。业务代码复制或重建 `Message` 时也应保留
`provider_state`；其 `Debug` 输出只展示类型和数量，不打印思考正文。

## 扩展请求体

需要接入兼容网关或厂商专有能力时，三个内置 HTTP provider 都可用 `.extra_body(...)` 为每次请求追加固定 JSON 字段（包括 Agent 的后续工具轮次）：

```rust
use phi::OpenAiChatProvider;
use serde_json::json;

let provider = OpenAiChatProvider::new(
    "api-key",
    "https://example.com/v1",
    "model-name",
)?
.extra_body(json!({
    "chat_template_kwargs": { "enable_thinking": true }
}))?;
# Ok::<(), phi::ProviderError>(())
```

传入值必须是 JSON object。其顶层字段会在 adapter 生成标准请求体后写入；同名字段以 `.extra_body(...)` 的值为准。因此只应在确有需要时覆盖 `model`、`messages`/`input`、`stream` 等标准字段。

## 异步 Hooks

实现 `Hook` 并通过 `AgentBuilder::hook(...)` 注册后，同一个 Hook 会同时作用于 Agent 生命周期和三个内置 HTTP Provider。多个 Hook 按注册顺序串行 `await`，后注册的 Hook 能看到前面 Hook 的修改；任一 Hook 返回错误都会中止当前运行。

下面的 Hook 在 Anthropic Messages 请求实际发送前，为最后一条 user text block 添加显式 Prompt Cache 断点，并追加一个 header：

```rust
use async_trait::async_trait;
use phi::{
    Agent, AnthropicMessagesProvider, BeforeRequestContext, Hook, HookError,
    ProviderApi,
};
use reqwest::header::HeaderValue;
use serde_json::json;

struct CacheLastUserText;

#[async_trait]
impl Hook for CacheLastUserText {
    async fn before_request(
        &self,
        context: &mut BeforeRequestContext,
    ) -> Result<(), HookError> {
        if context.api != ProviderApi::AnthropicMessages {
            return Ok(());
        }

        if let Some(messages) = context.body["messages"].as_array_mut() {
            for message in messages.iter_mut().rev() {
                if message["role"] != "user" {
                    continue;
                }
                let Some(blocks) = message["content"].as_array_mut() else {
                    continue;
                };
                if let Some(block) = blocks
                    .iter_mut()
                    .rev()
                    .find(|block| block["type"] == "text")
                {
                    block["cache_control"] = json!({ "type": "ephemeral" });
                    break;
                }
            }
        }
        context.headers.insert(
            "x-client-hook",
            HeaderValue::from_static("enabled"),
        );
        Ok(())
    }
}

# fn build() -> Result<(), Box<dyn std::error::Error>> {
let provider = AnthropicMessagesProvider::new("api-key", "model-name")?;
let _agent = Agent::builder(provider)
    .hook(CacheLastUserText)
    .build();
# Ok(())
# }
```

可实现的四个异步时机：

| Hook 方法 | 可修改内容 | 执行位置 |
| --- | --- | --- |
| `before_request` | 最终 `body`、`headers`、`endpoint` | adapter 序列化及 `extra_body` 合并之后，第一次 HTTP 尝试之前 |
| `on_turn_start` | 规范化 `ProviderRequest`，包括 messages、tools 和生成配置 | 每个 Agent turn 开始、调用 Provider 之前 |
| `on_llm_response` | 完整 `ProviderResponse`，包括 assistant 内容、tool calls 和 usage | SSE 响应完整结束、写入 Agent 状态之前 |
| `on_turn_end` | 本轮 assistant message 和 tool-result messages | 工具执行完成之后、发出 `TurnEnd` 及进入下一轮之前 |

`before_request` 每个逻辑 LLM 请求只执行一次，其修改结果会用于该请求的所有 HTTP 重试。若要修改 assistant 响应并让 `MessageEnd` 等下游事件看到修改结果，应使用 `on_llm_response`；`on_turn_end` 更适合基于工具执行结果调整下一轮上下文。直接调用内置 Provider 而不构建 Agent 时，也可以通过 Provider 自身的 `.hook(...)` 或 `.hooks(...)` 注册请求 Hook。

## HTTP 超时与重试

三个内置 Provider 共用同一套重试策略。默认在首次请求失败后最多重试 10 次；该数字由 `RetryConfig` 管理，不写死在请求循环中。`max_retries` 表示“额外重试次数”，所以配置为 10 时最多会发送 11 次 HTTP 请求，配置为 0 时关闭重试：

```rust
use std::time::Duration;
use phi::{OpenAiChatProvider, RetryConfig};

let retry = RetryConfig::default()
    .with_max_retries(5)
    .with_request_timeout(Duration::from_secs(20))
    .with_initial_backoff(Duration::from_millis(250))
    .with_max_backoff(Duration::from_secs(8))
    .with_rate_limit_backoff(Duration::from_secs(1));

let provider = OpenAiChatProvider::new(
    "api-key",
    "https://example.com/v1",
    "model-name",
)?.retry_config(retry);
# Ok::<(), phi::ProviderError>(())
```

重试分类如下：

- 连接、请求传输和响应头超时：带随机抖动的指数退避。
- HTTP 408、409、425，以及除 501/505 外的 5xx：带随机抖动的指数退避，且受 `max_backoff` 限制。
- HTTP 429：优先使用 `Retry-After`（同时支持秒数和 HTTP 日期）；缺失或无效时使用固定的 `rate_limit_backoff`，不做指数增长。等待时间同样受 `max_backoff` 限制。
- 其他非 2xx 响应均作为 `ProviderError::Api` 返回，但不会重试。包括 400、401、403、404 等永久性客户端错误，以及无法正常跟随的 3xx。
- 请求构造、响应解码和重定向错误不会重试。

请求超时只覆盖建立连接到收到响应头的阶段。SSE 流一旦建立就不会自动重放，因为重放已经输出一部分的流可能造成重复文本、重复计费或重复工具调用。`examples/agent.rs` 可以通过 `LLM_MAX_RETRIES` 和 `LLM_REQUEST_TIMEOUT_SECS` 覆盖默认配置。

每次准备重试时，Agent 会实时发出 `AgentEvent::ProviderRetry`。调用方可以通过已有的事件订阅机制记录日志、更新 UI 或上报监控：

```rust
use phi::AgentEvent;

agent.subscribe(|event| {
    if let AgentEvent::ProviderRetry { event } = event {
        eprintln!(
            "provider retry {}/{} after {:?}: {:?}",
            event.retry_number,
            event.max_retries,
            event.delay,
            event.reason,
        );
    }
});
```

`ProviderRetryReason` 会区分响应头超时、传输错误和 HTTP 状态错误；HTTP 错误同时包含状态码与响应体。事件在退避等待之前发出。重试次数耗尽或遇到不可重试错误时，仍会发出原有的 `AgentEvent::Error`，并从 `Agent::prompt()` 返回 `AgentError::Provider`。直接使用 `provider.stream()` 时，同一个通知以 `ProviderEvent::Retry` 返回。

## Session 持久化

`SessionStorage` 是一个只处理规范化 session 快照的异步接口：

```rust
#[async_trait::async_trait]
pub trait SessionStorage: Send + Sync {
    async fn load(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionSnapshot>, StorageError>;

    async fn save(&self, session: &SessionSnapshot) -> Result<(), StorageError>;
    async fn delete(&self, session_id: &str) -> Result<(), StorageError>;
}
```

内存 storage 适合测试或单进程临时会话；clone 后共享同一份状态：

```rust
use phi::{Agent, InMemorySessionStorage};

# async fn attach(mut agent: Agent) -> Result<(), phi::AgentError> {
let storage = InMemorySessionStorage::new();
agent.attach_session("user-42", storage.clone()).await?;
# Ok(())
# }
```

磁盘 storage 使用带格式版本的增量 JSONL 文件：

```rust
use phi::{Agent, DiskSessionStorage};

# async fn attach(mut agent: Agent) -> Result<(), phi::AgentError> {
agent
    .attach_session(
        "user-42",
        DiskSessionStorage::new(".phi/sessions"),
    )
    .await?;
# Ok(())
# }
```

每个同步点对应一条 JSONL 记录。第一条以及历史发生改写时使用 `replace`，正常对话增长使用 `append`，其中只包含相对上一状态新增的消息和最新 usage。加载时按行回放；如果进程在写最后一行时中断，未完成的尾行会被忽略，并在下次保存前截断，因此不会破坏此前已完成的 session。JSONL 更适合当前追加式同步和审计场景，代价是长 session 的加载需要回放多条记录；需要长期保留大量轮次时可在业务层定期归档或重建 session。

`attach_session` 会恢复已有的 `messages`、最后一次 usage 和累计 usage；session 不存在时则把当前 Agent 状态关联到这个新 ID。也可以在构建后链式恢复：

```rust
# use phi::{Agent, DiskSessionStorage, OpenAiChatProvider};
# async fn restore(provider: OpenAiChatProvider) -> Result<(), phi::AgentError> {
let mut agent = Agent::builder(provider)
    .build()
    .with_session("user-42", DiskSessionStorage::new(".phi/sessions"))
    .await?;
# Ok(())
# }
```

自动同步点严格为：

1. 用户的完整消息加入历史之后、调用 LLM 之前。
2. 每次 LLM 流结束并得到完整 `ProviderResponse`、assistant 消息加入历史之后。

流式 delta 和工具执行过程不会单独写 storage。工具结果会先保留在内存中，并随下一次完整 LLM 响应一起进入快照。保存失败会返回 `AgentError::Storage`，不会静默继续调用后续 LLM 或工具。

Session 持久化消息、usage 以及 assistant 消息携带的 opaque `ProviderState`，因此恢复后仍能无损续接推理/工具上下文。API key、model、base URL、system prompt 与工具实现不会写入 session 文件。Provider state 可能包含明文思考内容或可回放的加密块，应按敏感数据保护 session 文件。`examples/agent.rs` 可通过 `LLM_SESSION_ID` 开启磁盘 session，并用 `LLM_SESSION_DIR` 指定目录。

## 思考强度

核心层用统一的 `ReasoningEffort` 表达思考强度：

```rust
use phi::{Agent, OpenAiResponsesProvider, ReasoningEffort};

let provider = OpenAiResponsesProvider::openai("api-key", "model-name")?;
let agent = Agent::builder(provider)
    .reasoning_effort(ReasoningEffort::High)
    .max_tokens(16_384)
    .build();
# Ok::<(), phi::ProviderError>(())
```

可用枚举值为 `None`、`Minimal`、`Low`、`Medium`、`High`、`XHigh` 和 `Max`。未设置时不会向上游发送思考强度字段。Adapter 的 wire mapping 为：

- OpenAI Chat Completions：`reasoning_effort`
- OpenAI Responses：`reasoning.effort`
- Anthropic Messages：非 `None` 档位映射为 `thinking: { type: "adaptive" }` 和
  `output_config.effort`；`None` 映射为 `thinking: { type: "disabled" }`

完全未设置 `reasoning_effort` 时，Anthropic Messages 不发送 `thinking` 或
`output_config`，由上游模型采用默认行为。不同协议和模型只支持其中一部分档位，SDK
不根据 model 名称猜测能力；上游会对不支持的组合返回错误。

`examples/agent.rs` 也接受可选环境变量 `LLM_REASONING_EFFORT`，取值为小写的 `none`、`minimal`、`low`、`medium`、`high`、`xhigh` 或 `max`。

## 流式输出

`Agent::prompt()` 本身仍等待完整 agent/tool loop，但会在等待期间持续发出 `MessageUpdate`：

```rust
use phi::{AgentEvent, AssistantDelta};
use std::io::{self, Write};

agent.subscribe(|event| {
    if let AgentEvent::MessageUpdate {
        delta: AssistantDelta::Text { delta },
    } = event
    {
        print!("{delta}");
        io::stdout().flush().unwrap();
    }
});

agent.prompt("hello").await?;
# Ok::<(), phi::AgentError>(())
```

工具参数也会增量到达：

```rust
# use phi::{AgentEvent, AssistantDelta};
# let event: AgentEvent = unimplemented!();
if let AgentEvent::MessageUpdate {
    delta: AssistantDelta::ToolCall {
        index,
        id,
        name,
        arguments_delta,
    },
} = event
{
    // arguments_delta 是尚未完成的 JSON 字符串片段。
}
```

需要绕过 Agent 时，可以直接调用 `provider.stream(request)`，消费 `ProviderEvent::Delta` 和最终的 `ProviderEvent::Done`。`provider.generate(request)` 是便捷的非流式视图，内部消费完全相同的事件流，因此两条路径不会产生不同的协议实现。

## Token 与上下文统计

模型上下文上限由调用方显式提供：

```rust
# use phi::{Agent, OpenAiChatProvider};
# let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model")?;
let agent = Agent::builder(provider)
    .max_context_tokens(128_000)
    .max_tokens(4_096)
    .build();
# Ok::<(), phi::ProviderError>(())
```

一次 Agent 运行结束后：

```rust
# async fn inspect(mut agent: phi::Agent) -> Result<(), phi::AgentError> {
let result = agent.prompt("hello").await?;
println!("本次所有 API 请求：{} tokens", result.run_usage.total_tokens);
if let Some(context) = result.context_usage {
    println!("当前上下文：{} / {}", context.used_tokens, context.max_tokens);
    println!("剩余上下文：{}", context.remaining_tokens);
}
println!("Agent 累计 API 用量：{}", agent.cumulative_usage().total_tokens);
# Ok(())
# }
```

`run_usage` 是本次 Agent loop 内所有模型请求的 token 总和，适合用量/成本统计；`context_usage` 只采用最后一次模型响应的 `total_tokens`，用于衡量当前对话实际占用，二者不能混用。

## 图片输入

`content` 可以是普通文本或规范化的文本/图片内容块；每个 adapter 会转换成目标协议的图片结构：

```rust
use phi::{Agent, ImageDetail, ImageUrl};

# async fn vision(mut agent: Agent) -> Result<(), phi::AgentError> {
let image = ImageUrl::new("https://example.com/image.png")
    .with_detail(ImageDetail::High);
let result = agent
    .prompt_with_images("描述这张图片", vec![image])
    .await?;
# Ok(())
# }
```

本地图片可以编码成 data URL：

```rust
# use phi::ImageUrl;
let bytes = std::fs::read("image.png")?;
let image = ImageUrl::from_bytes("image/png", &bytes);
# Ok::<(), Box<dyn std::error::Error>>(())
```

也可以完全控制内容块顺序：

```rust
use phi::{Content, ContentPart, ImageUrl};

let content = Content::parts([
    ContentPart::text("比较下面两张图片"),
    ContentPart::image(ImageUrl::new("https://example.com/a.png")),
    ContentPart::image(ImageUrl::new("https://example.com/b.png")),
]);
# let mut agent: phi::Agent = unimplemented!();
# async {
agent.prompt_content(content).await?;
# Ok::<(), phi::AgentError>(())
# };
```

通用视觉示例同时支持本地路径、HTTP URL 和现成 data URL：

```bash
export VISION_API_KEY=your_key
export VISION_BASE_URL=https://provider.example/v1
export VISION_MODEL=vision-model
export VISION_MAX_CONTEXT_TOKENS=128000
cargo run --example vision -- ./image.png "图片里有什么？"
```

注意：SDK 会生成目标 adapter 对应的图片内容结构，但 Provider 对图片的实际支持仍取决于调用方传入的模型。

## Daemon 骨架

仓库同时包含独立的 `phi-daemon` workspace package。它目前只提供可启动、可优雅退出的 HTTP/WS transport 外壳，以及 `api`、`service`、`runtime`、`store` 分层；公开业务路由尚未启用，所有请求都会返回 `404 Not Found`。

```bash
cargo run -p phi-daemon
```

默认监听 `127.0.0.1:8787`，可以通过环境变量修改：

```bash
PHI_DAEMON_BIND=127.0.0.1:9000 cargo run -p phi-daemon
```

daemon 的 runtime 已建立每个 session 独占一个 `Agent` 的 actor/handle 边界，并为后续的 Agent factory、运行时 registry、WS event bus 和 control store 预留稳定落点。目前使用进程内 control store；Provider profile、持久化数据库以及具体 HTTP/WS 接口将在后续接入。

## 模块结构

- `agent`：状态、事件订阅及 agent/tool loop
- `provider`：中性 provider trait 与 OpenAI Chat、Responses、Anthropic adapter
- `tool`：异步工具接口与结果类型
- `types`：消息、工具调用、事件及运行结果
- `error`：Provider、工具及 Agent 错误
- `storage`：session 快照持久化接口与内存、JSONL 实现
- `mcp`：stdio 与 Streamable HTTP MCP client
- `crates/phi-daemon`：daemon 进程、transport 外壳与运行时编排边界

这是精简的 agent-core，而不是 Pi coding-agent CLI 的完整复刻；TUI、上下文压缩、运行取消和 daemon 业务接口尚未包含。
