//! agentbar: tmux 顶栏 session tab + agent 状态标记。
//! 状态判定逻辑移植自 opensessions（解析各 agent 的 session JSONL 日志）：
//!   claude  ~/.claude/projects/<encoded>/*.jsonl
//!   codex   ~/.codex/sessions/**/*.jsonl
//!   pi      ~/.pi/agent/sessions/**/*.jsonl
//! 标记：🔔 等你确认/输入  ⚡ 正在干活  💤 已完成/空闲  （无标记 = 没开 agent）

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const RECENT_MS: u64 = 5 * 60 * 1000; // 只看 5 分钟内活跃的日志（同 opensessions）
const TOOL_WAIT_MS: u64 = 3_000; // Running + 最后是 tool_use + 3s 没动 → 等确认
const STUCK_MS: u64 = 15_000; // Running 15s 没动 → 视为闲置
const CHUNK: u64 = 64 * 1024;

#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
enum Status {
    Done,
    Running,
    Waiting,
}

enum Key {
    Path(String),           // codex / pi：日志里的真实 cwd
    ClaudeEncoded(String),  // claude：目录名（编码路径），归一化后比较
}

struct Snap {
    key: Key,
    status: Status,
}

fn main() {
    let current = std::env::args().nth(1).unwrap_or_default();
    let Some(panes) = tmux_panes() else { return };
    let now = now_ms();
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());

    let mut snaps = Vec::new();
    scan_claude(&home, now, &mut snaps);
    scan_codex(&home, now, &mut snaps);
    scan_pi(&home, now, &mut snaps);

    let mut order = Vec::new();
    let mut cwds: HashMap<String, Vec<String>> = HashMap::new();
    for (session, cwd) in panes {
        if !cwds.contains_key(&session) {
            order.push(session.clone());
        }
        cwds.entry(session).or_default().push(cwd);
    }

    let mut out = String::new();
    for session in order {
        let mark = match session_status(&cwds[&session], &snaps) {
            Some(Status::Waiting) => " 🔔",
            Some(Status::Running) => " ⚡",
            Some(Status::Done) => " 💤",
            None => "",
        };
        let style = if session == current {
            "#[fg=black,bg=green,bold]"
        } else {
            "#[fg=white,bg=colour238]"
        };
        out.push_str(&format!("{style} {session}{mark} #[default] "));
    }
    print!("{out}");
}

fn session_status(cwds: &[String], snaps: &[Snap]) -> Option<Status> {
    let mut best: Option<Status> = None;
    for snap in snaps {
        let hit = cwds.iter().any(|cwd| match &snap.key {
            Key::Path(dir) => cwd == dir || cwd.starts_with(&format!("{dir}/")),
            Key::ClaudeEncoded(enc) => &normalize(cwd) == enc,
        });
        if hit && best.map_or(true, |b| snap.status > b) {
            best = Some(snap.status);
        }
    }
    best
}

fn tmux_panes() -> Option<Vec<(String, String)>> {
    let out = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{session_name}\t#{pane_current_path}"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let panes: Vec<_> = text
        .lines()
        .filter_map(|l| {
            let (s, p) = l.split_once('\t')?;
            Some((s.to_string(), p.to_string()))
        })
        .collect();
    (!panes.is_empty()).then_some(panes)
}

// ---------- 文件扫描 ----------

fn scan_claude(home: &Path, now: u64, snaps: &mut Vec<Snap>) {
    let Ok(projects) = fs::read_dir(home.join(".claude/projects")) else { return };
    for project in projects.flatten() {
        let dir = project.path();
        if !dir.is_dir() {
            continue;
        }
        let encoded = normalize(&project.file_name().to_string_lossy());
        let Ok(files) = fs::read_dir(&dir) else { continue };
        for file in files.flatten() {
            let path = file.path();
            let Some(mtime) = recent_jsonl_mtime(&path, now) else { continue };
            if let Some(status) = claude_status(&read_tail(&path), mtime, now) {
                snaps.push(Snap { key: Key::ClaudeEncoded(encoded.clone()), status });
            }
        }
    }
}

fn scan_codex(home: &Path, now: u64, snaps: &mut Vec<Snap>) {
    let root = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("sessions");
    for path in walk_jsonl(&root) {
        let Some(mtime) = recent_jsonl_mtime(&path, now) else { continue };
        let Some(cwd) = codex_cwd(&read_head(&path)) else { continue };
        if let Some(status) = codex_status(&read_tail(&path), mtime, now) {
            snaps.push(Snap { key: Key::Path(cwd), status });
        }
    }
}

fn scan_pi(home: &Path, now: u64, snaps: &mut Vec<Snap>) {
    let root = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".pi/agent/sessions"));
    for path in walk_jsonl(&root) {
        let Some(mtime) = recent_jsonl_mtime(&path, now) else { continue };
        let Some(cwd) = pi_cwd(&read_head(&path)) else { continue };
        if let Some(status) = pi_status(&read_tail(&path), mtime, now) {
            snaps.push(Snap { key: Key::Path(cwd), status });
        }
    }
}

// ponytail: 每次全量递归遍历目录，mtime 过滤兜底；目录多到变慢时按日期目录剪枝
fn walk_jsonl(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else { return out };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_jsonl(&path));
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
    out
}

fn recent_jsonl_mtime(path: &Path, now: u64) -> Option<u64> {
    if path.extension()? != "jsonl" {
        return None;
    }
    let mtime = path
        .metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    (now.saturating_sub(mtime) <= RECENT_MS).then_some(mtime)
}

/// 文件头 64KB（拿 cwd 等元信息，去掉末尾不完整行）
fn read_head(path: &Path) -> String {
    let Ok(file) = fs::File::open(path) else { return String::new() };
    let mut buf = Vec::new();
    let _ = file.take(CHUNK).read_to_end(&mut buf);
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if let Some(i) = text.rfind('\n') {
        text.truncate(i);
    }
    text
}

/// 文件尾 64KB（判状态，去掉开头不完整行）
fn read_tail(path: &Path) -> String {
    let Ok(mut file) = fs::File::open(path) else { return String::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len <= CHUNK {
        let mut text = String::new();
        let _ = file.read_to_string(&mut text);
        return text;
    }
    let _ = file.seek(SeekFrom::End(-(CHUNK as i64)));
    let mut buf = Vec::new();
    let _ = file.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    match text.find('\n') {
        Some(i) => text[i + 1..].to_string(),
        None => String::new(),
    }
}

fn normalize(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn lines(raw: &str) -> impl Iterator<Item = Value> + '_ {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
}

// ---------- claude ----------

const CLAUDE_NOISE: &[&str] = &[
    "<local-command-caveat>",
    "<local-command-stdout>",
    "<local-command-stderr>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
    "<system-reminder>",
    "<task-notification>",
];

fn claude_status(raw: &str, mtime: u64, now: u64) -> Option<Status> {
    let mut status = None;
    let mut last_tool_use = false;
    for entry in lines(raw) {
        if let Some(s) = det_claude(&entry) {
            status = Some(s);
        }
        last_tool_use = entry.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
            && has_type(entry.pointer("/message/content"), "tool_use");
    }
    finalize(status?, last_tool_use, mtime, now)
}

fn det_claude(entry: &Value) -> Option<Status> {
    let msg = entry.get("message")?;
    let content = msg.get("content");
    match msg.get("role").and_then(Value::as_str)? {
        "assistant" => {
            if has_type(content, "tool_use") || has_type(content, "thinking") {
                return Some(Status::Running);
            }
            match msg.get("stop_reason").and_then(Value::as_str) {
                None | Some("tool_use") => Some(Status::Running),
                Some(_) => Some(Status::Done),
            }
        }
        "user" => {
            if let Some(text) = text_of(content) {
                if text.starts_with("[Request interrupted")
                    || text.contains("<command-name>/exit</command-name>")
                {
                    return Some(Status::Done);
                }
                if text.contains("<command-name>/")
                    || CLAUDE_NOISE.iter().any(|p| text.starts_with(p))
                {
                    return None;
                }
            }
            Some(Status::Running)
        }
        _ => None,
    }
}

// ---------- codex ----------

fn codex_cwd(head: &str) -> Option<String> {
    lines(head).find_map(|e| {
        matches!(
            e.get("type").and_then(Value::as_str),
            Some("session_meta" | "turn_context")
        )
        .then(|| e.pointer("/payload/cwd")?.as_str().map(String::from))
        .flatten()
    })
}

fn codex_status(raw: &str, mtime: u64, now: u64) -> Option<Status> {
    let mut status = None;
    let mut last_tool_call = false;
    for entry in lines(raw) {
        if let Some(s) = det_codex(&entry) {
            status = Some(s);
            last_tool_call = entry.get("type").and_then(Value::as_str) == Some("function_call")
                || (entry.get("type").and_then(Value::as_str) == Some("response_item")
                    && entry.pointer("/payload/type").and_then(Value::as_str)
                        == Some("function_call"));
        }
    }
    finalize(status?, last_tool_call, mtime, now)
}

fn det_codex(entry: &Value) -> Option<Status> {
    match entry.get("type").and_then(Value::as_str) {
        Some("event_msg") => match entry.pointer("/payload/type").and_then(Value::as_str) {
            Some("task_complete" | "turn_aborted") => Some(Status::Done),
            Some("task_started" | "user_message") => Some(Status::Running),
            Some("agent_message") => {
                match entry.pointer("/payload/phase").and_then(Value::as_str) {
                    Some("final_answer") => Some(Status::Done),
                    _ => Some(Status::Running),
                }
            }
            _ => None,
        },
        Some("response_item") => match entry.pointer("/payload/type").and_then(Value::as_str) {
            Some("message") => match entry.pointer("/payload/role").and_then(Value::as_str) {
                Some("user") => Some(Status::Running),
                Some("assistant") => {
                    match entry.pointer("/payload/phase").and_then(Value::as_str) {
                        Some("final_answer") => Some(Status::Done),
                        _ => Some(Status::Running),
                    }
                }
                _ => None,
            },
            Some(
                "function_call" | "function_call_output" | "reasoning" | "custom_tool_call"
                | "custom_tool_call_output" | "web_search_call",
            ) => Some(Status::Running),
            _ => None,
        },
        Some("message") => match entry.get("role").and_then(Value::as_str) {
            Some("user" | "assistant") => Some(Status::Running),
            _ => None,
        },
        Some("function_call" | "function_call_output" | "reasoning") => Some(Status::Running),
        _ => None,
    }
}

// ---------- pi ----------

fn pi_cwd(head: &str) -> Option<String> {
    lines(head).find_map(|e| {
        (e.get("type").and_then(Value::as_str) == Some("session"))
            .then(|| e.get("cwd")?.as_str().map(String::from))
            .flatten()
    })
}

fn pi_status(raw: &str, mtime: u64, now: u64) -> Option<Status> {
    let mut status = None;
    for entry in lines(raw) {
        if let Some(s) = det_pi(&entry) {
            status = Some(s);
        }
    }
    finalize(status?, false, mtime, now)
}

fn det_pi(entry: &Value) -> Option<Status> {
    if entry.get("type").and_then(Value::as_str) != Some("message") {
        return None;
    }
    match entry.pointer("/message/role").and_then(Value::as_str)? {
        "user" | "toolResult" => Some(Status::Running),
        "assistant" => match entry.pointer("/message/stopReason").and_then(Value::as_str) {
            Some("toolUse") => Some(Status::Running),
            Some("stop" | "error" | "cancelled" | "aborted" | "interrupted") => Some(Status::Done),
            _ => Some(Status::Waiting),
        },
        _ => None,
    }
}

// ---------- 公共 ----------

fn finalize(mut status: Status, last_tool: bool, mtime: u64, now: u64) -> Option<Status> {
    let idle = now.saturating_sub(mtime);
    if status == Status::Running && last_tool && idle >= TOOL_WAIT_MS {
        status = Status::Waiting;
    }
    if matches!(status, Status::Running | Status::Waiting) && idle >= STUCK_MS {
        status = Status::Done;
    }
    Some(status)
}

fn has_type(content: Option<&Value>, target: &str) -> bool {
    content.and_then(Value::as_array).is_some_and(|items| {
        items
            .iter()
            .any(|i| i.get("type").and_then(Value::as_str) == Some(target))
    })
}

fn text_of(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(t) => Some(t.clone()),
        Value::Array(items) => items
            .iter()
            .find(|i| i.get("type").and_then(Value::as_str) == Some("text"))
            .and_then(|i| i.get("text").and_then(Value::as_str))
            .map(String::from),
        _ => None,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pi_running_then_done() {
        let running = r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"go"}]}}"#;
        assert_eq!(pi_status(running, 0, 1000), Some(Status::Running));
        let done = format!(
            "{running}\n{}",
            r#"{"type":"message","message":{"role":"assistant","stopReason":"stop","content":[]}}"#
        );
        assert_eq!(pi_status(&done, 0, 1000), Some(Status::Done));
    }

    #[test]
    fn claude_tool_use_escalates_to_waiting_then_stale() {
        let raw = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use"}]}}"#;
        assert_eq!(claude_status(raw, 0, 1000), Some(Status::Running));
        assert_eq!(claude_status(raw, 0, 5000), Some(Status::Waiting));
        assert_eq!(claude_status(raw, 0, 20_000), Some(Status::Done));
    }

    #[test]
    fn codex_task_complete_is_done() {
        let raw = r#"{"type":"event_msg","payload":{"type":"task_started"}}
{"type":"event_msg","payload":{"type":"task_complete"}}"#;
        assert_eq!(codex_status(raw, 0, 1000), Some(Status::Done));
    }

    #[test]
    fn cwd_extraction() {
        assert_eq!(
            pi_cwd(r#"{"type":"session","id":"x","cwd":"/repo"}"#).as_deref(),
            Some("/repo")
        );
        assert_eq!(
            codex_cwd(r#"{"type":"session_meta","payload":{"cwd":"/repo"}}"#).as_deref(),
            Some("/repo")
        );
        assert_eq!(normalize("/Users/me/.dir_x"), "-Users-me--dir-x");
    }
}
