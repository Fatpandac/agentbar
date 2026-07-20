//! agentbar: tmux 顶栏 session tab + 底栏每个 window 旁的 agent 状态标记。
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
const CHUNK: u64 = 64 * 1024;

#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
enum Status {
    Done,
    Running,
    Waiting,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Claude,
    Codex,
    Pi,
}

enum Key {
    Path(String),           // codex / pi：日志里的真实 cwd
    ClaudeEncoded(String),  // claude：目录名（编码路径），归一化后比较
}

struct Snap {
    key: Key,
    status: Status,
    kind: Kind,
    born: u64, // session 创建时间（日志首行时间戳，ms），用于把文件认领给具体 agent 进程
}

/// tmux 里正在跑的一个 agent 进程
struct Proc {
    kind: Kind,
    start: u64,    // 进程启动时间（ms）
    cwd: String,   // 所在 pane 的 cwd
    group: String, // 归属分组：window_id（底栏）或 session_name（顶栏）
}

// 认领容差：只容 ps etime 秒级精度的抖动。必须足够小，
// 否则几秒内先后启动的两个同目录 agent 会被后启动者抢走全部文件
const OWN_TOL_MS: u64 = 2_000;

fn main() {
    let current = std::env::args().nth(1).unwrap_or_default();
    match current.as_str() {
        "win" => {
            window_mark(&std::env::args().nth(2).unwrap_or_default());
            return;
        }
        "--version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return;
        }
        "next" | "prev" => {
            navigate(current == "next", &std::env::args().nth(2).unwrap_or_default());
            return;
        }
        _ => {}
    }
    // 默认模式：顶栏 session tab + 该 session 下 agent 状态聚合（同底栏的归属规则，按 session 分组）
    let Ok(out) = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{session_created}\t#{session_name}\t#{pane_pid}\t#{pane_current_path}",
        ])
        .output()
    else {
        return;
    };
    let mut order: Vec<(u64, String)> = Vec::new();
    let mut panes = Vec::new(); // (session, pid, cwd)
    let mut cwds: HashMap<String, Vec<String>> = HashMap::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(4, '\t');
        let (Some(created), Some(session), Some(pid), Some(cwd)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(created), Ok(pid)) = (created.parse::<u64>(), pid.parse::<u32>()) else { continue };
        if !order.iter().any(|(_, s)| s == session) {
            order.push((created, session.to_string()));
        }
        cwds.entry(session.to_string()).or_default().push(cwd.to_string());
        panes.push((session.to_string(), pid, cwd.to_string()));
    }
    order.sort();
    let procs = agent_procs(&panes);
    let snaps = scan_all();
    let mut line = String::new();
    for (_, session) in order {
        let mark = mark(group_status(&cwds[&session], &session, &procs, &snaps));
        let style = if session == current {
            "#[fg=black,bg=green,bold]"
        } else {
            "#[fg=white,bg=colour238]"
        };
        line.push_str(&format!("{style} {session}{mark} #[default]\u{2500}"));
    }
    print!("{line}");
}

/// 按顶栏顺序切换到下一个/上一个 session
fn navigate(forward: bool, current: &str) {
    let Some(names) = tmux_sessions() else { return };
    let Some(i) = names.iter().position(|n| n == current) else { return };
    let target = if forward {
        &names[(i + 1) % names.len()]
    } else {
        &names[(i + names.len() - 1) % names.len()]
    };
    let _ = Command::new("tmux")
        .args(["switch-client", "-t", &format!("={target}")])
        .status();
}

/// 所有 session 名，按创建时间排序：旧的在前，新建的追加在后
fn tmux_sessions() -> Option<Vec<String>> {
    let out = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_created}\t#{session_name}"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut order: Vec<(u64, String)> = text
        .lines()
        .filter_map(|l| {
            let (created, name) = l.split_once('\t')?;
            Some((created.parse().ok()?, name.to_string()))
        })
        .collect();
    order.sort();
    (!order.is_empty()).then(|| order.into_iter().map(|(_, s)| s).collect())
}

fn scan_all() -> Vec<Snap> {
    let now = now_ms();
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let mut snaps = Vec::new();
    scan_claude(&home, now, &mut snaps);
    scan_codex(&home, now, &mut snaps);
    scan_pi(&home, now, &mut snaps);
    snaps
}

fn mark(status: Option<Status>) -> &'static str {
    match status {
        Some(Status::Waiting) => " 🔔",
        Some(Status::Running) => " ⚡",
        Some(Status::Done) => " 💤",
        None => "",
    }
}

/// 输出单个 window 的 agent 状态标记（底部 window-status-format 用）
// ponytail: 每个 window 各起一个进程全量扫描 jsonl；window 很多导致状态栏卡顿时再改成单进程缓存
fn window_mark(window_id: &str) {
    let Ok(out) = Command::new("tmux")
        .args([
            "list-panes",
            "-a",
            "-F",
            "#{window_id}\t#{pane_pid}\t#{pane_current_path}",
        ])
        .output()
    else {
        return;
    };
    let mut panes = Vec::new(); // (window_id, pane_pid, cwd)
    let mut cwds = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(win), Some(pid), Some(cwd)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else { continue };
        if win == window_id {
            cwds.push(cwd.to_string());
        }
        panes.push((win.to_string(), pid, cwd.to_string()));
    }
    let procs = agent_procs(&panes);
    if cwds.is_empty() {
        return;
    }
    print!("{}", mark(group_status(&cwds, window_id, &procs, &scan_all())));
}

/// 分组（window / session）级状态：只统计归属于本组 agent 进程的日志。
/// 归属规则（依次）：
/// 1. 时间认领：文件属于「启动时间 ≤ session 创建时间、且启动最晚」的同类同目录进程；
/// 2. 排除法（resume 场景：文件比所有进程都老）：若同类同目录只剩一个名下无文件的
///    进程和一个无主文件，则配对；
/// 3. 仍认不出（如两个 resume）退回共享显示，不丢状态。
fn group_status(cwds: &[String], group: &str, procs: &[Proc], snaps: &[Snap]) -> Option<Status> {
    let mine_kinds: Vec<Kind> = procs
        .iter()
        .filter(|p| p.group == group)
        .map(|p| p.kind)
        .collect();
    // 第一轮：按时间给每个文件认领进程（全局，不限本 window）
    let owner: Vec<Option<usize>> = snaps
        .iter()
        .map(|snap| {
            procs
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.kind == snap.kind
                        && key_hit(&snap.key, &p.cwd)
                        && p.start <= snap.born + OWN_TOL_MS
                })
                .max_by_key(|(_, p)| p.start)
                .map(|(i, _)| i)
        })
        .collect();
    let mut best: Option<Status> = None;
    for (i, snap) in snaps.iter().enumerate() {
        if !mine_kinds.contains(&snap.kind) || !cwds.iter().any(|c| key_hit(&snap.key, c)) {
            continue;
        }
        let counted = match owner[i] {
            Some(o) => procs[o].group == group,
            None => {
                // 排除法：名下无文件的同类同目录进程
                let free: Vec<usize> = procs
                    .iter()
                    .enumerate()
                    .filter(|(j, p)| {
                        p.kind == snap.kind
                            && key_hit(&snap.key, &p.cwd)
                            && !owner.contains(&Some(*j))
                    })
                    .map(|(j, _)| j)
                    .collect();
                // 同组无主文件数
                let orphans = snaps
                    .iter()
                    .zip(&owner)
                    .filter(|(s, o)| s.kind == snap.kind && o.is_none())
                    .count();
                match free[..] {
                    [f] if orphans == 1 => procs[f].group == group, // 唯一配对
                    _ => true, // 认不出 → 共享显示
                }
            }
        };
        if counted && best.map_or(true, |b| snap.status > b) {
            best = Some(snap.status);
        }
    }
    best
}

/// 遍历所有 pane 的进程树，找出正在跑的 agent 进程（类型 + 启动时间 + pane cwd）
fn agent_procs(panes: &[(String, u32, String)]) -> Vec<Proc> {
    let Ok(out) = Command::new("ps")
        .args(["-eo", "pid=,ppid=,etime=,args="])
        .output()
    else {
        return Vec::new();
    };
    let now = now_ms();
    let text = String::from_utf8_lossy(&out.stdout);
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut info: HashMap<u32, (u64, String)> = HashMap::new(); // pid -> (start_ms, args)
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(pid), Some(ppid), Some(etime)) = (
            it.next().and_then(|s| s.parse().ok()),
            it.next().and_then(|s| s.parse().ok()),
            it.next(),
        ) else {
            continue;
        };
        children.entry(ppid).or_default().push(pid);
        let start = now.saturating_sub(parse_etime(etime) * 1000);
        info.insert(pid, (start, it.collect::<Vec<_>>().join(" ")));
    }
    let mut procs = Vec::new();
    for (group, pane_pid, cwd) in panes {
        let mut stack = vec![*pane_pid];
        while let Some(pid) = stack.pop() {
            if let Some((start, args)) = info.get(&pid) {
                if let Some(kind) = kind_of(args) {
                    procs.push(Proc { kind, start: *start, cwd: cwd.clone(), group: group.clone() });
                }
            }
            if let Some(cs) = children.get(&pid) {
                stack.extend(cs);
            }
        }
    }
    procs
}

/// 解析 ps etime（[[dd-]hh:]mm:ss）为秒
fn parse_etime(s: &str) -> u64 {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse().unwrap_or(0), r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest.split(':').filter_map(|p| p.parse().ok()).collect();
    let secs = match parts[..] {
        [h, m, s] => h * 3600 + m * 60 + s,
        [m, s] => m * 60 + s,
        [s] => s,
        _ => 0,
    };
    days * 86_400 + secs
}

/// 从进程命令行识别 agent：前两个 token 的 basename 命中 claude/codex/pi
/// （兼容 "claude ..." 和 "node /path/to/claude ..." 两种形态）
fn kind_of(args: &str) -> Option<Kind> {
    for tok in args.split_whitespace().take(2) {
        match tok.rsplit('/').next().unwrap_or(tok) {
            "claude" => return Some(Kind::Claude),
            "codex" => return Some(Kind::Codex),
            "pi" => return Some(Kind::Pi),
            _ => {}
        }
    }
    None
}

/// 日志 key 是否命中某个 cwd
fn key_hit(key: &Key, cwd: &str) -> bool {
    match key {
        Key::Path(dir) => cwd == dir || cwd.starts_with(&format!("{dir}/")),
        Key::ClaudeEncoded(enc) => &normalize(cwd) == enc,
    }
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
                snaps.push(Snap {
                    key: Key::ClaudeEncoded(encoded.clone()),
                    status,
                    kind: Kind::Claude,
                    born: head_born(&read_head(&path)),
                });
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
        let head = read_head(&path);
        let Some(cwd) = codex_cwd(&head) else { continue };
        if let Some(status) = codex_status(&read_tail(&path), mtime, now) {
            snaps.push(Snap { key: Key::Path(cwd), status, kind: Kind::Codex, born: head_born(&head) });
        }
    }
}

fn scan_pi(home: &Path, now: u64, snaps: &mut Vec<Snap>) {
    let root = std::env::var_os("PI_CODING_AGENT_SESSION_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".pi/agent/sessions"));
    for path in walk_jsonl(&root) {
        let Some(mtime) = recent_jsonl_mtime(&path, now) else { continue };
        let head = read_head(&path);
        let Some(cwd) = pi_cwd(&head) else { continue };
        if let Some(status) = pi_status(&read_tail(&path), mtime, now) {
            snaps.push(Snap { key: Key::Path(cwd), status, kind: Kind::Pi, born: head_born(&head) });
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

/// session 创建时间：日志头部第一个带 timestamp 的条目（UTC）。
/// 注意不能用文件 birthtime：那是首次写盘时间，可能晚于另一个 agent 启动，导致认领错乱。
/// 拿不到返回 0 → 视为无法认领，退回共享显示
fn head_born(head: &str) -> u64 {
    lines(head)
        .find_map(|e| e.get("timestamp")?.as_str().and_then(ts_ms))
        .unwrap_or(0)
}

/// 解析 ISO8601 UTC（2026-07-20T07:12:50.354Z）为 epoch ms，避免引 chrono
fn ts_ms(s: &str) -> Option<u64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<u64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let ms = if s.as_bytes().get(19) == Some(&b'.') { num(20..23).unwrap_or(0) } else { 0 };
    // days-from-civil（Howard Hinnant 算法）
    let (y, mo, d) = (y as i64, mo as i64, d as i64);
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let doy = (153 * ((mo + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days as u64 * 86_400 + h * 3600 + mi * 60 + sec) * 1000 + ms)
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
    // 注意：不能按闲置时长强降为 Done：长工具调用/长思考期间日志不写盘，
    // 会把正在干活的 agent 误判成闲置；agent 崩溃/退出由进程检测兑底（无进程不显示）
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
    fn claude_tool_use_escalates_to_waiting_and_stays() {
        let raw = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use"}]}}"#;
        assert_eq!(claude_status(raw, 0, 1000), Some(Status::Running));
        assert_eq!(claude_status(raw, 0, 5000), Some(Status::Waiting));
        // 长时间无写入不再强降 Done：可能是长工具调用或一直在等确认
        assert_eq!(claude_status(raw, 0, 20_000), Some(Status::Waiting));
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

    #[test]
    fn ownership_splits_same_dir_agents() {
        let cwds = vec!["/repo".to_string()];
        let procs = vec![
            Proc { kind: Kind::Pi, start: 1_000, cwd: "/repo".into(), group: "me".into() },
            Proc { kind: Kind::Pi, start: 60_000, cwd: "/repo".into(), group: "other".into() },
        ];
        let snap = |status, born| Snap { key: Key::Path("/repo".into()), status, kind: Kind::Pi, born };
        // 我的进程（1s 启动）创建的文件 → 算我的
        assert_eq!(group_status(&cwds, "me", &procs, &[snap(Status::Running, 2_000)]), Some(Status::Running));
        // 另一个 window 的进程（60s 启动）创建的文件 → 不算我的
        assert_eq!(group_status(&cwds, "me", &procs, &[snap(Status::Running, 61_000)]), None);
        // 排除法：我的进程名下已有文件，resume 的无主文件应归唯一空闲的另一方 → 不算我的
        assert_eq!(
            group_status(&cwds, "me", &procs, &[snap(Status::Running, 2_000), snap(Status::Done, 0)]),
            Some(Status::Running)
        );
        // 反过来：对方名下有文件，我空闲 → resume 文件归我
        assert_eq!(
            group_status(&cwds, "me", &procs, &[snap(Status::Running, 61_000), snap(Status::Done, 0)]),
            Some(Status::Done)
        );
        // 两个无主文件（双 resume）认不出 → 共享显示
        assert_eq!(
            group_status(&cwds, "me", &procs, &[snap(Status::Done, 0), snap(Status::Running, 100)]),
            Some(Status::Running)
        );
        // 单进程 + 无主文件（普通 resume）→ 排除法直接归它
        let solo = vec![Proc { kind: Kind::Pi, start: 60_000, cwd: "/repo".into(), group: "me".into() }];
        assert_eq!(group_status(&cwds, "me", &solo, &[snap(Status::Done, 0)]), Some(Status::Done));
        // 我没跑这个类型 → 不显示
        let other = vec![Proc { kind: Kind::Claude, start: 1_000, cwd: "/repo".into(), group: "me".into() }];
        assert_eq!(group_status(&cwds, "me", &other, &[snap(Status::Running, 2_000)]), None);
    }

    #[test]
    fn ts_parse() {
        assert_eq!(ts_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(ts_ms("2026-07-20T07:12:50.354Z"), Some(1_784_531_570_354));
        assert_eq!(ts_ms("bad"), None);
    }

    #[test]
    fn etime_parse() {
        assert_eq!(parse_etime("05:33"), 333);
        assert_eq!(parse_etime("01:02:03"), 3723);
        assert_eq!(parse_etime("2-01:00:00"), 2 * 86_400 + 3600);
    }

    #[test]
    fn kind_detection() {
        assert_eq!(kind_of("claude --resume"), Some(Kind::Claude));
        assert_eq!(kind_of("node /x/bin/pi"), Some(Kind::Pi));
        assert_eq!(kind_of("/usr/local/bin/codex exec"), Some(Kind::Codex));
        assert_eq!(kind_of("vim main.rs"), None);
        assert_eq!(kind_of("pip install x"), None);
    }
}
