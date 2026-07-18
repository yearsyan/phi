# Phi 项目协作指南

本文件适用于整个仓库。修改代码前先确认变更属于 library、daemon 还是协议层；不要把某一层的职责泄漏到另一层。代码和测试是实现事实，`README.md` 与 `crates/phi-daemon/README.md` 是对外行为说明；行为发生变化时必须同步更新对应测试和文档。

## 项目定位

Phi 是一个 Rust 2024 workspace，包含两个 crate：

- 根 crate `phi`：可嵌入的 agent library，负责模型抽象、agent/tool loop、消息与事件、hooks、上下文压缩、session transcript、skills、MCP、capability 控制和 subagent runtime。
- `crates/phi-daemon`：单进程 daemon，依赖 `phi`，负责 Provider profile、HTTP/WebSocket transport、鉴权、session actor、客户端状态投影和磁盘编排。

依赖方向只能是 `phi-daemon -> phi`。可复用的 agent 能力应放在 library；HTTP、WebSocket、daemon 配置、在线 session registry 和客户端 DTO 不得下沉到 library。daemon 应编排 `phi::Agent`，不要复制一套 agent loop、Provider 协议或 transcript 规则。

当前不是完整的 coding-agent CLI。不要假设已经具备 TUI、origin 校验、多租户、分布式 actor、durable event replay、工具副作用事务回滚或 partial/micro compaction。

## 总体架构

```text
HTTP / WebSocket client
        |
        v
phi-daemon::api -> ApplicationService -> AgentRegistry -> one SessionActor
                         |                    |                |
                         v                    |                v
                  AgentFactory               |             phi::Agent
                         |                    |          /      |       \
                  ProviderStore              |   LlmProvider  Tool  SessionStorage
                                              \
                                               -> snapshot + ordered event hub -> clients
```

一个 live session 只有一个 actor，actor 串行拥有一个 `phi::Agent`。同一 session 的 prompt 共用 FIFO，任何时刻最多运行一个 turn。多个 attach WebSocket 只共享状态和事件，不获得 Agent 的并发可变所有权。

## Library 模块边界

- `src/agent.rs`：Agent builder、运行循环、消息 checkpoint、工具调度、停止、mailbox、usage 和事件发布。这里是 transcript 可变状态的唯一协调者。
- `src/types.rs`：Provider 中性的消息、内容块、工具调用、usage、运行结果和 `AgentEvent`。公共语义优先在这里表达，不要让 wire DTO 反向污染它。
- `src/provider/`：`LlmProvider` 边界及 OpenAI Chat、Responses、Anthropic adapter。adapter 独占认证、endpoint、请求 JSON、SSE 和协议特有回放格式。
- `src/tool.rs`、`src/tool/`：工具接口、能力/副作用声明、并发策略和内置工具。Capability 限制必须在运行时再次校验，不能只依赖发给模型的 tool list。
- `src/hook.rs`：普通 Agent/Provider 生命周期 hooks。hooks 按注册顺序异步执行，并参与停止和错误传播。
- `src/context.rs`、`src/context/`：独立的 `ContextCompactor` 策略边界及默认完整压缩实现。它不是 lifecycle hook，也不依赖已删除的 context-manager 抽象。
- `src/storage.rs`：normalized session snapshot 及内存/append-only JSONL storage。
- `src/skills/`：skill 发现、合并、渲染和显式调用工具。
- `src/mcp.rs`：stdio 与 Streamable HTTP MCP client。
- `src/subagent/`、`src/tool/subagent.rs`：父 Agent 作用域的 child runtime、通知/mailbox 及显式注册的父子工具。
- `src/error.rs`：跨模块的 typed errors。外部输入和运行时失败不得用 panic 表达。

`src/lib.rs` 是 library 公共 API 清单。新增公开类型或重命名公开能力时，检查 re-export、Rustdoc、README 示例和兼容性影响。

## Daemon 模块边界

- `api/`：鉴权、HTTP/WS transport、wire DTO 和序列化测试。只负责协议转换与连接生命周期。
- `service.rs`：应用用例、prepared session 激活、metadata、恢复 single-flight 和 registry 协调。
- `runtime/actor.rs`：单 session 命令串行化、run 队列、快照和有序广播。
- `runtime/factory.rs`：根据 Provider profile 创建 Provider 与 Agent，并显式安装 daemon 默认能力。
- `runtime/ask_user.rs`：跨 WebSocket 重连仍由 actor 持有的挂起交互。
- `runtime/registry.rs`：仅保存本进程已经激活或通过 attach 恢复的 actor；prepared `/new` 连接不注册，磁盘 session 也不等于 live actor。
- `store/`：Provider profile、session metadata 的内存/磁盘实现。conversation transcript 仍由 `phi::SessionStorage` 保存。
- `config.rs`、`server.rs`、`telemetry.rs`：进程配置、Axum 装配、关闭与日志。

daemon crate 使用 `#![forbid(unsafe_code)]`；不得删除或弱化这个约束。

## 必须保持的核心不变量

### Transcript 与工具协议

- transcript 必须始终能被所选 Provider 继续回放。assistant tool-call batch 与对应 tool result 必须完整配对，压缩、停止、hook、mailbox 和持久化恢复都不得切断协议组。
- 在执行任何可能产生副作用的工具前，先持久化 assistant tool call 和一一对应的 `unknown` journal result。journal 保存失败时不得执行工具。
- 工具成功后以真实结果替换 journal tail；未启动、取消、超时、panic 或结果不可确认的调用必须保留明确的 cancelled/unknown 结果，恢复后不得自动重放。
- assistant streaming draft 在完整响应前不进入 transcript。停止时可以丢弃 draft，但必须保留最近的协议完整、已持久化 checkpoint。
- 内存 transcript、持久 transcript 和已发布事件必须保持一致。持久化失败时恢复到最近 durable checkpoint，不能让客户端看到一个未落盘的最终状态。
- 工具、hooks 或 Provider future 被取消不代表外部副作用回滚。不要宣称 exactly-once；非幂等工具应使用 `tool_call.id` 作为业务幂等键。

### Provider 中性边界

- Agent core 只使用 `ProviderRequest`、`ProviderEvent`、`ProviderResponse` 和 normalized `Message`；协议字段映射留在对应 adapter。
- assistant message 的 opaque `provider_state` 必须持久化并在同 adapter 中回放，但不得通过 daemon public history、日志或普通 Debug 输出泄露。
- normalized 字段是当前语义来源，不允许旧的 opaque replay state 覆盖新的文本、工具调用或 reasoning 设置。
- 请求超时、stream idle timeout、重试和上下文超限分类集中复用 `provider/retry.rs` 的语义。不要自动重试可能已经被服务端接收的非幂等 POST。
- API key、长期 daemon key、短期 WS token 和认证 header 必须在 Debug、错误、事件和日志中脱敏。

### ContextCompactor

- `ContextCompactor` 是每个 Agent 选择一个的独立 trait。library 不隐式安装实现；daemon 在 factory 中为每个 Agent 创建新的 `DefaultContextCompactor`。
- compactor 的选择属于 Agent/session 创建策略；未来可按 session 选择，并且只允许在 Agent 空闲且持有独占可变访问时切换。不要重新引入 `ContextManager`、manager registry 或百分比 manager hook 链。
- 类型、常量、模块名和事件中的 compactor 名称必须保持模型厂商中性。默认实现的稳定事件名是 `"default"`。
- compactor 接收 transcript snapshot，只生成完整 replacement plan；实现内部不得直接修改 live Agent，也不得递归调用 Agent runtime。Agent 负责验证协议、原子应用和持久化。
- 自动压缩在下一次真实 LLM 请求前判断；手动压缩和“尚未产生有效 assistant 输出时的 context-length error”也走同一策略。overflow 恢复最多重试原请求一次，不能形成无限压缩循环。
- 默认实现沿用 Agent 当前 Provider/模型生成纯文本摘要，禁用 tools/reasoning，成功前不修改 live transcript。摘要请求过长时只能按完整协议消息组裁剪。
- daemon attach WS 的主动压缩必须先进入 actor 串行状态；忙时拒绝。started/completed/failed 状态事件必须广播给调用客户端和其他 attach 客户端，但实际摘要 prompt、summary 正文和 replacement patch 只属于内部 transcript，不得进入 WS；snapshot 中被替换区间只投影无正文的 internal 占位和压缩状态。

### Token 与 Provider profile

- `run_usage`/累计 usage 用于 API 用量统计；`context_usage` 表示最近正常模型响应对应的当前上下文占用，二者不能混用。
- daemon 的 `PUT /v1/providers/{profile_id}` 中 `max_context_tokens` 必填且必须大于零。它是占用统计、自动压缩和未来精简策略的预算依据，不得改回 optional/null。
- profile 的 `provider`、`api_key`、`base_url`、`model` 同样必填。GET 不得返回 API key。
- Provider profile 更新只影响之后创建或重启恢复的 Agent；不得在 live/prepared Agent 中静默热替换 adapter。session 级 model/reasoning 变更走既有 WS 命令与 metadata 顺序。

### Session、actor 与事件

- `/new` 在首个有效 prompt 前只是 prepared Agent：不创建 metadata/transcript 文件，也不出现在 session 列表。断开时必须无残留清理。
- 首 prompt 激活、metadata 创建、storage attach 和入队是一个受控流程；并发首次 attach/恢复同一 session 必须 single-flight。
- prompt 在 running/stopping 时可以按既有上限入 FIFO；配置变更和主动压缩在 busy 时拒绝，不能偷偷排队改变语义。
- command accepted/rejected 是来源 socket 的 direct response；run、Agent、compaction、ask 和状态事件按 sequence 广播给所有 attach。修改 DTO 时保持这个区分。
- 重连依赖最新 snapshot 和后续 sequence，不提供 durable 历史事件 replay。发生广播 lag 时发送完整 resync snapshot，不补造旧事件。
- `askuser` 属于 session actor，不属于某条 WebSocket；断线不能取消它。
- child Agent observer WebSocket 严格只读；任何客户端 text/binary 输入都应拒绝。child 的 progress 可观察但不唤醒父 Agent，blocker/result/failed/closed 通知才进入父 mailbox。

### 持久化与安全

- conversation 使用 `append`/`replace_tail` 为主的 JSONL；局部变化优先调用 `save_incremental` 或 `save_replacing_from`，不要无理由重写完整历史。
- session ID 和文件名必须通过现有校验/编码，防止路径穿越、大小写别名和超长 path component。
- `provider.json` 包含明文 API key，Unix 创建权限保持 `0600`。长期 daemon key 只从 key 文件读取，不放入 URL 或普通明文配置环境变量。
- daemon 默认只监听 loopback，并可成对配置 PEM 证书与私钥启用 TLS。项目本身没有 origin 校验或租户隔离；绑定非本机地址时文档必须明确要求可信前置代理或等效安全边界。

## 默认启用与显式启用

- library 保持可嵌入和最小默认面：内置文件/shell 工具、skills、MCP、subagent 工具和 context compactor 都需要调用方显式安装或配置。
- daemon factory 当前显式安装 `askuser`、默认 compactor，并按 daemon 配置启用 skills 与父 subagent 工具。不要把 daemon 默认行为误写成 library 默认行为。
- child 默认不能再次 spawn child；若未来开放递归，必须先设计独立的深度、总量、资源和关闭传播上限。

## API 与兼容性约束

- Rust 公共 API、HTTP JSON、WS command/event、磁盘记录格式都是兼容性边界。修改前先搜索所有构造点、serde tag、测试 fixture 和 README 示例。
- 新增可选 wire 字段应优先使用向后兼容默认；删除/重命名字段、event type 或状态值需要明确迁移。持久化格式变化必须能读取已有数据，或提供显式版本错误/迁移。
- daemon public DTO 不直接序列化内部对象；继续通过显式 DTO 投影隐藏 credentials、opaque provider state 和内部控制字段。
- 外部输入错误使用 typed error 和稳定的协议 error code，不允许 panic、`unwrap` 或把内部调试细节回传给客户端。
- 改动公共行为时同步更新根 `README.md`；改动 daemon API、状态机、环境变量或持久化时同步更新 `crates/phi-daemon/README.md`。

## Async 与并发约束

- async 路径使用 Tokio 对应能力，避免阻塞 runtime thread。不要在持有普通 mutex guard 时跨 `.await`；确有必要的状态串行化应收敛到 actor 或 Tokio synchronization primitive。
- Provider stream、hooks、tools、compaction 和挂起交互必须继续响应 cooperative stop。新增长任务要有明确的 owner、取消和 shutdown 路径，不能留下失去管理的后台 task。
- 不要通过增加并行度绕过 actor、tool effect 或 capability 约束。并行工具仍需服从 `ToolConcurrency`、副作用分类、超时和 protocol journal。
- 广播、mailbox、队列和 token 池必须有界；新增通道时定义容量、满载行为、关闭语义和相应测试。

## 修改与验证流程

先运行最贴近改动的单元/集成测试；完成后至少执行：

```bash
cargo fmt --all --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
git diff --check
```

涉及以下内容时必须补回归测试：

- tool-call/result 配对、停止或持久化失败恢复；
- Provider 请求映射、SSE terminal event、usage/reasoning 或重试分类；
- compaction threshold、手动/自动/overflow 路径和 replacement patch；
- actor 状态迁移、命令 admission、FIFO、多 attach、sequence/resync；
- HTTP/WS serde 形状、credential redaction 和旧磁盘格式读取；
- path、revision、锁、队列容量和并发上限。

测试不得依赖真实 Provider、外网、用户主目录或已存在的 daemon 数据。使用 scripted provider、内存 store、临时目录和暂停时间控制来保证确定性。

## 文件与提交卫生

- 不提交 `target/`、`.phi/`、`.env`、key、日志、编辑器交换文件或临时测试产物；`.env.example` 只能包含无密钥示例。
- 不要覆盖工作区中与当前任务无关的用户改动。格式化应限定为项目标准工具，避免无意义的大范围手工重排。
- 新文件放入职责对应的模块；不要为了规避边界创建含糊的 `utils` 或跨层依赖。

## Web 样式

- 禁止引入或使用 Tailwind CSS。
- Web 组件样式必须使用 CSS Modules（`*.module.css`）。
