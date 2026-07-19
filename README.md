# agentbar

tmux 顶部 session tab 栏，显示每个 session 里 AI agent（claude / codex / pi）的实时状态。
状态判定解析各 agent 的 session JSONL 日志（思路来自 [opensessions](https://github.com/ataraxy-labs/opensessions)），不依赖 CPU 采样。

标记：

- 🔔 agent 在等你确认/输入
- ⚡ agent 正在干活
- 💤 agent 已完成/空闲
- 无标记：该 session 没开 agent

## 安装（TPM）

```tmux
set -g @plugin 'fatpandac/agentbar'
```

然后 `prefix + I` 安装（需要 cargo，首次加载自动编译）。

## 配置

```tmux
# tab 栏背景色（默认 #2e3b4e）
set -g @agentbar_bg '#2e3b4e'
```

## 要求

- tmux >= 3.0（pane-border-format 支持 `#()`）
- Rust toolchain（编译一次）
- 需保持较高刷新率：`set -g status-interval 1`
