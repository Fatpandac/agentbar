#!/usr/bin/env bash
# agentbar TPM 入口：底部 window 旁显示 agent 状态标记
set -e

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$CURRENT_DIR/bin/agentbar"

# 首次加载时下载预编译二进制
if [ ! -x "$BIN" ]; then
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64)  target=aarch64-apple-darwin ;;
    Linux-x86_64)  target=x86_64-unknown-linux-gnu ;;
    Linux-aarch64) target=aarch64-unknown-linux-gnu ;;
    *) tmux display-message "agentbar: 不支持的平台 $(uname -s)-$(uname -m)"; exit 0 ;;
  esac
  mkdir -p "$CURRENT_DIR/bin"
  curl -fsSL -o "$BIN" \
    "https://github.com/Fatpandac/agentbar/releases/latest/download/agentbar-$target" \
    && chmod +x "$BIN"
fi
[ -x "$BIN" ] || { tmux display-message "agentbar: 下载二进制失败"; exit 0; }

# 按 session 创建顺序循环切换 session；switch-client 会重置 key table，故用专用表实现连续 Tab
tmux bind Tab  run-shell "$BIN next '#{session_name}'" '\;' switch-client -T agentbar
tmux bind BTab run-shell "$BIN prev '#{session_name}'" '\;' switch-client -T agentbar
tmux bind -T agentbar Tab  run-shell "$BIN next '#{session_name}'" '\;' switch-client -T agentbar
tmux bind -T agentbar BTab run-shell "$BIN prev '#{session_name}'" '\;' switch-client -T agentbar

# 底部 window 旁显示该 window 下 agent 的运行状态（重复加载不重复追加）
if ! tmux show -gqv window-status-format | grep -q agentbar; then
  tmux set -ga window-status-format         "#($BIN win '#{window_id}')"
  tmux set -ga window-status-current-format "#($BIN win '#{window_id}')"
fi
