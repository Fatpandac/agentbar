# agentbar

在 tmux 底部状态栏每个 window 旁显示该 window 下 AI agent（claude / codex / pi）的实时状态，并提供 `prefix + Tab` 按创建顺序循环切换 session。
状态判定解析各 agent 的 session JSONL 日志（思路来自 [opensessions](https://github.com/ataraxy-labs/opensessions)），不依赖 CPU 采样。

标记：

- 🔔 agent 在等你确认/输入
- ⚡ agent 正在干活
- 💤 agent 已完成/空闲
- 无标记：该 window 没开 agent

## 安装（TPM）

```tmux
set -g @plugin 'fatpandac/agentbar'
```

然后 `prefix + I` 安装。首次加载自动从 GitHub Release 下载对应平台的预编译二进制（macOS arm64，Linux x86_64/arm64），无需 Rust 环境。

## 要求

- 需保持较高刷新率：`set -g status-interval 1`
