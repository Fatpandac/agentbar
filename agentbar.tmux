#!/usr/bin/env bash
# agentbar TPM 入口：顶部 session tab 栏 + agent 状态标记
set -e

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN="$CURRENT_DIR/target/release/agentbar"

# 首次加载时自动编译
if [ ! -x "$BIN" ]; then
  command -v cargo >/dev/null || { tmux display-message "agentbar: 需要 cargo 来编译"; exit 0; }
  cargo build --release --manifest-path "$CURRENT_DIR/Cargo.toml" >/dev/null 2>&1
fi
[ -x "$BIN" ] || exit 0

# 可配置项：set -g @agentbar_bg '#2e3b4e'
bg="$(tmux show -gqv @agentbar_bg)"
bg="${bg:-#2e3b4e}"

# 按顶栏显示顺序循环切换 session
tmux bind -r Tab run-shell "$BIN next '#{session_name}'"
tmux bind -r BTab run-shell "$BIN prev '#{session_name}'"

tmux set -g pane-border-status top
tmux set -g pane-border-format \
  "#{?#{&&:#{pane_at_top},#{pane_at_left}},#[bg=$bg] #($BIN '#{session_name}'),}"
