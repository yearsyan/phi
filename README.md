# phi

`phi` 是一个受 [Pi agent-core](https://github.com/earendil-works/pi/tree/main/packages/agent) 启发的 Rust Agent。Agent core 只处理规范化消息、工具调用和生成配置；模型、鉴权、URL 与厂商 wire format 全部封装在 provider adapter 中。

## 快速运行
需要常驻进程和多客户端会话管理时，使用 workspace 内的 daemon：

```bash
cargo run -p phi-daemon
```

未设置 `PHI_DAEMON_AUTH_KEY_FILE` 时，首次启动会安全生成
`$HOME/.phi/daemon/auth.key`。在另一个终端读取该 key 后即可调用 API：

daemon 在交互式终端中默认显示包含连接地址和长期 key 的 App 连接二维码；请像 key 文件
一样保护它。同一局域网中的手机直连使用 `cargo run -p phi-daemon -- --lan`，daemon 会
监听全部 IPv4 接口并优先把 `192.168/10/172.16–31` 私网地址写入二维码；找不到私网地址
时选择一个非 loopback 的本机 IPv4，仍不可用才回退 `127.0.0.1`。这会扩大网络暴露面。
可用 `--no-qr` 关闭二维码，非终端 stderr 会自动跳过。

Web 客户端把长期 key 存于同源 `localStorage`，可跨 tab 和浏览器重启保留；页面中的
JavaScript 能读取它，因此仍应视为高权限凭据。Flutter 客户端通过平台 secure storage
（Android EncryptedSharedPreferences / iOS·macOS Keychain / Windows DPAPI /
OpenHarmony AES）保存，不再写入明文 SharedPreferences。
macOS Keychain 访问显式禁止认证 UI；无法无交互读取的旧 ad-hoc 签名条目会被视为缺失，
避免应用启动时弹出系统密码框。

Web 客户端为页面提供可深链接路由：新会话使用 `/sessions/new`，具体会话使用
`/sessions/{session_id}`，定时任务使用 `/scheduled-tasks`；刷新以及浏览器前进、后退都会
恢复对应页面。

Web 与 Flutter 的会话列表只在会话存在 active run、正在生成时显示状态点；空闲、
离线或仅已加载的会话不显示状态点。

向 GitHub 推送 `v**` tag 会发布内嵌 Web 客户端的 daemon ZIP，覆盖 macOS ARM64、
Windows x86_64、Linux x86_64 和 Linux ARM64；普通 branch push 不触发发布 workflow。
macOS daemon Release 产物要求 Developer ID 签名和 Apple notarization。

定时任务的 Telegram 通知使用 Rich Markdown，可原生展示标题、加粗、列表和表格；格式被
Telegram 拒绝时会回退为纯文本，避免通知因模型生成的 Markdown 不完整而丢失。

daemon 默认从 `~/.phi/skills` 发现全局 skills；可通过
`PHI_DAEMON_GLOBAL_SKILLS_DIRS` 覆盖为操作系统原生 path-list。

Provider 层的瞬时失败有界恢复：响应头超时按独立预算默认重试 1 次（只可能重复计费，
不会重复工具副作用）；模型返回协议完整但 tool arguments 非法 JSON 的响应时，agent 会
把该调用与一条合成 error tool result 配对持久化并喂回模型自修复，每个 turn 最多 2 次，
超过上限才失败 run。

Provider 配置、HTTP/WS 协议和停止语义见
[`crates/phi-daemon/README.md`](crates/phi-daemon/README.md)。
