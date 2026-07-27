# phi-daemon

`phi-daemon` 是 `phi::Agent` 的单进程宿主。它通过 HTTP 管理配置和持久化资源，通过
WebSocket 创建、恢复和操纵 session，并把流式事件广播给所有 attach 的客户端。

daemon 负责 transport、鉴权、session actor、Provider/Agent/MCP Profile、定时任务、
Telegram 输出目标和磁盘编排；Agent loop、Provider 协议、工具协议和 transcript 规则仍由
根 crate `phi` 实现。

## 快速启动

在仓库根目录运行：

```bash
cargo run -p phi-daemon
```

默认监听 `127.0.0.1:8787`，数据写入当前目录下的 `.phi/daemon`。首次启动会生成
`$HOME/.phi/daemon/auth.key`；Unix 上目录和 key 文件权限分别为 `0700` 和 `0600`。

启动 session 前至少配置一个 Provider Profile：

```bash
DAEMON_KEY="$(cat "$HOME/.phi/daemon/auth.key")"

curl -X PUT http://127.0.0.1:8787/v1/providers/default \
  -H "Authorization: Bearer $DAEMON_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "provider": "openai_responses",
    "api_key": "...",
    "base_url": "https://provider.example/v1",
    "model": "model-name",
    "max_context_tokens": 128000
  }'
```

`provider` 支持 `openai_chat`、`openai_responses` 和 `anthropic`。
`provider`、`api_key`、`base_url`、`model`、正数 `max_context_tokens` 均为必填项。
可选项包括 `max_output_tokens`、`temperature`、`reasoning_effort`、重试次数和两个超时。
兼容字段 `system_prompt` 会被忽略；行为提示词应放入 Agent Profile。

交互式终端默认显示 App 连接二维码，其中包含连接地址和长期 key。局域网直连使用
`--lan`，关闭二维码使用 `--no-qr`：

```bash
cargo run -p phi-daemon -- --lan
```

## 运行模型

```text
HTTP / WebSocket clients
          |
          v
       Axum API
          |
          v
  ApplicationService
      /        \
     v          v
profiles     AgentRegistry
                 |
                 v
        one SessionActor
                 |
                 v
             phi::Agent
          /       |       \
     Provider    Tools    SessionStorage
```

- 一个 live session 只有一个 actor，actor 串行拥有一个 `phi::Agent`。同一 session
  同时最多运行一个 turn，后续 prompt 进入有界 FIFO。
- `/v1/ws/new` 先构建 prepared Agent。首个有效 prompt 之前不创建 metadata、transcript
  或 registry entry，断开连接不会留下 session。
- 首 prompt 激活、metadata 创建、storage attach 和 prompt 入队是一个受控流程。
  daemon 重启后的首次 attach 会从磁盘单飞恢复 actor。
- 多个 attach 共享快照和有序 live event。sequence 只属于当前进程中的 actor；
  重连依赖最新 snapshot，广播 lag 通过完整 resync 恢复，不提供 durable event replay。
- `askuser` 和工具审批由 session actor 持有，socket 断开不会取消。定时任务 session
  不安装这两类交互能力，超出 capability 的工具直接 fail closed。
- daemon 为交互 session 显式安装 `read`、`edit`、`write`、`bash`、后台 Bash、
  `askuser`、工具审批、默认 context compactor，并按配置安装 skills 和父 subagent 工具。
  这些都不是 `phi` library 的隐式默认能力。
- child observer 严格只读。child 默认不能继续创建 child；`general` child 可从 durable
  sidechain transcript 继续，`explore` 和 `plan` 是只读 one-shot。

## 配置与安全

| 变量 | 默认值 | 作用 |
| --- | --- | --- |
| `PHI_DAEMON_BIND` | `127.0.0.1:8787` | HTTP(S)/WS(S) 监听地址 |
| `PHI_DAEMON_PUBLIC_URL` | 未设置 | 只覆盖终端和二维码中的公开 base URL |
| `PHI_DAEMON_DATA_DIR` | `.phi/daemon` | daemon 数据目录 |
| `PHI_DAEMON_AUTH_KEY_FILE` | `$HOME/.phi/daemon/auth.key` | 长期 key 文件；显式路径必须已存在 |
| `PHI_DAEMON_TLS_CERT_FILE` | 未设置 | PEM 证书链，必须与私钥同时设置 |
| `PHI_DAEMON_TLS_KEY_FILE` | 未设置 | 未加密 PEM 私钥，必须与证书同时设置 |
| `PHI_DAEMON_WORKSPACE_DIR` | 启动目录 | 新 session 的默认 workspace |
| `PHI_DAEMON_SKILLS_ENABLED` | `true` | 是否为 daemon session 启用 skills |
| `PHI_DAEMON_SUBAGENTS_ENABLED` | `true` | 是否启用父 subagent 工具和 observer |
| `PHI_DAEMON_SESSION_TITLE_PROFILE_ID` | 未设置 | 可选的独立标题生成 Provider Profile |
| `PHI_DAEMON_GLOBAL_SKILLS_DIRS` | `~/.phi/skills` | 全局 skill 根目录 path-list |
| `PHI_DAEMON_WORKSPACE_SKILLS_DIRS` | `.phy/skills`、`.claude/skills` | 相对 session workspace 的 skill 根目录 path-list |
| `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY`、`NO_PROXY` | 未设置 | Provider 和标题请求的出站代理；兼容小写变量 |
| `RUST_LOG` | `phi_daemon=info` | tracing filter |

两个 `*_DIRS` 使用操作系统原生 path-list 分隔符，空值会关闭对应目录组。skill catalog
是 session 创建时的快照；文件变化只影响之后创建或重启恢复的 Agent。

安全边界：

- HTTP 使用 `Authorization: Bearer <long-term-key>`。WebSocket 只接受由长期 key 换取的
  60 秒、单次使用 token，token 通过 `Sec-WebSocket-Protocol` 提交，不进入 URL。
- 长期 key 只从 key 文件读取。Provider API key、Telegram token、MCP bearer/header/env
  secret 不会通过 GET、普通 Debug 或公开 session history 回显。
- `PHI_DAEMON_PUBLIC_URL` 不改变监听地址、不启用 TLS，也不启动 tunnel。
- 同时配置 TLS 证书和私钥后，同一 listener 改为 HTTPS/WSS，不再开放明文端口。
- daemon 没有 origin 校验、租户隔离或 OS sandbox。绑定非 loopback 地址时必须使用可信
  前置代理或等效安全边界；二维码和长期 key 都应视为可执行工作区命令的高权限凭据。
- 5xx 与 `mcp_connection_failed` / `Provider(_)` 类错误对外只回显稳定的 generic message
  （如 `"daemon operation failed"`），原始错误（磁盘路径、临时文件名、上游响应体、URL、
  MCP 命令行）通过 `tracing` 记录到服务端日志，不会进入 HTTP 响应体或 WebSocket 事件。
  客户端应始终依据 `code` 字段分支，不要解析 `message` 文本。
- `read_only`、`workspace_edit`、`full_access` 是工具 effect 与 workspace path 的应用层
  限制。它们不提供进程隔离、系统调用过滤或 network namespace。

## Profile 与能力

| 类型 | 内容 | 更新影响 |
| --- | --- | --- |
| Provider Profile | adapter、credential、base URL、模型、生成参数、上下文预算 | 只影响之后创建或重启恢复的 Agent，不热替换 live/prepared Agent |
| Agent Profile | prompt、工具/skill policy、MCP ID、初始 capability、model/reasoning override | prepared 时解析，首 prompt 激活时把完整 resolved profile 和 revision pin 到 session |
| MCP Profile | stdio 或 Streamable HTTP 连接、credential、工具前缀和输出限制 | Agent Profile 只 pin ID；新 Agent、重启恢复和定时任务下次执行读取最新连接配置 |
| Bot Account | Telegram token | 可被多个收件目标复用，公开响应只显示 token 是否已配置 |
| Output Channel | Bot Account ID 与 chat ID | 可被定时任务引用，通知失败不改变 Agent run 结果 |

模型配置优先级为 session override、Agent Profile、Provider Profile。改变 Provider adapter
后应新建 session，因为 opaque `provider_state` 与原 adapter 绑定。

Agent Profile 的 `prompt.mode` 可为 `extend` 或 `full`；daemon 始终追加不可删除的 harness
与 workspace 信息。工具权限取 Agent Profile 名称策略、当前 capability 和工具 effect 的
交集。MCP 工具保守归类为外部副作用，在较窄 capability 下需要交互审批；定时任务通常需要
显式选择 `full_access` 才能调用。

定时任务支持每日计划（本地时间、工作日、IANA 时区）和分钟/小时/天间隔。每次执行创建
一个独立、可 attach 的 session；同一任务不重叠，daemon 停机后最多补一次。任务定义使用
revision 做乐观并发控制。开始和终态可发送到 Telegram Output Channel，但消息发送失败只
记录脱敏日志。通知通过 Telegram `sendRichMessage` 发送 Rich Markdown，标题、加粗、列表
及 GFM 表格会原生渲染；若 Telegram 以 `400`/`404` 明确拒绝 Rich Markdown（例如语法不完整
或自托管 Bot API 版本过旧），daemon 会用 `sendMessage` 回退为纯文本。

## HTTP API

除 WebSocket upgrade 和内嵌 Web 静态资源外，所有 `/v1` HTTP 接口都要求长期 bearer key。
错误响应统一为：

```json
{"code":"stable_error_code","message":"human-readable message"}
```

`code` 是稳定标识，客户端应基于它分支。`message` 仅用于人类阅读：4xx 的校验类错误会
回显字段级原因，但任何可能携带内部数据（磁盘路径、临时文件名、provider 响应体、URL、
命令行）的错误一律被替换为 generic 文本，原始细节只写入服务端 `tracing` 日志。

| 方法与路径 | 作用 |
| --- | --- |
| `POST /v1/auth/token` | 换取 60 秒、单次使用的 WebSocket token |
| `GET /v1/workspaces/browse?path=...` | 浏览可读取绝对目录的直接子目录；省略 path 时使用默认 workspace |
| `GET /v1/providers` | 列出脱敏 Provider Profiles |
| `GET/PUT /v1/providers/{profile_id}` | 读取或完整替换一个 Provider Profile |
| `GET/PUT /v1/provider` | `default` Provider Profile 的兼容别名 |
| `GET /v1/agent-profiles` | 列出 Agent Profiles；内建 `default@0` 始终可用 |
| `GET/PUT /v1/agent-profiles/{agent_profile_id}` | 读取或完整替换一个 Agent Profile |
| `GET /v1/mcp-profiles` | 列出脱敏 MCP Profiles |
| `GET/PUT /v1/mcp-profiles/{mcp_profile_id}` | 读取或完整替换一个 MCP Profile |
| `GET /v1/bot-accounts` | 列出脱敏 Telegram Bot Accounts |
| `GET/PUT /v1/bot-accounts/{bot_account_id}` | 读取或完整替换一个 Bot Account |
| `GET /v1/output-channels` | 列出 Telegram 收件目标 |
| `GET/PUT /v1/output-channels/{output_channel_id}` | 读取或完整替换一个收件目标 |
| `POST /v1/output-channels/{output_channel_id}/test` | 发送测试消息，成功返回 `204` |
| `GET/POST /v1/scheduled-tasks` | 列出或创建定时任务 |
| `GET/PUT/PATCH/DELETE /v1/scheduled-tasks/{task_id}` | 查询、完整编辑、启停或删除任务 |
| `POST /v1/scheduled-tasks/{task_id}/run` | 立即异步执行一次，成功接纳返回 `202` |
| `GET /v1/sessions` | 返回按置顶/时间排序的扁平 sessions 和 workspace 树 |
| `GET/PATCH/DELETE /v1/sessions/{session_id}` | 查询、修改 `pinned` 或删除 session |
| `POST /v1/sessions/{session_id}/fork` | 从 public assistant 消息之后或 `before_tool_calls` 检查点分叉 |
| `GET /v1/sessions/{session_id}/skills` | 返回 session 的 skill 摘要和诊断，不返回正文或本地路径 |

`GET /v1/sessions` 只列出已经激活并保留 metadata 的 session。未恢复进当前进程的 session
状态为 `offline`，其 `message_count` 为 `null`。当前没有 HTTP 空白 session create、
prompt、stop、取消 queued run 或清空队列接口。

Provider、Agent、MCP、Bot Account 和 Output Channel 的 PUT 都是完整替换；包含 secret 的
对象更新时必须重新提交仍需保留的 secret。精确请求和响应 schema 见
[`src/api/dto.rs`](src/api/dto.rs)。

## WebSocket API

客户端先用长期 key 调用 `POST /v1/auth/token`，再同时提供两个 subprotocol：

```text
phi.v1
phi.auth.<temporary-token>
```

服务端只选择并回显 `phi.v1`。token 过期、使用或重放都会返回 `401`。

| 路径 | 作用 |
| --- | --- |
| `GET /v1/ws/new?profile_id=...&agent_profile_id=...&capability_mode=...&workspace=...` | 构建 prepared session；query 均可选 |
| `GET /v1/ws/attach/{session_id}` | 恢复/订阅已有 session |
| `GET /v1/ws/attach/{parent_session_id}/subagents/{agent_id}` | 只读观察 child Agent |

`/new` 的正常顺序是 `building`、`ready`、首个 `prompt`、`session_created`，之后原 socket
直接成为该 session 的 attach 连接。`/attach` 首帧是完整 `snapshot`，随后接收 live
`event`。child observer 首帧是 `subagent_snapshot`；任何应用层 text 或 binary 输入都会以
close code `1008` 拒绝。

最小 prompt：

```json
{
  "type": "prompt",
  "request_id": "prompt-1",
  "content": {"type": "text", "value": "检查当前仓库"}
}
```

每个客户端命令都带 `request_id`：

| `type` | 关键字段 | 准入语义 |
| --- | --- | --- |
| `prompt` | `content`，可选 `skill` | prepared 时激活、idle 时启动，running/stopping 时进入 FIFO |
| `stop` | 当前 `run_id` | 只停止 active run，不清空队列 |
| `compact` | 可选 `instructions` | 只在 idle 执行 |
| `set_model` | 非空 `model` | 只在 prepared/idle 执行 |
| `set_reasoning_effort` | `effort` 或 `null` | 只在 prepared/idle 执行 |
| `set_capability_mode` | `read_only`、`workspace_edit`、`full_access` | 只在 prepared/idle 执行 |
| `answer_askuser` | `ask_id`、完整 `answers` | 任一 attach 可回答 pending request |
| `decide_tool_permission` | `permission_id`、`decision` | 可单次允许、记住服务端候选规则或拒绝 |
| `ping` | 无 | 返回同 request ID 的 `pong` |

`command_accepted` 和 `command_rejected` 只发给命令来源 socket。`event`、run 状态、Agent
流式消息、工具、压缩、ask、审批和 subagent 状态按 sequence 广播给全部 attach。
run 的终态只能以 `run_completed`、`run_stopped` 或 `run_failed` 判断。

慢客户端落后于广播环时会收到 `resync_required`；客户端必须丢弃派生状态，以其中完整
`session` 和 `last_sequence` 继续。child observer 使用独立 sequence 和
`subagent_resync_required`。

snapshot 是客户端状态的唯一完整来源，包含 history、draft、active/queued run、配置、
capability、usage、压缩边界、pending ask/approval 和 subagent 摘要。内部 mailbox/压缩消息
只投影为 `visibility: "internal"` 占位，不公开正文；opaque `provider_state` 永不进入 wire。
`reasoning` 只包含 Provider 返回的规范化可显示文本。

daemon 为每个 Agent 安装独立的默认 compactor。自动压缩、手动 `compact` 和一次性的
context-length overflow 恢复走同一策略；started/completed/failed 事件会广播，但 summary
prompt、summary 正文和 replacement patch 不进入 WebSocket。public display history 会保留
被压缩前的消息和压缩分隔状态。

父 WebSocket 只从 UTF-8 JSON text 读取应用层命令，binary frame 会被忽略。单条
message/frame 上限为 1 MiB，单次服务端写超时为 10 秒。顶层 frame、snapshot、event
和错误字段的精确 schema 以以下类型为准：

- daemon：[`src/api/dto.rs`](src/api/dto.rs)
- Web：[`../../web/src/types/wire.ts`](../../web/src/types/wire.ts)
- Flutter：[`../../flutter/lib/core/models/wire.dart`](../../flutter/lib/core/models/wire.dart)

## 停止与工具协议

stop 是 cooperative cancellation，不是副作用事务回滚：

- 用户消息先持久化；未完成的 assistant streaming draft 不进入 transcript，停止时丢弃。
- assistant tool-call batch 会在执行任何工具前，与一一对应的 `unknown` journal result
  一起持久化。journal 保存失败时不得执行工具。
- 成功结果替换 journal tail；未启动、取消、超时、panic 或结果不确定的调用保留明确的
  cancelled/unknown result，恢复后不会自动重放。
- tool、hook 或 Provider future 被取消，不代表外部文件、进程、网络或远端副作用已回滚。
  非幂等工具应使用 `tool_call.id` 作为业务幂等键。
- stop 完成后 active run 以 `run_stopped` 终结，已经排队的下一个 prompt 继续执行。

因此 daemon 保证的是可恢复、Provider 协议完整的 durable checkpoint，不是 exactly-once。

## 持久化与恢复

```text
.phi/daemon/
├── provider.json
├── agent-profiles.json
├── mcp-profiles.json
├── output-channels.json
├── scheduled-tasks.json
├── control/
│   └── session-<uuid>.json
├── sessions/
│   └── session-<encoded-id>.jsonl
└── subagent-worktrees/
    └── <parent-session-id>/<agent-id>/
```

- profile、output channel 和 scheduled task 文件在 Unix 上以 `0600` 创建。它们可能包含
  API key、token、MCP secret 或任务 prompt，不应公开分发。
- `control/` 保存 session metadata、workspace、标题、置顶状态、Provider ID、pinned
  Agent Profile、模型/reasoning 和 revision。
- `sessions/` 使用 append/replace-tail 为主的 JSONL 保存 normalized transcript、usage、
  capability、工具审批规则和 opaque Provider replay state。
- `subagent-worktrees/` 保存 detached worktree。clean checkout 在 child finalization 时
  移除；有修改、commit 或状态无法安全判断时保留并通过事件报告位置。
- Provider Profile 更新不影响 live/prepared Agent。Agent Profile 激活后完整 pin；
  MCP 只 pin ID，因此重启恢复时读取该 ID 的最新 endpoint 和 credential。
- 首 prompt 入队后会异步生成标题；失败不影响主 run。配置
  `PHI_DAEMON_SESSION_TITLE_PROFILE_ID` 时使用独立 Provider Profile，否则复用当前
  session 的 profile/model。
- 删除 session 会先关闭 live actor，再删除 metadata 和 transcript。fork 会创建新的
  offline session，不复制 streaming draft、置顶状态或 session 工具审批规则。

## 内嵌 Web 客户端

daemon 在同一地址提供 Web 客户端和 API。`/v1` 保持纯 API 语义；其他 GET/HEAD 路径
提供静态资源，未知前端路由回退到 `index.html`。

发布前先构建 Web，再构建 daemon：

```bash
cd web
pnpm install
pnpm build
cd ..
cargo build --release -p phi-daemon
```

release 构建把 `web/dist` 嵌入二进制；debug 构建从磁盘读取。没有 `web/dist` 时
`build.rs` 会生成占位页面，因此仍可编译 daemon。

## 当前边界

- 单进程 actor registry，没有跨实例 live session、分布式锁或事件总线。
- 没有 durable event cursor/replay；恢复依赖 snapshot 和之后的 live sequence。
- 没有 WebSocket origin 校验、多租户、细粒度用户授权或 OS sandbox。
- 没有 HTTP prompt/stop、queued-run cancel、队列清空或空白 session create。
- 没有工具外部副作用的事务回滚或强制中断。
