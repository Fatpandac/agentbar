#!/usr/bin/env bash
# agentbar TPM 入口：顶部 session tab 栏 + agent 状态标记
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

# 可配置项：set -g @agentbar_bg '#2e3b4e'
bg="$(tmux show -gqv @agentbar_bg)"
bg="${bg:-#2e3b4e}"

# 按顶栏显示顺序循环切换 session
tmux bind -r Tab run-shell "$BIN next '#{session_name}'"
tmux bind -r BTab run-shell "$BIN prev '#{session_name}'"

tmux set -g pane-border-status top
tmux set -g pane-border-format \
  "#{?#{&&:#{pane_at_top},#{pane_at_left}},#[bg=$bg] #($BIN '#{session_name}'),}"
