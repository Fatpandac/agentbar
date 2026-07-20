#!/usr/bin/env bash
# agentbar TPM 入口：顶部 session tab 栏 + 底部 window 旁 agent 状态标记
set -e

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$CURRENT_DIR/bin/agentbar"

# 二进制缺失或版本与源码不一致时（TPM git pull 只更新源码）下载对应版本的预编译二进制
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$CURRENT_DIR/Cargo.toml")
if [ ! -x "$BIN" ] || [ "$("$BIN" --version 2>/dev/null)" != "$VERSION" ]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64)  target=aarch64-apple-darwin ;;
    Linux-x86_64)  target=x86_64-unknown-linux-gnu ;;
    Linux-aarch64) target=aarch64-unknown-linux-gnu ;;
    *) tmux display-message "agentbar: 不支持的平台 $(uname -s)-$(uname -m)"; exit 0 ;;
  esac
  mkdir -p "$CURRENT_DIR/bin"
  curl -fsSL -o "$BIN.tmp" \
    "https://github.com/Fatpandac/agentbar/releases/download/v$VERSION/agentbar-$target" \
    && chmod +x "$BIN.tmp" && mv "$BIN.tmp" "$BIN"
fi
[ -x "$BIN" ] || { tmux display-message "agentbar: 下载二进制失败"; exit 0; }

# 可配置项：set -g @agentbar_bg '#2e3b4e'
bg="$(tmux show -gqv @agentbar_bg)"
bg="${bg:-#2e3b4e}"

# 按顶栏显示顺序循环切换 session；用 -r 在 repeat-time 内连续 Tab，超时或按其他键即透传，不吃输入
# 二进制内部 switch-client -t 会重置 key table 打断 repeat，故链上 switch-client -T prefix 恢复
tmux bind -r Tab  run-shell "$BIN next '#{session_name}'" '\;' switch-client -T prefix
tmux bind -r BTab run-shell "$BIN prev '#{session_name}'" '\;' switch-client -T prefix
tmux unbind -T agentbar Tab  2>/dev/null || true
tmux unbind -T agentbar BTab 2>/dev/null || true

# 顶部 session tab 栏
tmux set -g pane-border-status top
tmux set -g pane-border-format \
  "#{?#{&&:#{pane_at_top},#{pane_at_left}},#[bg=$bg]#($BIN '#{session_name}'),}"

# 底部 window 旁显示该 window 下 agent 的运行状态（重复加载不重复追加）
if ! tmux show -gqv window-status-format | grep -q agentbar; then
  tmux set -ga window-status-format         "#($BIN win '#{window_id}')"
  tmux set -ga window-status-current-format "#($BIN win '#{window_id}')"
fi
