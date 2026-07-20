# agentbar

tmux 顶部 session tab 栏，并在底部状态栏每个 window 旁显示该 window 下 AI agent（claude / codex / pi）的实时状态。
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

## 配置

```tmux
# 顶栏 tab 背景色（默认 #2e3b4e）
set -g @agentbar_bg '#2e3b4e'
```

## 要求

- tmux >= 3.0（pane-border-format 支持 `#()`）
- 需保持较高刷新率：`set -g status-interval 1`
