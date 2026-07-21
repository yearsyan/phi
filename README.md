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
- 默认关闭、可按需启用的 `read`、`bash`、`edit`、`write` 内置工具，以及后台 Bash 落盘输出、完成通知和兼容查询/停止工具
- MCP client：支持 stdio 与 Streamable HTTP server 的工具发现和调用
- 默认关闭、支持多目录与渐进式正文加载的 Skills catalog/tool
- 可配置 HTTP 超时、错误重试和指数退避
- 可修改请求、响应和 turn 数据的异步生命周期 Hooks
- `agent_start`、turn、message、tool progress 和 error 生命周期事件
- 可协作停止的 run control，以及协议安全的停止检查点
- 可持久化的 `ReadOnly` / `WorkspaceEdit` / `FullAccess` capability 边界和工具名称策略
- 可显式启用的 foreground/background subagent runtime，支持 `general` / `explore` /
  `plan` 类型、sidechain transcript 恢复、模型与 reasoning override、输出契约、
  host-owned workspace isolation、双向通知和永久关闭
- 持续工具轮次与可复用对话历史
- 独立 `phi-daemon` 二进制：版本化 Agent Profile、session registry、HTTP 列表、
  new/attach WebSocket、可重连的 `askuser` 与持久化定时任务调度
- React/Vite Web 客户端与 Flutter 客户端；Flutter 支持 Android、iOS、macOS 和 HarmonyOS/OpenHarmony

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
cargo run -p phi-daemon
```

未设置 `PHI_DAEMON_AUTH_KEY_FILE` 时，首次启动会安全生成
`$HOME/.phi/daemon/auth.key`。在另一个终端读取该 key 后即可调用 API：

```bash
DAEMON_KEY="$(cat "$HOME/.phi/daemon/auth.key")"

curl -X PUT http://127.0.0.1:8787/v1/providers/default \
  -H "Authorization: Bearer $DAEMON_KEY" \
  -H 'content-type: application/json' \
  -d '{"provider":"openai_chat","api_key":"...","base_url":"https://example.com/v1","model":"model-name"}'
```

daemon 在交互式终端中默认显示包含连接地址和长期 key 的 App 连接二维码；请像 key 文件
一样保护它。同一局域网中的手机直连使用 `cargo run -p phi-daemon -- --lan`，daemon 会
监听全部 IPv4 接口并优先把 `192.168/10/172.16–31` 私网地址写入二维码；找不到私网地址
时选择一个非 loopback 的本机 IPv4，仍不可用才回退 `127.0.0.1`。这会扩大网络暴露面。
可用 `--no-qr` 关闭二维码，非终端 stderr 会自动跳过。

Provider 配置、HTTP/WS 协议和停止语义见
[`crates/phi-daemon/README.md`](crates/phi-daemon/README.md)。

GitHub 上的每次 push 都会构建 Windows x86_64 和 macOS ARM64 版本的
`phi-daemon`，并将压缩包上传到对应 Actions run 的 Artifacts；该流程不创建
GitHub Release。macOS 产物在仓库配置了下列全部 Actions secrets 时使用 Developer ID
Application 签名并提交 Apple notarization。完全未配置时只生成名称带 `-unsigned` 的
ad-hoc 签名产物，供可信的内部测试使用。
只配置一部分 secrets 会让构建失败，避免误发布未完成签名的产物。

- `MACOS_CERTIFICATE_P12_BASE64`：包含 Developer ID Application 证书与私钥的
  PKCS#12 文件的 base64 内容。
- `MACOS_CERTIFICATE_PASSWORD`：该 PKCS#12 文件的密码。
- `APPLE_API_KEY_P8_BASE64`：用于 Notary API 的 App Store Connect `.p8` 私钥的
  base64 内容。
- `APPLE_API_KEY_ID`：App Store Connect API key ID。
- `APPLE_API_ISSUER_ID`：App Store Connect issuer ID。

`phi-daemon` 是命令行 daemon，不是可以从 Finder 双击启动的 `.app`。解压后应在
Terminal 中启动；首次启动会自动创建长期鉴权 key：

```bash
chmod +x ./phi-daemon
./phi-daemon
```

正式分发应使用已签名且已 notarize、名称不带 `-unsigned` 的产物。只有在确认来源可信、
需要测试未签名 Actions 产物时，才可在接收机器上移除浏览器添加的 quarantine 标记后按
上述方式运行：

```bash
xattr -d com.apple.quarantine ./phi-daemon
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

## Capability 模式与工具策略

`CapabilityMode` 会在 Provider 请求、hook 修改之后和工具真正执行前重复校验。最终
权限是 capability mode、`ToolPolicy` 和工具自身 `ToolEffect` 的交集：

- `ReadOnly`：只允许只读和 Agent 内部协调工具。
- `WorkspaceEdit`：在 `ReadOnly` 基础上允许 `WorkspaceWrite`，但不允许 shell、网络等
  `ExternalSideEffect`。
- `FullAccess`：不额外屏蔽 effect。

`ToolPolicy` 可用精确工具名配置 allow-list 和 deny-list；deny 对普通工具优先。daemon
的 `askuser` 和关闭 child 等 harness 工具可以标记为 mandatory，从而绕过名称筛选，
但仍然受 capability mode 限制。capability 存入 session snapshot；持久化失败时运行时
只会保留更窄的能力，不会意外升权。

capability 切换约束之后的工具暴露/执行，并会收紧或关闭权限过宽的 child；它不会回滚
已经完成的外部副作用。先前已获准启动的 background Bash 也不是 OS sandbox 中的进程，
需要通过 `bash_task_stop` 或 session shutdown 显式终止。

这是应用层 capability sandbox，不是操作系统沙盒。`ReadOnly`/`WorkspaceEdit` 下，
内置 `read`、`edit`、`write` 会 canonicalize 路径并拒绝绝对路径、`..` 或 symlink
逃逸到 session workspace 之外。唯一的只读例外是同时启用 `read` 与 `bash` 时由 harness
创建的私有后台任务输出目录；该例外不扩展到任意临时目录，也不允许 symlink 逃逸。
这些约束不提供进程隔离、系统调用过滤、网络 namespace 或对自定义/MCP 工具实现的自动
审计。`FullAccess` 仍保留原有“workspace 作为默认工作目录”的语义，`bash` 也按 daemon
进程本身的操作系统权限执行。

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

内置工具默认全部关闭，不会因为构建 `Agent` 而自动获得文件系统或 shell 权限。library
使用 `Workspace` 表示 Agent/session 绑定的工作目录；相对根路径在创建 `Workspace`
时解析为绝对路径：

```rust
use phi::{Agent, BuiltinTools, OpenAiChatProvider, Workspace};

# let provider = OpenAiChatProvider::new("key", "https://example.com/v1", "model")?;
let workspace = Workspace::new("/workspace/project");
let agent = Agent::builder(provider)
    .workspace(workspace.clone())
    .builtin_tools(BuiltinTools::all_in(workspace))
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

`AgentBuilder::all_builtin_tools(cwd)` 是兼容的快捷方式：它同时绑定 session workspace
并启用 `read`、`bash`、`edit`、`write` 四类能力；启用 Bash 时还会安装共享状态的
`bash_task_output` 和 `bash_task_stop`。若 Builder 同时显式设置 `workspace`，所有
`BuiltinTools` 都会重定向到该 workspace，与调用顺序无关。各工具类型也可单独注册并
配置，例如 `.tool(BashTool::new(cwd).shell("/bin/zsh").timeout(Duration::from_secs(90)))`。

- `read`：只读取一个普通文件，不枚举目录；目录检查和文件发现应改用专用目录/搜索工具，或在 Bash 可用时执行 `ls`、`find`、`rg --files`。它流式读取普通 UTF-8 文本，支持 1-based `offset` 和 `limit`，大文件只扫描请求范围并返回继续读取提示；也能验证并返回 PNG/JPEG/GIF/WebP、PDF，以及按 cell 渲染 Jupyter notebook（包括受限的内嵌图片）。同一可见 tool result 对未变化文件的相同范围重复读取会返回轻量引用。FIFO、设备、伪装媒体和不支持的二进制文件会被拒绝。同时启用 Bash 时，它还能读取该工具返回的 harness-owned `output_file`，即使 capability 已限制为 `ReadOnly`/`WorkspaceEdit`。
- `bash`：在配置目录中执行 shell 命令并合并 stdout/stderr；默认超时 120 秒，调用参数中的 `timeout` 可覆盖，`BashTool::timeout`、`set_timeout` 和 `without_timeout` 可修改默认值。输出默认保留尾部 2000 行或 50KB，完整输出文件最多 5 GiB。`run_in_background=true` 会立即返回 `task_id` 与从启动时就持续写入的 `output_file`；安装 Agent mailbox 的 host 会在终态收到 internal `<task_notification>`，模型应等待通知后用 `read` 读取文件，而不是轮询。`bash_task_output` 仅作为已弃用的兼容路径保留，支持默认阻塞、`block=false` 和最长 600 秒的 `timeout`；`bash_task_stop` 停止整个进程组。Agent/registry 释放时也会取消遗留任务并清理其输出文件。
- `edit`：一个调用可提交多个基于原文件快照的精确替换，默认要求 `oldText` 唯一，也可逐项设置 `replaceAll`；各替换不能重叠。它限制输入文件大小，保留 UTF-8 BOM 和未编辑区域的 CRLF/LF/CR 混合换行，并返回紧凑 diff 与结构化修改统计。
- `write`：创建或完全覆盖文件，并递归创建父目录。

后台完成通知使用通用 `ToolExecutionContext::notify_agent` 通道。嵌入 library 的 host 若要
启用它，需要给 Agent 安装有界 `AgentMailbox`，并在 `AgentMailboxSender::wait_for_wake`
就绪时调用 `Agent::prompt_from_mailbox`；活动 run 会在下一处完整协议边界接收通知，不会被
异步打断。`phi-daemon` 已默认完成这层监督，不依赖是否启用 subagent。

Agent 默认最多同时执行 10 个被分类为 `Safe` 的调用，可通过 `max_parallel_tools` / `set_max_parallel_tools` 调整。只读内置工具和显式声明为 `Safe` 的内部协调工具可并行；Bash 仅对保守 allowlist 能证明为只读的命令开放并行，解析不明、后台命令和任何写入/副作用工具都会让整批调用串行。`edit` 与 `write` 还会按目标文件串行化。

Agent 还会对每个工具调用施加默认 300 秒的外层超时，可通过 Builder 的 `tool_call_timeout` / `without_tool_call_timeout`，或运行期的 `Agent::set_tool_call_timeout` 修改。外层超时和 stop 会把已开始但未确认完成的调用标记为 `unknown`；对于 Bash，取消时还会终止其进程组。

在 `FullAccess` 下，这些工具遵循原有本地工具语义：workspace 主要作为相对路径和 shell
的默认工作目录，绝对路径仍可能访问其外，`bash` 也可以执行当前进程权限允许的命令。
`ReadOnly`/`WorkspaceEdit` 会额外限制内置文件工具的 canonical path，但仍不是 OS
sandbox。只应向可信 Agent 显式开放完成任务所需的最小能力。

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

`spawn_agent` 采用普通 Agent tool 语义：默认以前台方式等待 child，并把最终结果
直接作为本次 tool result 返回；`run_in_background=true` 才会立即返回稳定 ID，child 在
独立 task 中继续运行并在结束时自动通知父 Agent。该工具显式标记为 concurrency-safe，
因此同一个 assistant tool-call batch 中的多个 delegation 可在默认 10 路上限内并行。
配置化入口还可选择：

- `general`、`explore` 或 `plan`；`explore`/`plan` 是不可继续的 one-shot 角色，capability
  上限固定为 `ReadOnly`，请求不能把它们升宽。
- 更窄的 capability、model/reasoning override。
- 文本输出，或带必需顶层字段的 JSON 输出契约；最终响应不满足契约时 child 以失败结束。
- `shared` workspace，或由 host 明确实现的 `worktree` isolation；library 不执行 Git
  操作，也不会在 host 不支持时静默退回 shared。

每次 child task 结束后 runtime 会释放 live `Agent`、Provider 和工具 registry，已完成记录
不再占用 `max_agents`（默认 10）的并发配额。`general` child 的 sidechain transcript 仍保留；
`send_agent_message` 会先重建 child、attach 原 transcript，再把新消息放入 provider/tool
协议安全边界。`explore`/`plan` 拒绝继续。使用 `with_storage` 可把 sidechain 接到调用方的
durable `SessionStorage`；`restore_from_history` 只恢复父 transcript 中已有的 child ID，不会
重放原始 prompt 或不确定的工具副作用。新记录同时固化 parent scope，fork 出来的 transcript
不会接管源 session 的 sidechain；缺少该字段的旧记录仍按兼容路径恢复。

runtime 自动只给 child 注入 `notify_parent`：`progress` 可观察但不唤醒父 Agent；后台 child
的 `blocker`/`result` 会唤醒，前台 child 的结果由原 tool result 返回，因此不会制造重复父
turn。后台终态若发生在父 Agent 正在运行时，会进入父 mailbox 并在当前 turn 的下一处完整
工具协议边界被消费；父 Agent 已 idle 时才另起一轮。若当前父 run 在消费前 stop/fail，actor
会对仍待处理的 mailbox 消息重新做一次有界 admission，不会等到下一次人工 prompt。失败、
输出校验和资源 finalization 也会产生结构化事件。
用于唤醒父 Agent 的运行时输入以 `MessageVisibility::Internal` 保留在 provider-safe
transcript 中，模型仍会收到它，但应用可以将其排除在用户对话 UI 之外；普通消息默认
为 `Public`，因此旧 transcript 仍可直接读取。
`close_agent` 可关闭 Starting、Running 或 Idle/resumable child，永久拒绝后续消息且幂等。child
默认不会得到 `spawn_agent`，因此不能递归创建下一级 Agent。

daemon 默认注册这组父工具，可用 `PHI_DAEMON_SUBAGENTS_ENABLED=false` 关闭，并为
`worktree` isolation 提供 detached Git worktree。clean worktree 在 child 关闭时移除；
存在 tracked/untracked 修改、HEAD 已产生新 commit，或无法安全检查状态时会保留目录
并在 finalization 事件中返回位置，不会用强制删除丢弃 child 的工作。library 本身始终
保持显式启用。

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

Adapter 会把可显示的思考文本规范化到 assistant `Message.reasoning`，并通过
`AssistantDelta::Reasoning` 实时发布；同时完整思考数据仍作为 opaque `ProviderState`
随消息保留，并在后续请求中按原协议回放：

- Chat Completions：`reasoning_content`、`reasoning` 与完整 `reasoning_details`
- Responses：原始 output Items，包括 `type: "reasoning"` Items
- Anthropic Messages：原始 `thinking`/`redacted_thinking` content blocks，包括 `signature`

工具续轮与普通下一轮都会保留这些数据。业务代码复制或重建 `Message` 时也应保留
`provider_state`；其 `Debug` 输出只展示类型和数量，不打印思考正文。规范化 reasoning
只包含文本，不包含 Anthropic signature、redacted thinking、Responses encrypted
content 或其他协议回放字段。

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

三个内置 Provider 都支持链式 `.http_client(reqwest::Client)` 注入，也提供直接接收 Client 的 `new_with_client`（Anthropic 自定义 endpoint 使用 `with_base_url_and_client`）。需要构建多个 Agent/session 时应 clone 同一个 Client，以复用 DNS、TLS 和连接池；standalone daemon 会在启动时读取 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 与 `NO_PROXY`（同时接受小写别名），让普通/child Agent 和标题生成共享同一个 Client。协议专用代理优先于 `ALL_PROXY`，详细配置见 [`crates/phi-daemon/README.md`](crates/phi-daemon/README.md#环境变量)。

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

两个增量方法都有回退到 `save` 的默认实现，因此已有自定义 storage 不需要改动；追加型实现可以利用消息游标避免反复加载完整快照。`SessionSnapshot::messages` 是下一次 Provider 请求使用的 active transcript，`SessionSnapshot::history` 则是压缩后仍供宿主回看的完整会话投影；后者绝不会作为模型输入。

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

每个同步点只向 JSONL 尾部写入一条记录。正常增长使用 `append`，只包含新增消息和最新 usage；工具 journal 或清空历史需要更新尾部时使用 `replace_tail`，只包含变化起点及新尾部；完整 `replace` 保留给通用 snapshot 保存。`DiskSessionStorage` 会缓存每个已加载 session 的文件游标，正常 Agent checkpoint 不再重新读取和重建整个 JSONL；检测到文件被外部修改或有残缺尾行时才重新校验。加载时仍按行回放，未完成的最后一行会被忽略并在下次保存前截断。回放同时从压缩前的旧记录重建 `history` 与全部压缩边界，因此升级前已经压缩过的 append-only session 也能恢复可见历史，只要原 JSONL 记录仍在。

`attach_session` 会恢复已有的 active `messages`、完整 `history`、最后一次 usage、累计 usage、执行模式和
workspace；session 不存在时则把当前 Agent 状态关联到这个新 ID。workspace 一旦写入
session snapshot 就不可重绑定：使用不同 workspace 构建的 Agent 恢复该 session 会
返回 `StorageError::WorkspaceMismatch`。旧 snapshot 没有 workspace 字段时可由首次
绑定的 Agent 补写迁移。也可以在构建后链式恢复：

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

Session 持久化消息、消息的 `visibility`、usage 以及 assistant 消息携带的 opaque
`ProviderState`，因此恢复后仍能无损续接推理/工具上下文。`visibility=internal` 只控制
应用展示，不会从 Provider 请求中删除该消息；旧记录缺少该字段时按 `public` 读取。
API key、model、base URL、system prompt 与工具实现不会写入 session 文件。Provider
state 可能包含明文思考内容或可回放的加密块，应按敏感数据保护 session 文件。
`examples/agent.rs` 可通过 `LLM_SESSION_ID` 开启磁盘 session，并用 `LLM_SESSION_DIR`
指定目录。

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

`run_usage` 是本次 Agent loop 内所有模型请求的 Provider accounting 总和，适合用量/成本统计；`context_usage` 则使用最后一次正常模型响应的规范化 `input_tokens + output_tokens` 衡量窗口占用。Provider 返回的 `total_tokens` 可能包含供应商特有的计费单位，因此不会反向覆盖上下文容量；二者不能混用。

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

`DefaultContextCompactor` 是内置的默认方案实现：使用当前 Agent 的同一模型生成纯文本摘要，禁用工具和 reasoning，summary 最多输出 20k tokens；图片和文档在摘要请求中替换为标记，规范化 reasoning 与 opaque provider state 都不参与摘要。成功前不会修改 live transcript；成功后用标记为 `internal` 的 `Conversation compacted` boundary 和 synthetic user summary 原子替换 active transcript，并通过 `save_replacing_from` 持久化。被替换的消息继续保留在独立的 `SessionHistory` 中，供重新 attach 或重启后的 UI 回看，但不会重新进入 Provider 上下文；两条内部压缩消息仍会交给 Provider 续接上下文，daemon public history 不暴露其正文。摘要请求自身超出窗口时会按协议安全的完整消息组从头裁剪，最多重试 3 次。

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
- `GET /v1/agent-profiles`、`GET/PUT /v1/agent-profiles/{agent_profile_id}`：管理
  prompt、工具/skill 筛选、初始 capability 和生成 override；revision 按 profile 独立递增。
- `GET /v1/sessions`：列出已经持久化的 session；同时返回向后兼容的有序
  `sessions` 和由 daemon 按工作区投影的 `workspaces` 树。
- `GET /v1/sessions/{session_id}`：查询单个 session 的当前模型与状态。
- `PATCH /v1/sessions/{session_id}`：持久化设置或取消会话置顶。
- `DELETE /v1/sessions/{session_id}`：关闭 live actor，并删除会话 metadata 与 transcript。
- `POST /v1/sessions/{session_id}/fork`：从指定 public assistant 消息之后，或从其
  `before_tool_calls` 持久化检查点，克隆一个新的离线 session；运行中的工具阶段也可用。
- `GET/POST /v1/scheduled-tasks`：列出或创建持久化的每日/间隔定时任务。
- `GET/PATCH/DELETE /v1/scheduled-tasks/{task_id}`：查询、暂停/恢复或删除任务；
  `POST .../{task_id}/run` 可立即执行一次。
- `GET /v1/workspaces/browse?path=...`：从 daemon 默认 workspace 或指定绝对路径浏览
  可读取子目录，供新 session 选择工作目录。
- `POST /v1/auth/token`：使用长期 bearer key 换取 60 秒有效、单次使用的 WebSocket subprotocol token。
- `GET /v1/ws/new?profile_id=...&agent_profile_id=...&capability_mode=...&workspace=...`：选择
  Provider、Agent Profile、可选 capability override 和工作目录；首 prompt 才初始化并持久化。
- `GET /v1/ws/attach/{session_id}`：返回历史与当前快照，并持续订阅流式事件；可发送 `compact` 主动触发默认上下文压缩。
- `GET /v1/ws/attach/{session_id}/subagents/{agent_id}`：只读观察 child Agent；任何 text/binary 输入都以 `1008` 拒绝。

同一 session 可以被多个 WebSocket 同时 attach；运行期间的新 prompt 会进入有界
FIFO，stop、状态、模型和 capability 变化会同步广播。每个 session 由单独 actor 串行拥有
`Agent`，metadata 与 transcript 默认持久化到磁盘。daemon 创建的 Agent 会自动获得
`askuser` 和 subagent 工具；父 Agent 创建 child 时会向 session 调用方广播结构化事件，
child 的流式过程可通过独立只读 WebSocket 观察。问题通过广播发送，任一 attach 客户端
可回答，断线重连可从 snapshot 的 pending 状态恢复。

定时任务由 daemon 进程调度，每次执行都创建独立 session，并把任务名作为
session 标题。同一任务不重叠执行；下一次计划在开始 Agent 前持久化，避免进程
崩溃后自动重放可能已发生的外部副作用。daemon 必须持续运行才能准时触发。

首个 prompt 入队后，daemon 会异步生成并持久化 session 标题，再向所有 attach 客户端
广播 `title_changed`。可通过 `PHI_DAEMON_SESSION_TITLE_PROFILE_ID` 指定独立的
Provider profile；未设置时复用当前 session 的 profile，并采用该 session 的有效
model。标题请求始终禁用 reasoning，并且不阻塞主 run；生成后会在主 run 仍进行时立即
持久化和广播。失败时 session 继续保持可用且标题为 `null`。

```bash
cargo run -p phi-daemon
```

未设置 `PHI_DAEMON_AUTH_KEY_FILE` 时，daemon 会复用或首次生成
`$HOME/.phi/daemon/auth.key`；显式设置该变量仍可读取其他已有 key 文件。key 至少包含 32 个
可打印非空白 ASCII 字节，自动创建的目录和文件在 Unix 上分别限制为 `0700` 和 `0600`。
所有 HTTP API 通过 `Authorization: Bearer <key>` 鉴权；WebSocket 客户端同时 offer
`phi.v1` 与 `phi.auth.<temporary-token>`，服务端只选择固定的 `phi.v1`。默认监听
`127.0.0.1:8787`，可以通过环境变量修改。默认 transport 是 HTTP/WS；同时配置
`PHI_DAEMON_TLS_CERT_FILE` 和 `PHI_DAEMON_TLS_KEY_FILE` 后，同一监听地址改为 HTTPS/WSS，
两个变量只设置一个会拒绝启动：

```bash
PHI_DAEMON_BIND=127.0.0.1:9000 \
  PHI_DAEMON_SUBAGENTS_ENABLED=true \
  cargo run -p phi-daemon
```

通过 Cloudflare Tunnel、Tailscale Funnel 或反向代理发布 loopback daemon 时，可设置
`PHI_DAEMON_PUBLIC_URL=https://phi.example.com`。它只覆盖终端和 App 连接二维码中的
公开 `base_url`，不改变 `PHI_DAEMON_BIND`、daemon TLS 或代理生命周期；因此代理仍可连接
`http://127.0.0.1:8787`，客户端则使用公开 HTTPS/WSS 地址。公开 URL 必须是无 credentials、
query 和 fragment 的绝对 HTTP(S) URL。

### Web client

`web/` 提供 React/Vite 客户端。开发服务器会把 `/v1` HTTP 与 WebSocket 请求代理到
默认的 `http://127.0.0.1:8787` daemon。可用 `PHI_WEB_DAEMON_PROXY_TARGET` 覆盖目标；
连接使用自签名证书的 TLS daemon 时，应通过 `NODE_EXTRA_CA_CERTS` 显式信任证书：

```bash
cd web
pnpm install
pnpm dev

# TLS daemon 示例；路径相对于 web/。
NODE_EXTRA_CA_CERTS=../.phi/daemon/tls/localhost.crt \
  PHI_WEB_DAEMON_PROXY_TARGET=https://localhost:8787 \
  pnpm dev
```

首次使用需在设置中保存 daemon 长期 key 和至少一个 Provider profile。设置页会列出
daemon 中的全部命名 Provider profiles，可在同一界面新增配置、切换编辑并选择新对话的
默认 profile；daemon 已保存的 API key 不会返回浏览器。已有本地配置时，页面会自动连接
一个 prepared session；它仍然只有在首个 prompt 后才创建并持久化 session。
客户端在 `session_created` 后继续复用原 WebSocket，不会在首条消息开始时强制重连；
连接中断后，已激活 session 会通过 attach 自动退避恢复。prompt 使用 `request_id`
维护本地发送/接纳/排队状态，可连续加入 daemon FIFO；压缩状态、usage、AskUser
多选和多 turn 工具轨迹都会实时投影。同一次 assistant 响应包含多个工具调用时，前端会
默认把它们归纳为一行动作摘要；展开后仍使用逐工具行，每行可再次展开参数与输出，单个
工具调用保持原有展示。压缩期间时间线显示不确定进度，完成后保留完整可见
对话并插入“上下文已压缩”分隔线；重新 attach 或 daemon 重启后也会从 durable history
恢复压缩前内容和全部分隔线，摘要 prompt、结果和 transcript replacement 不进入
浏览器。输入框底栏提供 capability、Provider/model、reasoning 与上下文容量明细；
Provider 列表来自 `GET /v1/providers`。prepared 对话发送首个 prompt 前，选择不同 profile
会保留当前 workspace 并重建 `/new` 连接；session 激活后，列表中的 profile 只作为模型
预设，通过 `set_model` 修改下一次用户请求使用的 model，并保留输入草稿。已激活 session
继续使用创建时固定的 Provider adapter、base URL 和凭据，不会热替换连接或混用协议特有
历史；如需切换完整 Provider 连接，应新建对话。输入 `/` 会显示 `/compact` 和当前 session 中允许
用户显式调用的 skills，skill 正文仍只在 daemon/library 展开后进入模型上下文。界面
同时提供独立 Stop/Queue 操作、自动生成的会话标题、默认折叠的流式思考块、响应式会话
导航和移动端布局。侧栏直接消费 daemon 返回的工作区树，并以默认展开的分支展示；
工作区顺序取该组在“置顶优先、最新优先”会话序列中的首次出现位置，组内保持该序列
顺序，前端不再自行聚类或排序。右键会话仍可置顶、取消置顶或确认后删除。新会话的
工作区按钮先展开限制高度、内部滚动的最近工作区列表；选择“添加工作区”后再打开独立
的目录浏览弹窗。用户消息支持复制；assistant 文本下方提供复制和“从此回复分叉”操作；当工具调用 journal
已经持久化时，运行中的回复也会显示“从这批工具调用前分叉”，且不会把未完成 draft 或
工具结果带入新 session。分叉成功后直接切换到新 session。时间线尾部会额外保留 composer
之外的安全区，避免最后一行操作贴住浮动输入框。
侧栏的“定时任务”页面可创建每日（IANA 时区与工作日）或间隔调度，并展示
运行中/已暂停分组、下次时间与最近结果；可暂停、恢复、立即运行、删除或打开最近
执行产生的 session。

### Flutter client

`flutter/` 提供与 Web 客户端并列的 Flutter 应用，复用 daemon 的 REST、单次 WS token、
prepared session、attach/resync 和 sequence gap 语义。应用包含 session/chat、workspace、
scheduled task、askuser、模型与 capability 控制，当前平台工程覆盖 Android、iOS、macOS 与
HarmonyOS/OpenHarmony。Provider 配置仍由 Web 客户端或 HTTP API 管理。GitHub 每次 push
还会使用 Actions secrets 中的专用 Android release key 构建仅含 ARM64 ABI 的包、验签并上传
`phi-client-android-release.apk`；签名变量和本地构建方式见
[`flutter/README.md`](flutter/README.md)。

```bash
cd flutter
flutter pub get
flutter test

# macOS
flutter run -d macos

# iOS 无签名 Release 验证；安装或归档需要在 Xcode 中配置签名
flutter build ios --release --no-codesign

# Android 设备访问宿主机 daemon
adb reverse tcp:8787 tcp:8787
flutter run -d <android-device-id>
```

应用支持配置多个 daemon 机器连接（名称、URL、长期 key、自签名开关），可在 Settings →
Machines 管理，并在会话页标题栏一键切换活跃机器；开发时也可用
`PHI_DAEMON_URL`/`PHI_DAEMON_KEY` dart define 注入首台机器。Flutter-OH 版本、DevEco 签名和
HAP 构建步骤见 [`flutter/README.md`](flutter/README.md)。

完整 Provider/Agent Profile、wire protocol、事件 DTO、排队与停止 checkpoint 语义见
[`crates/phi-daemon/README.md`](crates/phi-daemon/README.md)。daemon factory 按
`profile_id` 读取 Provider 配置，并按 `agent_profile_id` 编译 `extend`/`full` prompt、
工具/skill policy、初始 capability 与可选 model/reasoning override。首 prompt 激活时
会把完整 resolved Agent Profile 与 revision pin 到 session metadata；之后 profile
更新只影响新 session，重启恢复仍使用原 pin。

standalone daemon 默认以 `PHI_DAEMON_WORKSPACE_DIR` 为根目录安装 `read`、`edit`、
`write`、`bash` 及后台 bash task 工具。该目录是新 session 的默认 `Workspace`；
激活时会同时写入 daemon metadata 与 library session snapshot，child 的 shared
workspace 继承该目录，worktree isolation 则使用独立 detached checkout。daemon key
可能授予工作区读写和命令执行权限，应只提供给可信客户端；capability mode 是应用层
边界，不替代 OS sandbox。

## 模块结构

- `agent`：状态、事件订阅及 agent/tool loop
- `provider`：中性 provider trait 与 OpenAI Chat、Responses、Anthropic adapter
- `tool`：异步工具接口与结果类型
- `types`：消息、工具调用、事件及运行结果
- `error`：Provider、工具及 Agent 错误
- `storage`：session 快照持久化接口与内存、JSONL 实现
- `mcp`：stdio 与 Streamable HTTP MCP client
- `crates/phi-daemon`：daemon 进程、HTTP/WS transport、session actor 与持久化编排
- `web`：React/Vite daemon 客户端
- `flutter`：Android、iOS、macOS、HarmonyOS/OpenHarmony Flutter daemon 客户端

这是精简的 agent-core 与单进程 daemon，而不是 Pi coding-agent CLI 的完整复刻；
TUI、partial/micro compaction、WebSocket origin 校验、多租户和分布式 actor 协调尚未包含。
