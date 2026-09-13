# agentbar

tmux 顶部 session tab 栏（带该 session 的 agent 状态聚合），并在底部状态栏每个 window 旁显示该 window 下 AI agent（claude / codex / pi）的实时状态。
状态判定解析各 agent 的 session JSONL 日志（思路来自 [opensessions](https://github.com/ataraxy-labs/opensessions)），不依赖 CPU 采样。

顶部 session tab 用整个背景颜色表示状态：

- 琥珀黄 `#E5C07B`：agent 在等你确认/输入
- 青蓝色 `#56B6C2`：agent 正在干活
- 默认灰色：agent 已完成/空闲，或该 session 没有 agent

当前选中的 session 保持绿色背景，不使用下划线；等待和运行状态色优先覆盖绿色。

底部 window 仍使用紧凑标记：

- 🔔 agent 在等你确认/输入
- ⚡ agent 正在干活
- 💤 agent 已完成/空闲
- 无标记：该 window 没开 agent

agent 状态跳变为「执行结束」或「等待输入」时会发系统通知，内容包含具体位置，如 `pi 在 work:2.1 执行结束`（session:window.pane）。仅在有客户端 attach 时生效。

macOS 上装了 [terminal-notifier](https://github.com/julienXX/terminal-notifier)（`brew install terminal-notifier`）时，**点击通知可直接跳到对应的 tmux window/pane**；未安装则退回 osascript（不可点击）。Linux 用 notify-send。

## 安装（TPM）

```tmux
set -g @plugin 'fatpandac/agentbar'
```

然后 `prefix + I` 安装。首次加载自动从 GitHub Release 下载对应平台的预编译二进制（macOS arm64，Linux x86_64/arm64），无需 Rust 环境。

## 切换 session

- `prefix + Tab` / `prefix + Shift+Tab`：按顶栏顺序循环切换（repeat-time 内可连按）
- `Alt/Option + 1..9`：直接跳到顶栏第 N 个 session（插件绑在 root 表，无需 prefix）

### Cmd + 数字（需终端配合）

tmux 收不到 Cmd 键：终端只传字节流，`Ctrl`/`Alt` 有标准编码（`Alt+1` = `Esc 1`），而 macOS 的 Cmd 是应用层修饰键，没有编码，tmux 的 `bind` 也没有 `Super`/`Cmd` 修饰符。
所以要用 `Cmd + 数字`，得让终端把 `Cmd+N` 翻译成 `Esc N`（即 `Alt+N`），再由插件的 `M-1..M-9` 接住。

**Ghostty**（`~/.config/ghostty/config`）：

```
keybind = cmd+digit_1=text:\x1b1
keybind = cmd+digit_2=text:\x1b2
keybind = cmd+digit_3=text:\x1b3
# ... 以此类推到 cmd+digit_9
```

注意必须写 `digit_N`：`cmd+1` 不会覆盖 Ghostty 默认的 `super+digit_1=goto_tab:1`，配置看着生效实则被默认切标签页吃掉。
改完按 `Cmd+Shift+,` reload，用 `ghostty +list-keybinds | grep digit_1` 可确认。

**iTerm2**：Settings → Keys → Key Bindings，给 ⌘1..⌘9 选 “Send Escape Sequence”，内容填 `1`..`9`（需先关掉 Preferences → Keys 里自带的 ⌘数字切标签页）。

**其他终端**：只要能把 `Cmd+N` 发送成 `ESC` + 数字字符即可（WezTerm 用 `action=wezterm.action.SendString("\x1b1")`）。

代价：Cmd+1..9 被占用后不能再切终端自己的标签页。不想改终端配置就用 `Alt/Option + 1..9`（Ghostty 需 `macos-option-as-alt = true`）。

## 配置

```tmux
# 顶栏 tab 背景色（默认 #2e3b4e）
set -g @agentbar_bg '#2e3b4e'
```

## 要求

- tmux >= 3.0（pane-border-format 支持 `#()`）
- 需保持较高刷新率：`set -g status-interval 1`
