# phi-daemon

`phi-daemon` 把 `phi::Agent` 包装成一个常驻进程：进程内维护 `session_id -> Agent actor` 映射，通过 HTTP 列出已经激活的 session，通过 WebSocket 创建、恢复、操纵 session，并把 Agent 的流式事件广播给所有 attach 的客户端。

当前实现的重点是 session 生命周期、排队、广播、停止和磁盘恢复。命名 Provider profile 通过 HTTP 配置并持久化，session 以 `profile_id` 选择其中一个；daemon 不读取 `LLM_*` 环境变量。每个 daemon 创建的 Agent 都会自动获得交互式 `askuser`、`spawn_agent`/`send_agent_message`/`close_agent`，以及 `read_plan`、`write_plan`、`exit_plan_mode` 三个 Plan Mode 工具。Skills 默认从全局与工作目录加载；其他内置工具和 MCP 尚未通过 daemon 配置接入，需要这些能力时应提供自定义 `AgentFactory`。

## 架构

```mermaid
flowchart LR
    Client["HTTP / WebSocket clients"] --> Axum["Axum transport"]
    Axum --> Service["ApplicationService"]
    Service --> Registry["AgentRegistry<br/>session_id -> AgentHandle"]
    Service --> Factory["AgentFactory"]
    Service --> ProviderProfiles["provider.json<br/>profile array + credentials"]
    ProviderProfiles --> Factory
    Factory --> Provider["phi Provider adapter"]
    Registry --> Actor["one actor per live session"]
    Actor --> Agent["phi::Agent"]
    Actor --> Hub["snapshot + ordered broadcast events"]
    Hub --> Axum
    Service --> Metadata["control/*.json<br/>session metadata"]
    Agent --> Transcript["sessions/*.jsonl<br/>conversation snapshots"]
    Agent --> Plans["plans/*.md<br/>versioned plan artifacts"]
```

关键边界如下：

- 一个 live session 只有一个 actor。actor 串行拥有 `Agent`，因此同一 session 不会并发执行两个 turn。
- `AgentRegistry` 只保存本进程已经激活或 attach 恢复的 session。磁盘上存在、但本进程尚未 attach 的 session 在 HTTP 列表中显示为 `offline`。
- 每个 actor 同时维护一个最新快照和一个有序广播环。多个 WebSocket attach 到同一 actor，会收到相同顺序的 live event。
- Agent 调用 `askuser` 时，actor 保持当前 run 为 `running`，把问题放进快照并广播；任一 attach 客户端都可回答，回答后原 tool future 恢复执行。
- Agent 调用 `exit_plan_mode` 时，actor 同样保持 run，发布带 revision 的不可变计划审批请求。只有批准仍是当前 revision 的计划才会切回 Default；拒绝、过期、stop 或保存失败都保持/恢复 Plan。
- Agent 调用 `spawn_agent` 时，child 在独立 runtime 中异步运行；创建事件先广播给父 session 调用方。父模型可在协议安全边界发送后续消息或永久关闭 child；child 的 blocker/result/failed/closed 通知会排队唤醒父 Agent，progress 只广播、不唤醒。
- `ApplicationService` 负责首个 prompt 的延迟激活、持久化 metadata、进程重启后的单飞恢复，以及 registry 生命周期。
- `phi::SessionStorage` 保存完整 transcript 和 Provider 回放状态；WebSocket 的 public history 刻意不暴露 opaque `provider_state`。

## 启动

启动 daemon 不需要 Provider 环境变量，但必须通过文件提供 daemon 长期鉴权 key。建议生成至少 32 字节的随机 key，并让文件只对 daemon 用户可读：

```bash
mkdir -p .phi/daemon
openssl rand -hex 32 > .phi/daemon/auth.key
chmod 600 .phi/daemon/auth.key
export PHI_DAEMON_AUTH_KEY_FILE=.phi/daemon/auth.key
cargo run -p phi-daemon
```

默认监听 `127.0.0.1:8787`，默认数据目录是相对于启动工作目录的 `.phi/daemon`。
启动后通过 `PUT /v1/providers/{profile_id}` 写入一个或多个 Provider profile；配置成功前 session 列表仍可使用，但选择未配置 profile 的 `/v1/ws/new` 会返回 `agent_build_failed`。

### 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `PHI_DAEMON_BIND` | `127.0.0.1:8787` | HTTP/WS 监听地址 |
| `PHI_DAEMON_DATA_DIR` | `.phi/daemon` | metadata 与 transcript 根目录 |
| `PHI_DAEMON_AUTH_KEY_FILE` | 无，必须设置 | 只包含长期 bearer key 的文件；key 长度为 32–4096 字节，建议文件权限 `0600` |
| `PHI_DAEMON_SKILLS_ENABLED` | `true` | 是否为 daemon session 启用 skills；library 默认仍为关闭 |
| `PHI_DAEMON_SUBAGENTS_ENABLED` | `true` | 是否注入父 Agent 的 subagent 工具并开放只读 child observer；library 仍需显式注册工具 |
| `PHI_DAEMON_WORKSPACE_DIR` | daemon 启动工作目录 | 所有 session 共用的工作目录根路径 |
| `PHI_DAEMON_GLOBAL_SKILLS_DIRS` | `~/.phy/skills` | 全局 skill 根目录列表，可配置多个 |
| `PHI_DAEMON_WORKSPACE_SKILLS_DIRS` | `.phy/skills`、`.claude/skills` | 相对工作目录的 skill 根目录列表，可配置多个 |
| `RUST_LOG` | `phi_daemon=info` | tracing filter |

两个 `*_DIRS` 变量使用操作系统原生 path-list 格式（Unix/macOS 用 `:`，Windows 用 `;`），空值表示关闭该组目录。每个根目录只扫描直接子目录中的 `<name>/SKILL.md`；按“全局目录在前、工作目录在后”的顺序合并，后扫描到的同名 skill 覆盖先前版本。live session 使用创建时的不可变 catalog 快照；修改文件只影响之后创建或进程重启后恢复的 session。

长期 key 不接受 URL 参数或明文环境变量，只从 `PHI_DAEMON_AUTH_KEY_FILE` 指向的文件加载。HTTP API 要求 `Authorization: Bearer <key>`。WebSocket 不直接携带长期 key：客户端先调用 `POST /v1/auth/token` 换取 60 秒有效、单次使用的临时 token，再通过 `Sec-WebSocket-Protocol` 提交。key 和 token 都不会写入应用日志、URL 或 Debug 输出。

daemon 目前仍不提供 TLS、origin 校验或租户隔离。默认 loopback 监听是有意的；若绑定非本机地址，仍应使用可信前置代理补充 TLS、origin 校验、授权和访问控制。代理不应记录 `Sec-WebSocket-Protocol` 的完整请求值，因为其中包含短期凭证。

## 持久化与恢复

数据目录包含四类文件：

```text
.phi/daemon/
├── provider.json
├── control/
│   └── session-<uuid>.json
├── plans/
│   └── v1/
│       └── <session-id UTF-8 lowercase hex，最多 64 字符一层>/
│           ├── plan.md
│           └── plan.lock
└── sessions/
    └── session-<base64url-session-id>.jsonl
```

- `provider.json` 是 Provider profile 数组，保存每个 profile 的 ID、API key、base URL、默认模型、生成参数和独立 revision；Unix 上以 `0600` 创建。HTTP GET 不返回 API key。旧版本的单对象格式会被读取为 `default` profile，并在下一次写入时自动迁移为数组。
- `control` 保存 `session_id`、`profile_id`、该 session 的模型、reasoning effort 和配置 revision。每个已激活 session 一个 JSON 文件。
- `sessions` 使用 append-only JSONL 保存 conversation snapshot 的 `append`、`replace_tail` 和兼容性 `replace` 记录，包括完整消息、usage 和 Provider 回放状态。live Agent 通过持久化游标直接追加本轮 delta，不会在每次 checkpoint 前重放完整日志。
- `plans` 每个 session 一个独立 Markdown 文件。路径先把 session ID 的 UTF-8 字节编码为 lowercase hex，再按最多 64 个字符分层，避免路径穿越、大小写文件系统别名和单段文件名上限；同目录 `plan.lock` 用于跨进程 CAS 串行化。`plan.md` 首行保存内部 revision，工具读取的正文不包含该元数据。写入使用 CAS revision 和原子替换，审批绑定到一个精确 revision。
- `/new` 连接只完成内存中的 Agent 构建时，不创建任何文件，也不会出现在 session 列表中。只有该连接收到首个有效 `prompt`，metadata 创建、storage attach 和 prompt 入队才作为一次激活流程发生。
- 首个 prompt 前断开连接会销毁 prepared Agent，不留下 session。
- daemon 重启后，首次 `/attach/{session_id}` 会从 metadata 重建 Agent、从 JSONL 恢复 transcript 并注册 live actor；同一 session 的并发首次 attach 是单飞的。
- model/reasoning 变更在 session 激活后会先更新 metadata，再更新内存配置；首个 prompt 前的变更会在激活时写入 metadata。
- Provider profile 更新影响之后选择该 profile 的 `/new` 构建，或进程重启后恢复的 Agent。已经 live/prepared 的 Agent 不热替换 adapter；它们的模型仍可通过 WS 独立修改。

`provider.json` 的顶层结构如下；该文件包含明文 API key，不能作为公开配置分发：

```json
[
  {
    "profile_id": "default",
    "provider": "openai_responses",
    "api_key": "...",
    "base_url": "https://provider.example/v1",
    "model": "model-name",
    "revision": 1
  },
  {
    "profile_id": "anthropic-main",
    "provider": "anthropic",
    "api_key": "...",
    "base_url": "https://api.anthropic.com",
    "model": "model-name",
    "revision": 1
  }
]
```

`GET /v1/sessions` 不会为了统计离线 session 而加载全部 transcript。因此 `offline` session 的 `message_count` 为 `null`，不代表磁盘历史为空。

## HTTP API

除 WebSocket upgrade 和不存在的 fallback 路径外，所有 `/v1` HTTP 接口都要求长期 key：

```text
Authorization: Bearer <daemon-auth-key>
```

缺少、错误、重复或格式不正确的 Authorization header 均返回 `401`，响应不会回显提交的 key。

### `POST /v1/auth/token`

使用长期 key 换取 WebSocket 临时 token：

```bash
DAEMON_KEY="$(cat "$PHI_DAEMON_AUTH_KEY_FILE")"
curl -X POST http://127.0.0.1:8787/v1/auth/token \
  -H "Authorization: Bearer $DAEMON_KEY"
```

```json
{
  "token": "url-safe-random-token",
  "token_type": "websocket_subprotocol",
  "protocol": "phi.v1",
  "expires_in_secs": 60
}
```

token 由操作系统密码学随机源生成，只能用于一次 WebSocket upgrade 尝试；使用、过期或重放均返回 `401`。响应带有 `Cache-Control: no-store`。客户端必须同时提供固定应用协议 `phi.v1` 和凭证协议 `phi.auth.<token>`；服务端只会选择并回显 `phi.v1`，不会在握手响应中回显凭证协议。

### `GET /v1/providers`

列出所有 Provider profile。响应中的 `providers` 是数组，每项包含 `profile_id`、adapter、base URL、默认模型、生成参数和 revision，并以 `api_key_configured` 表示密钥是否存在；响应中永远没有 `api_key` 字段：

```json
{
  "providers": [
    {
      "profile_id": "openai-main",
      "provider": "openai_chat",
      "api_key_configured": true,
      "base_url": "https://provider.example/v1",
      "model": "model-name",
      "system_prompt": null,
      "max_output_tokens": null,
      "max_context_tokens": 128000,
      "temperature": null,
      "reasoning_effort": null,
      "max_retries": 10,
      "request_timeout_secs": 30,
      "stream_idle_timeout_secs": 120,
      "revision": 1
    }
  ]
}
```

### `GET /v1/providers/{profile_id}`

读取单个 profile。未配置时返回 `{"configured":false,"provider":null}`；已配置时返回 `configured=true` 和公开配置。

### `PUT /v1/providers/{profile_id}`

创建或完整替换一个 profile。revision 按 profile 独立计算：每个 profile 首次成功为 `1`，之后更新该 profile 时递增。

```bash
curl -X PUT http://127.0.0.1:8787/v1/providers/openai-main \
  -H "Authorization: Bearer $DAEMON_KEY" \
  -H 'content-type: application/json' \
  -d '{
    "provider": "openai_chat",
    "api_key": "...",
    "base_url": "https://provider.example/v1",
    "model": "model-name",
    "system_prompt": "You are a helpful assistant.",
    "max_output_tokens": 4096,
    "max_context_tokens": 128000,
    "temperature": 0.2,
    "reasoning_effort": "medium",
    "max_retries": 10,
    "request_timeout_secs": 30,
    "stream_idle_timeout_secs": 120
  }'
```

`provider` 支持 `openai_chat`、`openai_responses`、`anthropic`。`provider`、`api_key`、`base_url`、`model` 和 `max_context_tokens` 必填；`max_context_tokens` 必须是正整数，用于上下文占用统计，并作为后续压缩和精简策略的预算上限。默认 `max_retries=10`、`request_timeout_secs=30`、`stream_idle_timeout_secs=120`，其余可选字段可省略或为 `null`。连接响应头超时和 SSE 完整事件间空闲超时都必须大于零。`request_timeout_secs` 命中后请求会直接失败，不会自动重发，以免已经被 Provider 接收的 POST 重复计费。该接口只做本地格式和 Provider URL 校验，不会发送探测请求。daemon factory 为所有 session 构建的 Provider 复用同一个 HTTP client 和连接池。

daemon factory 会为每个新 Agent 显式创建一份 `DefaultContextCompactor`。嵌入 daemon library 的调用方可通过 `ConfiguredAgentFactory::context_compactor` 或 `context_compactor_factory` 替换默认策略。

旧 `provider.json` 中缺少 `max_context_tokens` 或将其设为 `null` 的 profile 必须先补成正整数，否则新版本会拒绝加载该配置文件。

`GET /v1/provider` 和 `PUT /v1/provider` 保留为 `default` profile 的兼容别名。

API key/base URL 轮换可继续恢复引用该 profile 的原 session。若改变 profile 的 `provider` adapter 类型，建议新建 session，因为历史 assistant message 中的 opaque `provider_state` 与原 adapter 绑定，跨协议无法保证无损回放。

### `GET /v1/sessions`

返回所有已经激活并保留 metadata 的 session；prepared 但尚未收到首个 prompt 的 `/new` 连接不在其中。

```bash
curl http://127.0.0.1:8787/v1/sessions \
  -H "Authorization: Bearer $DAEMON_KEY"
```

示例响应：

```json
{
  "sessions": [
    {
      "session_id": "019c0000-0000-7000-8000-000000000001",
      "profile_id": "default",
      "status": "idle",
      "active_run_id": null,
      "queued_runs": 0,
      "config": {
        "model": "model-name",
        "reasoning_effort": "medium",
        "revision": 0
      },
      "mode": "default",
      "message_count": 2
    }
  ]
}
```

`status` 可能是 `awaiting_first_prompt`、`idle`、`compacting`、`running`、`stopping`、`closing`、`closed` 或 `offline`；正常已持久化 session 通常是前述状态中的 `idle`/`compacting`/`running`/`stopping`，而尚未恢复进本进程的是 `offline`。live session 的 `mode` 是 `default` 或 `plan`；离线 summary 不加载 transcript，因此为 `null`。

### `GET /v1/sessions/{session_id}`

返回单个 session 的 summary，结构与列表中的元素相同。当前模型位于 `config.model`；该值包含最近一次成功的 WS `set_model`，离线 session 也可从 metadata 查询。

### `GET /v1/sessions/{session_id}/skills`

返回该 session 支持的 skill 摘要，包括名称、描述、参数提示、来源以及 model/user 是否可调用。响应不会返回 `SKILL.md` 正文或本地绝对路径；正文只在 skill 实际调用时进入模型上下文。查询 live session 返回其固定快照；查询 offline session 会构建当前目录下下一次 attach 将使用的 catalog，但不会把该 session 注册成 live actor。

```json
{
  "session_id": "019c0000-0000-7000-8000-000000000001",
  "skills": [
    {
      "name": "code-review",
      "description": "Review code for correctness and security",
      "model_invocable": true,
      "user_invocable": true,
      "source": "workspace"
    }
  ]
}
```

当前没有 HTTP session create、delete、prompt 或 stop 接口。

## WebSocket API

父 session WebSocket 的应用层消息都是 UTF-8 JSON text frame。单条 WebSocket message/frame 上限均为 1 MiB；父连接的 binary frame 被忽略。child observer 是严格只读连接，任何 text 或 binary 输入都会以 WebSocket close code `1008` 关闭。服务器单次写等待超过 10 秒会结束该 socket。

客户端发出的每个命令都带由客户端生成的 `request_id`。命令的直接结果只回给发送该命令的 socket；由命令产生的 `event` 会广播给同一 session 的所有 socket。

`prompt` 命令可以显式指定 skill。未提供 `skill` 时与旧协议完全一致；提供时 daemon 会在激活和排队之前由 library 确定性展开该 skill。未知或不可显式调用的 skill 会以 `invalid_command` 拒绝，因此首个无效 prompt 不会创建 session：

```json
{
  "type": "prompt",
  "request_id": "r1",
  "content": { "type": "text", "value": "检查认证逻辑" },
  "skill": {
    "name": "code-review",
    "arguments": "--focus security"
  }
}
```

服务端顶层 frame 如下：

| `type` | 其余字段 | 含义 |
| --- | --- | --- |
| `building` | 无 | `/new` 正在构建 Agent |
| `ready` | `config`、`mode` | `/new` 已可接受命令，但尚未激活 session |
| `session_created` | `session_id` | 首 prompt 已激活并持久化 session |
| `snapshot` | `session` | `/attach` 的完整当前状态 |
| `subagent_snapshot` | `subagent`、`input_allowed=false` | child observer 的完整当前状态 |
| `subagent_event` | `sequence`、`parent_session_id`、`agent_id`、`event` | child observer 的有序只读事件 |
| `subagent_resync_required` | `skipped`、`subagent`、`input_allowed=false` | child observer lag 后的完整状态替换 |
| `command_accepted` | `request_id`、`command`，可选 `run_id`、`queue_position` | 命令已被接纳 |
| `command_rejected` | `request_id`、`code`、`message` | 命令未被接纳 |
| `event` | `sequence`、`session_id`、可选 `run_id`、`event` | 有序广播事件 |
| `resync_required` | `skipped`、`session` | 客户端 lag 后的完整状态替换 |
| `pong` | `request_id` | 应用层 ping 响应 |
| `fatal_error` | `code`、`message` | 当前 socket 无法继续建立 session |

表中标为“可选”的字段在没有值时会省略；`SessionDto` 内的 `active_run_id`、`draft`、usage 等 option 字段则会显式序列化为 `null`。

### 新建：`GET /v1/ws/new`

`profile_id` 是唯一的 query 参数，可选且默认为 `default`。临时 token 不允许放在 URL 中；浏览器客户端通过 subprotocol 数组提交固定协议和凭证协议：

```javascript
const issued = await fetch("http://127.0.0.1:8787/v1/auth/token", {
  method: "POST",
  headers: { Authorization: `Bearer ${daemonKey}` },
}).then((response) => response.json());

const socket = new WebSocket(
  "ws://127.0.0.1:8787/v1/ws/new?profile_id=default",
  [issued.protocol, `phi.auth.${issued.token}`],
);
```

正常消息序列：

1. 服务端立即发送 `{"type":"building"}`。
2. factory 构建内存 Agent；成功后发送 `ready` 和有效配置。此时没有 metadata、没有 registry entry，也没有可 attach 的 session。
3. 客户端可以先发送 `set_model`、`set_reasoning_effort`、`set_mode` 或 `ping`。
4. 首个 `prompt` 触发 session 激活。成功后服务端先发送 `session_created`，再发送该 prompt 的 `command_accepted`。
5. 已缓冲的 `session_initialized`、`run_queued`、`run_started` 以及 Agent 流式事件随后按 `sequence` 发出。

```json
{"type":"building"}
```

```json
{
  "type": "ready",
  "mode": "default",
  "config": {
    "model": "model-name",
    "reasoning_effort": null,
    "revision": 0
  }
}
```

```json
{
  "type": "session_created",
  "session_id": "019c0000-0000-7000-8000-000000000001"
}
```

如果 Provider 尚未配置或 Agent 构建失败，`building` 之后会收到：

```json
{
  "type": "fatal_error",
  "code": "agent_build_failed",
  "message": "..."
}
```

注意：首个 prompt 前的 config change 也是 runtime event，因此 event envelope 可能已经包含内部预留的 `session_id`；`session_created` 仍只表示“首个 prompt 已成功激活并持久化 session”。

### 恢复/订阅：`GET /v1/ws/attach/{session_id}`

`session_id` 是 UUID。每次 attach 都必须重新换取一个单次临时 token，并以同样的 subprotocol 数组提交：

```javascript
const socket = new WebSocket(
  "ws://127.0.0.1:8787/v1/ws/attach/<session-id>",
  [issued.protocol, `phi.auth.${issued.token}`],
);
```

连接成功后，服务端首先发送完整 `snapshot`：

```json
{
  "type": "snapshot",
  "session": {
    "session_id": "019c0000-0000-7000-8000-000000000001",
    "profile_id": "default",
    "initialized": true,
    "status": "running",
    "active_run_id": "019c0000-0001-7000-8000-000000000002",
    "queued_runs": 1,
    "mode": "plan",
    "config": {
      "model": "model-name",
      "reasoning_effort": "medium",
      "revision": 2
    },
    "history": [
      {
        "role": "user",
        "content": {"type": "text", "value": "你好"},
        "tool_calls": [],
        "tool_call_id": null,
        "tool_result_is_error": false
      }
    ],
    "draft": {
      "text": "正在",
      "tool_calls": []
    },
    "pending_asks": [],
    "pending_plan_approvals": [],
    "subagents": [],
    "usage": {
      "last": null,
      "context": null,
      "cumulative": {
        "input_tokens": 0,
        "output_tokens": 0,
        "total_tokens": 0,
        "cached_input_tokens": 0
      }
    },
    "last_sequence": 12
  }
}
```

连接 attach 时会先订阅广播、再读取快照，并按 sequence 去重，所以快照与 live event 交界处不会因竞争而丢更新。若 snapshot 为 `running`/`stopping`，`draft` 是当前尚未提交的 assistant 流；后续 delta 会继续以 event 发送。`pending_asks` 与 `pending_plan_approvals` 始终是数组，重连客户端应据此重新渲染尚未回答的问题或审批。

未知 session 会在 WebSocket upgrade 后收到 `fatal_error`，code 为 `attach_failed`。多个客户端可以同时 attach；任一客户端提交的 prompt、stop 或配置变更所产生的事件都会同步给所有客户端。

### 只读子 Agent：`GET /v1/ws/attach/{parent_session_id}/subagents/{agent_id}`

父 Agent 成功调用 `spawn_agent` 后，父 session 的所有 attach 客户端都会收到 `subagent_spawned`，其中包含稳定的 `agent_id`、`initial_delivery_id` 和 `observer_path`。父 session snapshot 的 `subagents` 也包含当前状态与同一路径。

使用新的单次临时 token 连接 `observer_path` 后，服务端先发送 `subagent_snapshot`，再持续发送 `subagent_event`。该连接只用于观察 child 的 transcript、流式 draft、通知与生命周期；它不接受 prompt、消息、stop 或 close 命令。WebSocket control ping/pong/close 正常工作，但任何应用层 text（包括 JSON ping）或 binary frame 都会收到 policy-violation `1008` close，且内容不会被反序列化或执行。

child observer 使用自己独立的 sequence。消费过慢时服务端发送 `subagent_resync_required`；客户端应以其中 snapshot 的 `last_sequence` 重建状态。child 永久关闭后在父 actor 生命周期内保留 tombstone，observer 可以读取最终 snapshot，但不能恢复或再次输入。当前 child registry 与事件 cursor 是进程内状态，daemon 重启恢复父 session 时不会恢复之前的 live child。

## 客户端命令

### prompt

文本 prompt：

```json
{
  "type": "prompt",
  "request_id": "prompt-1",
  "content": {
    "type": "text",
    "value": "解释一下当前项目"
  }
}
```

多模态 prompt 使用 `parts`；`image_url.detail` 可为 `auto`、`low`、`high` 或 `null`：

```json
{
  "type": "prompt",
  "request_id": "prompt-2",
  "content": {
    "type": "parts",
    "value": [
      {"type": "text", "text": "描述这张图"},
      {
        "type": "image_url",
        "image_url": {
          "url": "https://example.test/image.png",
          "detail": "high"
        }
      }
    ]
  }
}
```

接收成功：

```json
{
  "type": "command_accepted",
  "request_id": "prompt-1",
  "command": "prompt",
  "run_id": "019c0000-0001-7000-8000-000000000002",
  "queue_position": 1
}
```

`queue_position` 是命令被 actor 接纳时的等待队列位置。即使 session 当前正在输出、停止或压缩，prompt 也会被接纳并按 actor 的 FIFO 顺序等待；当前操作终止后才会开始下一 run。等待 prompt 的容量当前为 64，满时返回 `queue_full`。

### 主动压缩上下文

主动压缩只允许从已经建立的 `/attach` 连接发起，`/new` 会以 `invalid_command` 拒绝。session 必须处于 `idle` 且已有对话历史；压缩或 run 正在执行时返回 `session_busy`。`instructions` 可省略，也可补充本次摘要应特别保留的内容：

```json
{
  "type": "compact",
  "request_id": "compact-1",
  "instructions": "保留所有已确认的存储不变量"
}
```

actor 接纳任务并切换到 `compacting` 后，发送者会先收到：

```json
{
  "type": "command_accepted",
  "request_id": "compact-1",
  "command": "compact"
}
```

随后所有 attach 客户端按同一 sequence 收到 `context_compaction_started`，其中 `prompt` 是实际交给压缩模型的摘要提示；成功时收到 `context_compaction_completed`，其 `changed_from` 与 `replacement` 是应用到 `snapshot.session.history` 的 replace-tail patch。失败时收到 `context_compaction_failed`，原 history 不变。完成或失败后状态回到 `idle`。

daemon 创建每个 Agent 时显式安装一份新的 `DefaultContextCompactor`。它按固定 token 余量自动压缩，同时也是上述主动命令使用的默认实现；factory 保留按 Agent 构造策略的入口，便于未来在创建 session 时选择实现，并在 session 空闲时安全切换。

### stop

```json
{
  "type": "stop",
  "request_id": "stop-1",
  "run_id": "019c0000-0001-7000-8000-000000000002"
}
```

`run_id` 必须与 snapshot/event 中的 `active_run_id` 精确一致。stop 不能取消尚在队列里的 run，也不会清空后续 prompt 队列。`command_accepted` 只表示停止信号已被接纳；应以广播的 `run_stopped` 作为该 run 的终态。

没有 active run 时返回 `no_active_run`，ID 不匹配返回 `run_mismatch`。

### 修改模型

```json
{
  "type": "set_model",
  "request_id": "model-1",
  "model": "another-model"
}
```

model 必须非空。该命令只允许在 `awaiting_first_prompt` 或 `idle` 状态执行；`compacting`、`running`、`stopping`、`closing`、`closed` 状态返回 `session_busy`。成功后 revision 加一，并广播 `config_changed`。

### 修改思考强度

```json
{
  "type": "set_reasoning_effort",
  "request_id": "reasoning-1",
  "effort": "high"
}
```

允许值为 `none`、`minimal`、`low`、`medium`、`high`、`xhigh`、`max`；传 `null` 会清除 session override。状态限制与 `set_model` 相同。具体模型/Provider 是否支持某一级别，仍由 adapter 做兼容映射。

### 切换执行模式

```json
{
  "type": "set_mode",
  "request_id": "enter-plan",
  "mode": "plan"
}
```

`mode` 为 `default` 或 `plan`。该命令只允许在 `awaiting_first_prompt` 或 `idle` 状态执行，并在成功后广播 `mode_changed`。Plan Mode 是工具能力边界：Provider 只能看到并执行只读、内部协调及 plan-only 工具；未知、自定义和外部副作用工具默认不会暴露，即使模型伪造 tool call，执行入口还会再次拒绝。模式也保存在 session snapshot 中，重启恢复后继续生效。

daemon 注入的 plan-only 工具如下：

- `read_plan` 返回当前 plan 或 `null`。
- `write_plan` 接受 `expected_revision` 和完整 Markdown `content`；首次写入使用 revision `0`，成功后 revision 递增。revision 不一致时写入失败，不覆盖新版本。调用被取消或 timeout 后结果可能不确定，重试前必须先用 `read_plan` 对账，不能盲目重放旧 `expected_revision`。
- `exit_plan_mode` 不接受参数，读取当前非空 plan 并发布审批；它不会仅凭模型输出切换模式。

### 审批退出 Plan Mode

`exit_plan_mode` 等待期间，所有 attach 客户端都会收到 `plan_approval_requested`，完整请求也保留在 `snapshot.session.pending_plan_approvals`：

```json
{
  "type": "event",
  "sequence": 24,
  "session_id": "019c0000-0000-7000-8000-000000000001",
  "run_id": "019c0000-0001-7000-8000-000000000002",
  "event": {
    "type": "plan_approval_requested",
    "request": {
      "approval_id": "019c0000-0002-7000-8000-000000000003",
      "plan": {
        "session_id": "019c0000-0000-7000-8000-000000000001",
        "revision": 3,
        "content": "# Plan\n\n1. ..."
      }
    }
  }
}
```

批准必须回传同一个 revision：

```json
{
  "type": "decide_plan_approval",
  "request_id": "approve-plan-3",
  "approval_id": "019c0000-0002-7000-8000-000000000003",
  "decision": {"type": "approve", "revision": 3}
}
```

拒绝可附带最多 16 KiB 的反馈；空白反馈会归一化为无反馈：

```json
{
  "type": "decide_plan_approval",
  "request_id": "reject-plan-3",
  "approval_id": "019c0000-0002-7000-8000-000000000003",
  "decision": {
    "type": "reject",
    "revision": 3,
    "feedback": "补充回滚与迁移步骤"
  }
}
```

格式或 revision 不匹配返回 `invalid_plan_approval_decision`，请求继续 pending。若计划文件在等待期间已经更新，返回 `stale_plan_approval`、广播 `plan_approval_cancelled`，模型收到失败的 tool result 并继续留在 Plan。批准成功会广播 `plan_approval_decided` 和 `mode_changed(default)`，然后把批准结果作为 tool result 交还模型；只有后续 turn 能使用 Default 权限。涉及 Plan 的工具 batch 强制按调用顺序执行：`write_plan → exit_plan_mode` 审批写入后的新 revision；`exit_plan_mode → write_plan` 在批准切换到 Default 后拒绝后续写入，因此已审批 artifact 不会被同批调用改写。其他同批副作用调用也不会因 Exit 解锁。

socket 断开不取消审批。stop、shutdown 或 run 终止会广播 `plan_approval_cancelled`。如果批准后的 tool-result 持久化失败，core 会恢复 Plan，daemon 在 `run_failed` 前广播对应的 `mode_changed(plan)`，快照不会停留在错误的 Default 投影。

### 回答 `askuser`

daemon 会给每个创建的 Agent 注入名为 `askuser` 的工具。模型调用它后，run 保持 `running`，所有 attach 客户端会收到 `askuser_requested`：

```json
{
  "type": "event",
  "sequence": 18,
  "session_id": "019c0000-0000-7000-8000-000000000001",
  "run_id": "019c0000-0001-7000-8000-000000000002",
  "event": {
    "type": "askuser_requested",
    "request": {
      "ask_id": "019c0000-0002-7000-8000-000000000003",
      "questions": [
        {
          "question": "采用哪种布局？",
          "header": "布局",
          "options": [
            {
              "label": "紧凑 (Recommended)",
              "description": "减少留白",
              "preview": "[A] [B]"
            },
            {
              "label": "宽松",
              "description": "增加留白",
              "preview": "[ A ]   [ B ]"
            }
          ],
          "multiSelect": false
        }
      ]
    }
  }
}
```

一次调用包含 1–3 个问题，每题包含 2–4 个显式选项。`multiSelect` 为 `false` 时只能选择一个显式选项或填写一次自定义文本；为 `true` 时可组合多个显式选项与一条自定义文本。UI 中的 “Other” 是隐式入口，不需要模型把它放进 `options`。`preview` 只允许用于单选题，内容是供客户端并排展示的 Markdown/等宽预览。

任一 attach 客户端可用 `ask_id` 回答。`answers` 必须按 `question_index` 覆盖全部问题；`selected_options` 的值是原始 option label，`custom_text` 表示用户的 “Other” 回复：

```json
{
  "type": "answer_askuser",
  "request_id": "answer-1",
  "ask_id": "019c0000-0002-7000-8000-000000000003",
  "answers": [
    {
      "question_index": 0,
      "selected_options": [],
      "custom_text": "使用我自己的混合布局"
    }
  ]
}
```

成功会直接回复 `command_accepted`，其中 `command` 为 `answer_askuser`，并向全部 attach 广播 `askuser_answered`。答案被序列化为 tool result 交还模型，原 run 随后继续。格式错误返回 `invalid_askuser_answer` 且请求仍保持 pending；不存在、已回答或已取消的 `ask_id` 返回 `askuser_not_pending`。

socket 断开不会取消问题；它保留在 `snapshot.session.pending_asks` 中。stop、shutdown 或 run 以其他方式终止时，尚未回答的问题会收到 `askuser_cancelled` 并从快照移除。

### ping

这是应用层 ping，与 WebSocket control ping 不同：

```json
{"type":"ping","request_id":"ping-1"}
```

```json
{"type":"pong","request_id":"ping-1"}
```

WebSocket control ping 也会得到 control pong。

### 命令拒绝

```json
{
  "type": "command_rejected",
  "request_id": "prompt-1",
  "code": "queue_full",
  "message": "..."
}
```

当前 code 包括 `invalid_command`、`queue_full`、`session_busy`、`no_active_run`、`run_mismatch`、`askuser_not_pending`、`invalid_askuser_answer`、`plan_approval_not_pending`、`invalid_plan_approval_decision`、`stale_plan_approval`、`actor_stopped`、`operation_failed` 和首 prompt 专用的 `session_activation_failed`。无法解析 JSON 时 `request_id` 是空字符串。

## 事件与客户端状态投影

每个 runtime event 使用同一个 envelope：

```json
{
  "type": "event",
  "sequence": 13,
  "session_id": "019c0000-0000-7000-8000-000000000001",
  "run_id": "019c0000-0001-7000-8000-000000000002",
  "event": {
    "type": "message_update",
    "delta": {
      "type": "text",
      "delta": "输出片段"
    }
  }
}
```

工具调用参数 delta 的形状为：

```json
{
  "type": "tool_call",
  "index": 0,
  "id": "call_1",
  "name": "read",
  "arguments_delta": "{\"path\":"
}
```

事件类型分组如下：

- runtime 生命周期：`state_changed`、`session_initialized`、`run_queued`、`run_started`、`run_completed`、`run_stopped`、`run_failed`、`config_changed`、`mode_changed`、`askuser_requested`、`askuser_answered`、`askuser_cancelled`、`plan_approval_requested`、`plan_approval_decided`、`plan_approval_cancelled`、`subagent_spawned`、`subagent_state_changed`、`subagent_message_queued`、`subagent_notification`、`subagent_run_finished`、`subagent_closed`、`subagents_resynced`、`operation_failed`、`actor_crashed`。
- Agent 生命周期：`agent_start`、`agent_end`、`agent_stopped`、`turn_start`、`turn_end`。
- 流式消息：`message_start`、`message_update`、`message_end`、`message_aborted`。
- 工具、压缩与统计：`tool_execution_start`、`tool_execution_end`、`usage_update`、`provider_retry`、`context_compaction_started`、`context_compaction_completed`、`context_compaction_failed`、`error`。

`event` 对象的字段如下；没有列出字段的事件只有 `type`：

| `event.type` | 其余字段 |
| --- | --- |
| `state_changed` | `status` |
| `session_initialized` | 无 |
| `run_queued`、`run_started`、`run_completed`、`run_stopped` | `run_id` |
| `run_failed` | `run_id`、`message` |
| `config_changed` | `config` |
| `mode_changed` | `mode` |
| `askuser_requested` | `request`，包含 `ask_id` 与 `questions` |
| `askuser_answered`、`askuser_cancelled` | `ask_id` |
| `plan_approval_requested` | `request`，包含 `approval_id` 与不可变 `plan` revision/content |
| `plan_approval_decided` | `approval_id`、`decision` |
| `plan_approval_cancelled` | `approval_id` |
| `subagent_spawned` | `agent_id`、`description`、`initial_delivery_id`、`observer_path` |
| `subagent_state_changed` | `agent_id`、`state` |
| `subagent_message_queued` | `agent_id`、`delivery_id` |
| `subagent_notification` | `agent_id`、`notification`（含 `kind`、`source`、`message`、`wake_parent`） |
| `subagent_run_finished` | `agent_id`、`run_id`、`outcome` |
| `subagent_closed` | `agent_id`、`delivery_id`、`reason`、`wake_parent` |
| `subagents_resynced` | `subagents` 完整父投影 |
| `operation_failed` | `operation`、`message` |
| `actor_crashed` | `message` |
| `agent_start`、`agent_end`、`agent_stopped` | 无；完整历史在 snapshot 投影中，不随终止 marker 重复广播 |
| `turn_start` | `turn` |
| `turn_end` | `turn`、`message`、`tool_results` |
| `message_start`、`message_end` | `message` |
| `message_update` | `delta` |
| `message_aborted` | 无 |
| `tool_execution_start` | `call` |
| `tool_execution_end` | `call`、`content`、`is_error` |
| `usage_update` | `usage`、`context_usage` |
| `provider_retry` | `retry_number`、`max_retries`、`delay_ms`、`reason` |
| `context_compaction_started` | `trigger`、`compactor`、`prompt` |
| `context_compaction_completed` | `trigger`、`compactor`、`before_message_count`、`after_message_count`、`changed_from`、`replacement`、`summary`、`usage`、`estimated_context_tokens` |
| `context_compaction_failed` | `trigger`、`compactor`、`message` |
| `error` | `message` |

Public `message` 固定包含 `role`、`content`、`tool_calls`、`tool_call_id`、`tool_result_is_error`。`call` 固定包含 `id`、`name`、任意 JSON `arguments`。`context_usage` 非空时包含 `max_tokens`、`used_tokens`、`remaining_tokens`。压缩 `trigger.type` 为 `manual`、`automatic` 或 `context_length_exceeded`；手动 trigger 可带 `instructions`，自动 trigger 带触发前的 `usage`。`provider_retry.reason.type` 为 `request_timeout`、`transport` 或 `http_status`，分别携带 `timeout_ms`、`message`，或 `status` + `body`；`request_timeout` 为事件协议兼容保留，内置 Provider 对响应头超时直接失败，不再发出该 retry 事件。

常见 run 顺序是：

```text
run_queued
state_changed          # 从等待队列移除，queued_runs 变化
run_started
agent_start
message_start/end      # user message
turn_start
message_start
message_update ...
message_end
turn_end
agent_end
run_completed
```

存在 tool call、多 turn、重试、错误或 stop 时会插入相应事件。客户端不应依赖上面的最短序列来判断结束，应以 `run_completed`、`run_stopped` 或 `run_failed` 为 run 的终态。

广播环容量当前为 1024 个 event。若某个 socket 消费过慢导致 lag，服务端发送完整重同步帧：

```json
{
  "type": "resync_required",
  "skipped": 37,
  "session": {"...": "完整 SessionDto，与 snapshot.session 同形"}
}
```

客户端应丢弃自己的派生状态，以该 `session` 为准，并从其中的 `last_sequence` 继续。sequence 是 live actor 内的单调序号，不是持久化的全局 offset；进程重启后重新恢复 actor 时会重新开始。

## 排队、多 attach 与停止语义

### 排队和多 attach

- 所有 attach 共用同一个 actor 和一条 prompt FIFO。一个 session 同时最多执行一个 run。
- prompt 在 `running`/`stopping` 时仍可入队。配置变更不会排队等待空闲，而是直接以 `session_busy` 拒绝。
- direct `command_accepted`/`command_rejected` 只发给命令来源 socket；`run_queued` 及后续 runtime/Agent event 广播给全部 attach。
- socket 断开不会停止 run，也不会移除已经入队的 prompt。客户端可以重新 attach 并从 snapshot 恢复视图。
- socket 断开同样不会取消 `askuser`；待回答请求由 session actor 持有，不属于某一个客户端。
- 当前协议没有“取消某个 queued run”或“清空队列”命令。

### stop 的安全 checkpoint

stop 是合作式停止，不是把 session 简单截断到“最后一个数组元素”，更不是事务性撤销外部副作用。实现保证持久 transcript 保持 Provider 协议可继续使用：

- 用户消息在 run 开始时先持久化。因此在 assistant 流式输出中 stop，会丢弃未提交 draft、发出 `message_aborted`，但保留触发该 run 的用户消息。
- assistant 流式 draft 不进入 transcript。若完整响应包含工具调用，Agent 会在启动任何工具前先持久化 assistant 调用及一一配对的 `unknown` error result；journal 保存失败时不会执行工具。
- stop 会参与 Provider stream、三个生命周期 hook，以及顺序/并行工具 future 的等待。尚未进入工具 journal 的 assistant draft 会被丢弃并发出 `message_aborted`，不会无限等待一个不返回的 hook 或工具。
- 工具正常完成后会把 journal 尾部更新为真实结果；顺序模式下尚未启动的调用使用明确的 cancelled result，已启动但因 stop、超时或 panic 无法确认结果的调用保留 unknown result。恢复后不会自动重放原调用。
- 丢弃 Rust future 只能停止 Agent 对它的等待，不能事务性撤销工具已经造成的文件、网络或远端系统副作用；工具自行分离出去的后台任务也可能继续运行。daemon 因此提供的是“可恢复且不自动重放未知调用”，不是 exactly-once。非幂等工具仍应使用 `tool_call.id` 作为业务幂等键。
- stop 后，当前 run 最终广播 `agent_stopped`/`run_stopped`，状态回到 `idle`；队列中已有的下一个 prompt 会继续启动。

这也意味着“回滚”始终回到最近一个可持久化、协议完整的 checkpoint：它可能只包含刚刚的 user message，也可能包含一整个 assistant tool-call 与配对的 completed/cancelled/unknown tool results。

## 当前未实现的能力

- TLS、WebSocket origin 校验、租户与权限模型。
- HTTP session detail/create/delete、queued-run cancel、队列清空和 session 删除协议。
- daemon binary 中除 `askuser`、subagent 工具、`read_plan`、`write_plan`、`exit_plan_mode` 之外的内置工具和 MCP 配置；相关能力存在于 `phi` library，但当前 HTTP-managed factory 没有接线。
- 跨 daemon 实例共享 live actor、分布式锁或事件总线；当前 mapping 与广播都是单进程的。
- durable event cursor/replay；重连通过 snapshot 恢复，不按旧 sequence 重放。
- 工具外部副作用的事务回滚或强制中断。
