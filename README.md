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

Web 客户端把长期 key 存于 `sessionStorage`（关闭 tab 即清除），Flutter 客户端通过平台
secure storage（Android EncryptedSharedPreferences / iOS·macOS Keychain / Windows DPAPI /
OpenHarmony AES）保存，不再写入明文 SharedPreferences。

定时任务的 Telegram 通知使用 Rich Markdown，可原生展示标题、加粗、列表和表格；格式被
Telegram 拒绝时会回退为纯文本，避免通知因模型生成的 Markdown 不完整而丢失。

daemon 默认从 `~/.phi/skills` 发现全局 skills；可通过
`PHI_DAEMON_GLOBAL_SKILLS_DIRS` 覆盖为操作系统原生 path-list。

Provider 配置、HTTP/WS 协议和停止语义见
[`crates/phi-daemon/README.md`](crates/phi-daemon/README.md)。
