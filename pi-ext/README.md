# Pi-backed Phi daemon

`pi-ext` 是一个 TypeScript 实现的 Phi daemon v1 transport，Agent 运行时由
[`@earendil-works/pi-coding-agent`](https://pi.dev/docs/latest/sdk) 提供。HTTP/WS 客户端继续使用
Phi 的公开 DTO、鉴权和 session actor 语义；模型、工具、skills、extensions、settings、认证与
conversation JSONL 则交给 Pi SDK。

与 Rust `phi-daemon` 相比，默认监听端口从 `8787` 改为 `8788`，配置根目录从 Phi 目录改为
Pi 的默认 agent 目录。

## 架构

```text
Web / Flutter / other Phi v1 clients
                │
        HTTP + WebSocket (phi.v1)
                │
          DaemonServer
          ├─ bearer auth -> one-use WS token
          ├─ HTTP DTO/routes
          └─ prepared /new + attach transport
                │
       ApplicationService
       ├─ live actor registry / restore single-flight
       ├─ Pi session discovery / fork / metadata
       └─ one SessionActor per live session
                │
          SessionActor
          ├─ one active turn + bounded FIFO (64)
          ├─ ordered global sequence / snapshots
          ├─ askuser and tool-permission brokers
          └─ config/compaction admission
                │
       @earendil-works/pi-coding-agent
       ├─ AgentSession / model runtime / tools
       ├─ DefaultResourceLoader / skills / extensions
       └─ SessionManager JSONL
```

职责分层与原 daemon 对齐：`server.ts` 只处理 transport，`service.ts` 管理 session 用例与
registry，`session-actor.ts` 串行化每个 live session，`pi-session.ts` 是 Pi SDK adapter，
`projection.ts` 负责 Pi message/event 到 provider-neutral wire DTO 的投影。

### 关键映射

| Phi daemon 概念 | TypeScript/Pi 实现 |
| --- | --- |
| `ApplicationService` | `src/service.ts` |
| `AgentRegistry` + `SessionActor` | `ApplicationService` registry + `SessionActor` |
| `AgentFactory` | `PiSessionFactory` + `createAgentSession()` |
| Provider store/factory | Pi `ModelRuntime`，外加兼容 profile overlay |
| conversation storage | Pi `SessionManager` JSONL |
| metadata/profile/task store | `<Pi agent dir>/daemon/control.json` |
| `askuser` | Pi custom tool + actor-owned `AskUserBroker` |
| capability approval | Pi `beforeToolCall` + actor-owned `ToolPermissionBroker` |
| context compaction | Pi `AgentSession.compact()`，投影成 Phi compaction events |

## 配置与磁盘目录

代码直接调用 Pi SDK 的 `getAgentDir()`。默认目录为 `~/.pi/agent`，并遵循 Pi 的
`PI_CODING_AGENT_DIR` 覆盖值。

```text
~/.pi/agent/
├── auth.json              # Pi provider credentials
├── models.json            # Pi custom models/providers
├── settings.json          # Pi settings
├── skills/                # Pi global skills
├── extensions/            # Pi extensions
├── sessions/              # Pi SessionManager JSONL transcripts
└── daemon/
    ├── auth.key           # daemon long-lived bearer key, Unix 0600
    └── control.json       # profiles, metadata, pin/revision, scheduled tasks
```

`provider_state`、Pi credential 和 API key 不进入 public history。兼容 Provider profile 的 API
key 只保存在权限为 `0600` 的 `control.json` 中，GET 响应只返回
`api_key_configured: true/false`。

支持的进程配置：

| 环境变量 | 默认值 | 用途 |
| --- | --- | --- |
| `PI_EXT_BIND` | `127.0.0.1:8788` | HTTP(S)/WS(S) 地址 |
| `PI_EXT_DATA_DIR` | `<Pi agent dir>/daemon` | daemon control state |
| `PI_EXT_AUTH_KEY_FILE` | `<data dir>/auth.key` | 长期 bearer key；显式路径不存在时拒绝启动 |
| `PI_EXT_WORKSPACE_DIR` | 启动工作目录 | 新 session 默认 workspace |
| `PI_EXT_TLS_CERT_FILE` | 未设置 | PEM certificate，必须和 key 成对配置 |
| `PI_EXT_TLS_KEY_FILE` | 未设置 | PEM private key |

## 安装与启动

要求 Node.js 22.19+ 和 pnpm。

```bash
cd pi-ext
pnpm install
pnpm build
pnpm start
```

也可以直接使用 CLI 参数：

```bash
pnpm start -- --bind 127.0.0.1:8788 --workspace /absolute/project
```

启动时若默认 key 文件不存在，会生成 32-byte 随机 key。CLI 只打印 key 文件路径，不打印
key 本身。

如果 Pi 已在 `auth.json`、`models.json` 和 `settings.json` 中配置默认 provider/model，
`/v1/ws/new?profile_id=default` 会直接使用它。也可以通过 Phi 兼容 API 创建一个 daemon
profile：

```bash
KEY="$(tr -d '\r\n' < ~/.pi/agent/daemon/auth.key)"
curl -X PUT http://127.0.0.1:8788/v1/providers/openai-main \
  -H "Authorization: Bearer $KEY" \
  -H 'Content-Type: application/json' \
  -d '{
    "provider":"openai_chat",
    "api_key":"replace-me",
    "base_url":"https://api.openai.com/v1",
    "model":"gpt-5.1-codex",
    "max_context_tokens":200000
  }'
```

Provider profile 更新只影响之后创建或重启恢复的 session。live session 继续持有自己的
Pi `ModelRuntime` 和 pinned config。Phi 兼容 profile 沿用原 daemon 默认值：
`max_retries=10`、`request_timeout_secs=30`、`stream_idle_timeout_secs=120`；直接来自 Pi 的
native provider 则服从 Pi `settings.json`。

## HTTP 协议

所有 HTTP route 都要求一个且仅一个严格格式的 `Authorization: Bearer <key>` header。

| Method | Route | 说明 |
| --- | --- | --- |
| `POST` | `/v1/auth/token` | 换取 60 秒、单次使用的 WS subprotocol token |
| `GET/PUT` | `/v1/provider` | `default` provider 兼容 route |
| `GET` | `/v1/providers` | Pi native provider 与 daemon overlay 列表 |
| `GET/PUT` | `/v1/providers/{profile_id}` | provider profile |
| `GET` | `/v1/agent-profiles` | agent profile 列表 |
| `GET/PUT` | `/v1/agent-profiles/{id}` | agent profile |
| `GET` | `/v1/sessions` | pinned-first session/workspace projection |
| `GET/PATCH/DELETE` | `/v1/sessions/{id}` | summary、pin、删除 |
| `POST` | `/v1/sessions/{id}/fork` | `after` / `before_tool_calls` fork |
| `GET` | `/v1/sessions/{id}/skills` | pinned Pi skill catalog |
| `GET` | `/v1/workspaces/browse` | canonical absolute directory browser |
| CRUD | `/v1/scheduled-tasks` | interval/daily scheduled sessions |

JSON body 和 WebSocket message 上限为 1 MiB。错误响应使用稳定的
`{"code":"...","message":"..."}`，内部异常不回传 stack/debug 信息。

## WebSocket 协议与状态机

浏览器先请求 `/v1/auth/token`，再同时提供两个 subprotocol：

```text
phi.v1
phi.auth.<one-use-token>
```

服务端只选择并回显 `phi.v1`，不会把 token 放进响应。

### 新 session

```text
connect /v1/ws/new
  -> building
  -> ready
  -> set_model / set_reasoning_effort / set_capability_mode (可选)
  -> first prompt
  -> session_created
  -> command_accepted
  -> ordered event stream
```

`ready` 前只构建 `PreparedSession`。首个有效 prompt 前不会写 metadata 或 transcript，也不会
出现在 session list；断开连接无需残留清理。首 prompt 激活后，actor 才进入 live registry。

### Attach 与重连

`/v1/ws/attach/{session_id}` 会先订阅 actor，再读取并发送完整 `snapshot`。所有广播事件共享
session 级单调 `sequence`。transport backpressure 造成事件跳过时发送
`resync_required { skipped, session }`，客户端用新 snapshot 替换本地投影。

事件不是 durable replay log；持久化恢复依赖 Pi transcript 和 daemon metadata，而不是重放
旧 WebSocket event。

### 命令 admission

- `prompt` 在 running/stopping 时进入上限为 64 的 FIFO；每个 session 同时最多运行一个 turn。
- `set_model`、`set_reasoning_effort`、`set_capability_mode` 和主动 `compact` 仅在 idle 接受。
- `stop` 必须携带当前 `run_id`。
- `askuser`、tool permission 挂起状态属于 actor；任一 attach 客户端可回答，单个 socket 断开
  不会取消请求。
- direct `command_accepted`/`command_rejected` 只发回来源 socket；run、Agent、tool、usage、
  compaction 和交互状态按 sequence 广播给全部 attach。

Pi profile 允许的工具会保持对模型可见。调用 effect 超过当前 capability 时，
`beforeToolCall` 暂停执行并广播 `tool_permission_requested`；客户端可 `allow_once`、使用服务端
候选规则 `allow_for_session`，或 `deny`。stop/shutdown 会取消挂起审批，外部工具不会在决定前
执行。记住的规则先作为 Pi custom session entry 落盘，再释放工具执行；恢复 session 时会
重建规则，fork 不继承。

在 `read_only` / `workspace_edit` 下，Pi 内置 `read`、`grep`、`find`、`ls`、`edit`、`write`
会在真正执行前 canonicalize 路径，包含 symlink 和尚不存在的写入目标，拒绝逃逸 session
workspace。`full_access` 保留 Pi 原生文件和进程权限；这仍是应用层 capability，不是操作系统
sandbox。Pi extensions/MCP 等自定义工具的 effect 与路径安全仍由扩展作者负责。

scheduled task 保持 1000 个任务、8 个并发 run、名称 100 字符、prompt 20000 字符和最长十年
interval 的边界。后台 session 以任务名作为标题，并关闭交互式 tool permission prompt；超出
capability 的工具直接 fail closed，避免无人 attach 时无限等待。`askuser` 仍可按协议挂起。

## 兼容范围

已实现的公开面包括 provider/agent profiles、session list/pin/delete/fork/skills、workspace
browse、scheduled tasks、prepared `/new`、attach/resync、FIFO、stop、config、compaction、
askuser、tool permission、usage/message/tool event projection、TLS 和 key/token 鉴权。

以下属于明确的运行时差异，而不是隐藏的 wire 变化：

1. Pi SDK 当前没有 Phi child/subagent runtime。只读 observer route 会返回
   `fatal_error{subagents_disabled}`，parent snapshot 的 `subagents` 为空。
2. Conversation 的 durable 格式和 compaction 实现是 Pi `SessionManager` 的 JSONL/compactor，
   不是 Rust `phi::SessionStorage`。因此 crash-journal 的磁盘细节服从 Pi SDK；public history
   从 Pi active branch 单独投影，压缩摘要正文不会泄露，未完成 streaming draft 不作为最终
   assistant message 投影。
3. 自动标题使用首 prompt 的本地、长度受限摘要，不额外消耗一次模型调用。
4. Pi SDK 原生 prompt 只接受文本和 base64 data image。远程图片 URL 与 Phi document block
   会降级成明确的文本标记，不会伪装成已经传给模型的二进制内容。

Provider profile 为保持 Phi wire 兼容仍接受 `system_prompt`，但 Pi 的 session system prompt
由 resource loader/extensions 管理，因此该字段不会写入运行时，GET 时固定投影为 `null`。

绑定到非 loopback 地址时，本项目和原 daemon 一样不提供 origin 校验或多租户隔离。必须放在
可信网络、TLS 和可信反向代理之后；长期 key 不应出现在 URL、日志或仓库文件中。

## 开发验证

```bash
pnpm check
pnpm test
pnpm build
```

测试使用临时 Pi 目录、dummy provider 和本地 in-process server，不调用真实 Provider 或外网。
如果执行沙箱禁止 loopback listener，HTTP/WS smoke test 会明确标记 skipped；在允许本机监听的
环境中会完整执行。
