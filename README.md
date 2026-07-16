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
- 文本、图片和文档组成的 provider-neutral 富 `content`
- 保守副作用分类、并发上限可调的工具调度，结果仍按调用顺序写回上下文
- 带协作取消、进度事件、结构化 metadata 和富内容结果的工具执行上下文
- 默认关闭、可按需启用的 `read`、`bash`、`edit`、`write` 内置工具，以及后台 Bash 查询/停止工具
- MCP client：支持 stdio 与 Streamable HTTP server 的工具发现和调用
- 默认关闭、支持多目录与渐进式正文加载的 Skills catalog/tool
- 可配置 HTTP 超时、错误重试和指数退避
- 可修改请求、响应和 turn 数据的异步生命周期 Hooks
- `agent_start`、turn、message、tool progress 和 error 生命周期事件
- 可协作停止的 run control，以及协议安全的停止检查点
- 可持久化的 Default/Plan 模式、工具能力硬限制与独立版本化计划文件
- 可显式启用的异步 subagent runtime，支持双向通知、安全边界消息注入和永久关闭
- 持续工具轮次与可复用对话历史
- 独立 `phi-daemon` 二进制：session registry、HTTP 列表、new/attach WebSocket、可重连的 `askuser` 与 Plan Exit 审批

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

需要常驻进程和多客户端会话管理时，使用 workspace 内的 daemon：

```bash
mkdir -p .phi/daemon
openssl rand -hex 32 > .phi/daemon/auth.key
chmod 600 .phi/daemon/auth.key
export PHI_DAEMON_AUTH_KEY_FILE=.phi/daemon/auth.key
DAEMON_KEY="$(cat "$PHI_DAEMON_AUTH_KEY_FILE")"
cargo run -p phi-daemon

curl -X PUT http://127.0.0.1:8787/v1/providers/default \
  -H "Authorization: Bearer $DAEMON_KEY" \
  -H 'content-type: application/json' \
  -d '{"provider":"openai_chat","api_key":"...","base_url":"https://example.com/v1","model":"model-name"}'
```

Provider 配置、HTTP/WS 协议和停止语义见
[`crates/phi-daemon/README.md`](crates/phi-daemon/README.md)。

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

已有工具只实现 `execute` 即可保持兼容。需要长任务协作时可以覆盖 `execute_with_context`，通过 `ToolExecutionContext` 检查取消、发送进度并查看当前 transcript 中仍可见的 tool result；`ToolOutput` 还可以附带 `ContentPart` 和 JSON metadata，这些数据会进入事件、session storage 与 daemon API。

## Skills

Skills 属于 library 能力且默认关闭。调用方显式提供一个或多个根目录；每个根目录只扫描 `<root>/<name>/SKILL.md`，后配置的目录可确定性覆盖前面的同名 skill。catalog 是不可变快照，tool schema 只暴露有界的名称/描述索引，完整正文仅在模型调用 `skill` tool 或调用方显式选择 skill 时进入上下文：

```rust
use phi::{Agent, SkillCatalog, SkillDirectory, SkillInvocation, SkillsConfig};

# async fn build(provider: impl phi::LlmProvider + 'static) -> Result<(), phi::SkillError> {
let config = SkillsConfig::new()
    .skill_directory(SkillDirectory::new("/home/user/.phy/skills").source("global"))
    .skill_directory(SkillDirectory::new("/workspace/.claude/skills").source("workspace"));
let catalog = SkillCatalog::load(&config).await?;
let agent = Agent::builder(provider).skills(catalog.clone()).build();

// 显式选择时由 library 展开，不依赖模型再次决定是否调用 skill。
let prompt = catalog.apply_to_prompt(
    &SkillInvocation::new("code-review"),
    "检查认证逻辑".into(),
)?;
# let _ = (agent, prompt);
# Ok(())
# }
```

支持 Claude 常用的 `description`、`when_to_use`、`argument-hint`、`arguments`、`version`、`disable-model-invocation` 和 `user-invocable` frontmatter，以及 `$ARGUMENTS`、索引/命名参数和 `${CLAUDE_SKILL_DIR}` 替换。Skills 不执行 markdown 内嵌 shell、hooks，也不把 `allowed-tools` 等 frontmatter 当作授权；这些字段只产生诊断。live catalog 不自动监听文件变化，需要重新构建 Agent 才会更新。

## Plan 模式与计划文件

`AgentMode::Plan` 是执行权限边界，不只是额外提示词。Agent 会在 lifecycle hook 之后再次过滤 Provider 可见的工具，并在真正执行 tool call 时再次校验；因此 hook 重新加入工具或模型伪造调用都不能绕过限制。模式保存在 session snapshot 中，恢复会话后仍然生效：

```rust
use phi::{Agent, AgentMode, OpenAiChatProvider};

# let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model")?;
let mut agent = Agent::builder(provider)
    .mode(AgentMode::Plan)
    .build();

// 对已 attach session 的 Agent，这个异步入口会立即持久化模式。
agent.set_mode(AgentMode::Default).await?;
# Ok::<(), phi::AgentError>(())
```

每个 `Tool` 通过 `effect()` 声明最大副作用。Plan 只允许 `ReadOnly`、`Internal` 和 `PlanOnly`；`edit`/`write`、`bash` 分别被归类为 workspace write 和 external side effect。自定义与 MCP 工具默认是 `ExternalSideEffect`，需要实现者审计后显式降权，才会在 Plan 中可用。`PlanOnly` 工具只在 Plan 中可见。

`InMemoryPlanStore` 和 `DiskPlanStore` 提供独立于 transcript 的 session-scoped Markdown 计划文件。更新使用 `expected_revision` 做乐观并发控制，revision `0` 表示计划尚不存在；磁盘实现按 session 分文件并原子替换。daemon 在此基础上注入 `read_plan`、`write_plan`、`exit_plan_mode`：退出请求绑定计划的完整 revision/content，只有客户端显式批准同一版本才会切回 Default，拒绝或计划已变化都会留在 Plan。协议示例见 [`crates/phi-daemon/README.md`](crates/phi-daemon/README.md)。

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

`AgentBuilder::all_builtin_tools(cwd)` 是启用 `read`、`bash`、`edit`、`write` 四类能力的快捷方式；启用 Bash 时还会安装共享状态的 `bash_task_output` 和 `bash_task_stop`。各工具类型也可单独注册并配置，例如 `.tool(BashTool::new(cwd).shell("/bin/zsh").timeout(Duration::from_secs(90)))`。

- `read`：流式读取普通 UTF-8 文本，支持 1-based `offset` 和 `limit`，大文件只扫描请求范围并返回继续读取提示；也能验证并返回 PNG/JPEG/GIF/WebP、PDF，以及按 cell 渲染 Jupyter notebook（包括受限的内嵌图片）。同一可见 tool result 对未变化文件的相同范围重复读取会返回轻量引用。FIFO、设备、伪装媒体和不支持的二进制文件会被拒绝。
- `bash`：在配置目录中执行 shell 命令并合并 stdout/stderr；默认超时 120 秒，调用参数中的 `timeout` 可覆盖，`BashTool::timeout`、`set_timeout` 和 `without_timeout` 可修改默认值。输出默认保留尾部 2000 行或 50KB，截断时完整内容写入临时文件。`run_in_background=true` 会立即返回 task id，随后可查询实时尾部输出或停止整个进程组；Agent/registry 释放时也会取消遗留任务。
- `edit`：一个调用可提交多个基于原文件快照的精确替换，默认要求 `oldText` 唯一，也可逐项设置 `replaceAll`；各替换不能重叠。它限制输入文件大小，保留 UTF-8 BOM 和未编辑区域的 CRLF/LF/CR 混合换行，并返回紧凑 diff 与结构化修改统计。
- `write`：创建或完全覆盖文件，并递归创建父目录。

Agent 默认最多同时执行 8 个被分类为 `Safe` 的调用，可通过 `max_parallel_tools` / `set_max_parallel_tools` 调整。只读内置工具可并行；Bash 仅对保守 allowlist 能证明为只读的命令开放并行，解析不明、后台命令和任何写入/副作用工具都会让整批调用串行。`edit` 与 `write` 还会按目标文件串行化。

Agent 还会对每个工具调用施加默认 300 秒的外层超时，可通过 Builder 的 `tool_call_timeout` / `without_tool_call_timeout`，或运行期的 `Agent::set_tool_call_timeout` 修改。外层超时和 stop 会把已开始但未确认完成的调用标记为 `unknown`；对于 Bash，取消时还会终止其进程组。

这些工具遵循 Pi 的本地工具语义：工作目录只用于解析相对路径，不是安全沙箱；绝对路径和包含 `..` 的路径仍可访问工作目录以外的位置，`bash` 也可以执行当前进程权限允许的任意命令。只应向可信 Agent 显式开放所需的最小能力。

## Subagent

Subagent 是 library 的显式 opt-in 能力。调用方提供 `SubagentFactory`，创建一个父 Agent 作用域的 `SubagentRuntime`，再选择性注册三个父工具；省略注册就是关闭，不会隐式增加模型能力：

```rust
use std::sync::Arc;
use phi::{Agent, SubagentConfig, SubagentFactory, SubagentRuntime, SubagentTools};

# fn build(parent: &mut Agent, child_factory: Arc<dyn SubagentFactory>) {
let runtime = SubagentRuntime::new("parent-session", child_factory, SubagentConfig::default());
let SubagentTools { spawn_agent, send_agent_message, close_agent } =
    SubagentTools::new(runtime.clone());
parent.add_tool(spawn_agent);
parent.add_tool(send_agent_message);
parent.add_tool(close_agent);
# }
```

`spawn_agent` 立即返回稳定 ID，child 在独立 task 中继续运行；`send_agent_message` 在 provider/tool 协议安全边界注入消息，idle child 会被唤醒。runtime 自动只给 child 注入 `notify_parent`：`progress` 可观察但不唤醒父 Agent，`blocker`/`result` 会唤醒；失败和关闭也会产生 runtime 通知。`close_agent` 可关闭 Starting、Running 或 Idle child，永久拒绝后续消息且幂等。child 默认不会得到 `spawn_agent`，因此不能递归创建下一级 Agent。

daemon 默认注册这组父工具，可用 `PHI_DAEMON_SUBAGENTS_ENABLED=false` 关闭；library 本身始终保持显式启用。

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
- 文本和 structured content 会写回模型上下文，默认限制为 2000 行或 50KB；structured content 还以有界 metadata 保留。MCP image 会作为 provider-neutral 富内容返回（超限时省略），audio 和二进制 resource 仍只返回安全摘要。
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
- `config: GenerationConfig`，包括可选的每请求 `model` override、`temperature`、`max_tokens` 和 `reasoning_effort`

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

三个内置 Provider 共用同一套重试策略。对于可以确认安全重试的失败，默认在首次请求失败后最多重试 10 次；该数字由 `RetryConfig` 管理，不写死在请求循环中。`max_retries` 表示“额外重试次数”，所以配置为 10 时最多会发送 11 次 HTTP 请求，配置为 0 时关闭重试：

```rust
use std::time::Duration;
use phi::{OpenAiChatProvider, RetryConfig};

let retry = RetryConfig::default()
    .with_max_retries(5)
    .with_request_timeout(Duration::from_secs(20))
    .with_stream_idle_timeout(Duration::from_secs(90))
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

三个内置 Provider 都支持链式 `.http_client(reqwest::Client)` 注入，也提供直接接收 Client 的 `new_with_client`（Anthropic 自定义 endpoint 使用 `with_base_url_and_client`）。需要构建多个 Agent/session 时应 clone 同一个 Client，以复用 DNS、TLS 和连接池；daemon 的默认 factory 使用直接构造入口，所有 session 共享一个 Client，且不会先创建临时 Client。

重试分类如下：

- 建立连接阶段的失败：请求尚未送达，使用带随机抖动的指数退避。
- 已建立连接后的传输错误和响应头超时：结果可能已经被服务端处理，因此立即返回错误，不自动重试。
- HTTP 408、409、425，以及除 501/505 外的 5xx：带随机抖动的指数退避，且受 `max_backoff` 限制。
- HTTP 429：优先使用 `Retry-After`（同时支持秒数和 HTTP 日期）；缺失或无效时使用固定的 `rate_limit_backoff`，不做指数增长。等待时间同样受 `max_backoff` 限制。
- 其他非 2xx 响应不会重试。常见上下文窗口超限响应会归一化为 `ProviderError::ContextLengthExceeded`，其余响应作为 `ProviderError::Api` 返回；包括 400、401、403、404 等永久性客户端错误，以及无法正常跟随的 3xx。
- 请求构造、响应解码和重定向错误不会重试。

请求超时覆盖建立连接到收到响应头的阶段。这个阶段一旦超时，客户端无法判断 POST 是否已经被接收，所以即使仍有 retry budget 也不会重发，避免重复生成、计费或工具调用。已经建立的 SSE 流另有默认 120 秒的 event idle timeout，可通过 `with_stream_idle_timeout` 修改或通过 `without_stream_idle_timeout` 关闭；idle timeout 和非正常 EOF 同样只返回错误，不重放已输出的流。OpenAI Chat 只有收到 `[DONE]` 或明确正常的 `finish_reason` 才会接受响应。`examples/agent.rs` 可以通过 `LLM_MAX_RETRIES`、`LLM_REQUEST_TIMEOUT_SECS` 和 `LLM_STREAM_IDLE_TIMEOUT_SECS` 覆盖默认配置。

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

`ProviderRetryReason` 会区分安全的连接传输错误和 HTTP 状态错误；HTTP 错误同时包含状态码与响应体。事件在退避等待之前发出。响应头超时和结果不明确的传输错误不会发出 retry 事件，而是直接进入原有的 `AgentEvent::Error`，并从 `Agent::prompt()` 返回 `AgentError::Provider`。直接使用 `provider.stream()` 时，同一个重试通知以 `ProviderEvent::Retry` 返回。

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
    async fn save_incremental(
        &self,
        session: &SessionSnapshot,
        previous_message_count: usize,
    ) -> Result<(), StorageError>;
    async fn save_replacing_from(
        &self,
        session: &SessionSnapshot,
        unchanged_message_count: usize,
    ) -> Result<(), StorageError>;
    async fn delete(&self, session_id: &str) -> Result<(), StorageError>;
}
```

两个增量方法都有回退到 `save` 的默认实现，因此已有自定义 storage 不需要改动；追加型实现可以利用消息游标避免反复加载完整快照。

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

每个同步点只向 JSONL 尾部写入一条记录。正常增长使用 `append`，只包含新增消息和最新 usage；工具 journal 或清空历史需要更新尾部时使用 `replace_tail`，只包含变化起点及新尾部；完整 `replace` 保留给通用 snapshot 保存。`DiskSessionStorage` 会缓存每个已加载 session 的文件游标，正常 Agent checkpoint 不再重新读取和重建整个 JSONL；检测到文件被外部修改或有残缺尾行时才重新校验。加载时仍按行回放，未完成的最后一行会被忽略并在下次保存前截断。

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

自动同步点为：

1. 用户的完整消息加入历史之后、调用 LLM 之前。
2. Provider 返回工具调用后、任何工具开始前，先持久化 assistant 调用和 `unknown` 结果占位。
3. 工具完成、取消或超时后，用真实结果、`cancelled` 或 `unknown` 原子更新本轮尾部。
4. 无工具的完整 assistant 响应，以及 turn-end hook 对本轮尾部的修改完成之后。

流式 delta 不写 storage。工具执行前的 journal 保存失败时不会启动工具；如果工具可能已经产生副作用但结果未能确认，恢复后会保留 `unknown` 结果且不会自动重放该调用。这样避免 transcript 回滚导致重复执行，但并不声称外部副作用具备 exactly-once 语义；支付、消息发送等工具仍应使用 `tool_call.id` 作为幂等键。保存失败会返回 `AgentError::Storage`，不会静默继续后续 LLM 或工具。

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

### 上下文压缩

`ContextCompactor` 是每个 Agent 选择一个的可替换压缩策略。library 不隐式安装策略；创建 Agent 时显式声明，owner 持有 `&mut Agent` 且 Agent 空闲时也可以通过 `set_context_compactor` 原子切换实现：

```rust
use phi::{Agent, DefaultContextCompactor};

# fn build(provider: impl phi::LlmProvider + 'static) {
let agent = Agent::builder(provider)
    .max_context_tokens(200_000)
    .max_tokens(20_000)
    .context_compactor(DefaultContextCompactor::default())
    .build();
# let _ = agent;
# }
```

`DefaultContextCompactor` 是内置的默认方案实现：使用当前 Agent 的同一模型生成纯文本摘要，禁用工具和 reasoning，summary 最多输出 20k tokens；图片和文档在摘要请求中替换为标记，opaque provider state 不参与摘要。成功前不会修改 live transcript；成功后用 `Conversation compacted` boundary 和 synthetic user summary 原子替换 active history，并通过 `save_replacing_from` 持久化。摘要请求自身超出窗口时会按协议安全的完整消息组从头裁剪，最多重试 3 次。

自动压缩不是固定百分比。默认实现采用固定余量公式：

```text
max_context_tokens - min(max_output_tokens, 20_000) - 13_000
```

Agent 在每次真正发送下一条 LLM 请求前，以最近 Provider usage 加上此后新增消息的估算量做判断。因此普通最终回答不会刚生成就立刻被摘要；tool loop 的下一轮和下一个用户 prompt 会先压缩再请求模型。连续 3 次自动压缩失败后，该 Agent 会停止自动重试；手动压缩和上下文超限恢复仍可执行。

调用方可主动执行 `agent.compact_context(Some("额外摘要要求".into())).await`。压缩会发出 `ContextCompactionStarted`（含实际 summary prompt）、`ContextCompactionCompleted`（含 history replace-tail patch）或 `ContextCompactionFailed`。Provider 在尚未输出有效 assistant delta 时返回上下文超限后，Agent 会触发所选 compactor；只在 transcript 实际改变时重试原请求一次，第二次超限直接返回，避免恢复循环。压缩后 `last_usage/context_usage` 清空，压缩 API usage 只计入累计/本次计费用量，下一次正常响应会重新建立占用统计。

## 图片与文档输入

`content` 可以是普通文本或规范化的文本、图片、文档内容块；每个 adapter 会转换成目标协议支持的原生结构：

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

文档同样可以从本地字节构造；例如 `ContentPart::document(Document::from_bytes("report.pdf", "application/pdf", &bytes))`。工具返回的图片和文档也使用同一套内容块，adapter 会在保持 tool-call 协议顺序的前提下映射或安全降级。

通用视觉示例同时支持本地路径、HTTP URL 和现成 data URL：

```bash
export VISION_API_KEY=your_key
export VISION_BASE_URL=https://provider.example/v1
export VISION_MODEL=vision-model
export VISION_MAX_CONTEXT_TOKENS=128000
cargo run --example vision -- ./image.png "图片里有什么？"
```

注意：SDK 会生成目标 adapter 对应的图片内容结构，但 Provider 对图片的实际支持仍取决于调用方传入的模型。

## Daemon

仓库同时包含独立的 `phi-daemon` workspace package。它在进程内维护
`session_id -> Agent actor` 映射，并提供 session 列表、延迟创建和恢复会话的
HTTP/WebSocket 接口：

- `GET /v1/providers`：列出 daemon 的命名 Provider profiles（密钥不回显）。
- `GET/PUT /v1/providers/{profile_id}`：查询或原子更新指定 Provider profile。
- `GET /v1/sessions`：列出已经持久化的 session。
- `GET /v1/sessions/{session_id}`：查询单个 session 的当前模型与状态。
- `POST /v1/auth/token`：使用长期 bearer key 换取 60 秒有效、单次使用的 WebSocket subprotocol token。
- `GET /v1/ws/new?profile_id=...`：用指定 profile 构建 Agent，首个 prompt 才初始化并持久化 session。
- `GET /v1/ws/attach/{session_id}`：返回历史与当前快照，并持续订阅流式事件；可发送 `compact` 主动触发默认上下文压缩。
- `GET /v1/ws/attach/{session_id}/subagents/{agent_id}`：只读观察 child Agent；任何 text/binary 输入都以 `1008` 拒绝。

同一 session 可以被多个 WebSocket 同时 attach；运行期间的新 prompt 会进入有界
FIFO，stop、状态和模型配置变化会同步广播。每个 session 由单独 actor 串行拥有
`Agent`，metadata 与 transcript 默认持久化到磁盘。daemon 创建的 Agent 会自动获得
`askuser`、subagent 工具以及三个 Plan 工具；父 Agent 创建 child 时会向 session
调用方广播结构化事件，child 的流式过程可通过独立只读 WebSocket 观察。问题和 Exit
审批通过广播发送，任一 attach 客户端可处理，断线重连可从 snapshot 的 pending 状态恢复。

```bash
PHI_DAEMON_AUTH_KEY_FILE=.phi/daemon/auth.key cargo run -p phi-daemon
```

daemon 要求 key 文件至少包含 32 个可打印非空白 ASCII 字节，建议权限为 `0600`。所有 HTTP API 通过 `Authorization: Bearer <key>` 鉴权；WebSocket 客户端同时 offer `phi.v1` 与 `phi.auth.<temporary-token>`，服务端只选择固定的 `phi.v1`。默认监听 `127.0.0.1:8787`，可以通过环境变量修改：

```bash
PHI_DAEMON_AUTH_KEY_FILE=.phi/daemon/auth.key \
  PHI_DAEMON_BIND=127.0.0.1:9000 \
  PHI_DAEMON_SUBAGENTS_ENABLED=true \
  cargo run -p phi-daemon
```

完整 Provider/启动配置、wire protocol、事件 DTO、排队与停止 checkpoint 语义见
[`crates/phi-daemon/README.md`](crates/phi-daemon/README.md)。daemon 自带的 factory
按 `profile_id` 读取由 HTTP 管理并持久化的 Provider 配置；若需要 MCP 或 workspace
read/edit/write/bash 等工具，可提供自定义 `AgentFactory`。

## 模块结构

- `agent`：状态、事件订阅及 agent/tool loop
- `provider`：中性 provider trait 与 OpenAI Chat、Responses、Anthropic adapter
- `tool`：异步工具接口与结果类型
- `types`：消息、工具调用、事件及运行结果
- `error`：Provider、工具及 Agent 错误
- `storage`：session 快照持久化接口与内存、JSONL 实现
- `mcp`：stdio 与 Streamable HTTP MCP client
- `crates/phi-daemon`：daemon 进程、HTTP/WS transport、session actor 与持久化编排

这是精简的 agent-core 与单进程 daemon，而不是 Pi coding-agent CLI 的完整复刻；
TUI、partial/micro compaction、TLS/origin 校验、多租户和分布式 actor 协调尚未包含。
