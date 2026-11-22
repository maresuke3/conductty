use std::time::Duration;

use crate::grid::GridSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Antigravity,
    ClaudeCode,
    Codex,
    GeminiCli,
    Aider,
    Copilot,
    OpenCode,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Antigravity => "Antigravity",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::GeminiCli => "Gemini CLI",
            Self::Aider => "Aider",
            Self::Copilot => "Copilot",
            Self::OpenCode => "OpenCode",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Ask,
    Running,
    Waiting,
    Done,
}

impl AgentStatus {
    pub fn badge(self) -> &'static str {
        match self {
            Self::Ask => "ASK",
            Self::Running => "RUN",
            Self::Waiting => "WAIT",
            Self::Done => "DONE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneAgentState {
    pub kind: AgentKind,
    pub status: AgentStatus,
    pub summary: String,
    pub last_hash: u64,
    pub last_changed_ms: u64,
}

pub const AGENT_RUNNING_GRACE: Duration = Duration::from_millis(2500);

/// `GridSnapshot` から空行を除いたトリム済み行一覧と FNV-1a 64bit ハッシュを取り出す。
pub fn snapshot_visible_lines(snap: &GridSnapshot) -> (Vec<String>, u64) {
    let mut lines = Vec::new();
    let mut hash: u64 = 0xcbf29ce484222325;
    for row in 0..snap.rows {
        let start = row * snap.cols;
        let end = (start + snap.cols).min(snap.cells.len());
        if start >= end {
            continue;
        }
        let mut s = String::with_capacity(snap.cols);
        for cell in &snap.cells[start..end] {
            let ch = if cell.ch == '\0' { ' ' } else { cell.ch };
            s.push(ch);
        }
        let trimmed = s.trim_end();
        if !trimmed.is_empty() {
            for b in trimmed.as_bytes() {
                hash ^= u64::from(*b);
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash ^= 0x0a;
            hash = hash.wrapping_mul(0x100000001b3);
            lines.push(trimmed.trim_start().to_string());
        }
    }
    (lines, hash)
}

fn detect_explicit_agent_kind(lines: &[String]) -> Option<AgentKind> {
    let joined = lines.join("\n").to_lowercase();
    if joined.contains("antigravity") {
        return Some(AgentKind::Antigravity);
    }
    if joined.contains("openai codex")
        || joined.contains("codex>")
        || joined.contains("codex cli")
        || joined.contains("codex v")
        || joined.contains("codex (")
        || joined.contains("context left")
        || joined.contains("tab to queue")
        || joined.contains("reasoning effort")
    {
        return Some(AgentKind::Codex);
    }
    if joined.contains("gemini cli")
        || joined.contains("gemini>")
        || joined.contains("type your message or @path/to/file")
    {
        return Some(AgentKind::GeminiCli);
    }
    if joined.contains("opencode>") || joined.contains("opencode cli") {
        return Some(AgentKind::OpenCode);
    }
    if joined.contains("aider>") || joined.contains("aider v") {
        return Some(AgentKind::Aider);
    }
    if joined.contains("github copilot") || joined.contains("copilot>") {
        return Some(AgentKind::Copilot);
    }
    if joined.contains("claude code")
        || joined.contains("bypass permissions")
        || joined.contains("accept edits on")
    {
        return Some(AgentKind::ClaudeCode);
    }
    None
}

fn detect_fallback_agent_kind(lines: &[String]) -> Option<AgentKind> {
    let joined = lines.join("\n").to_lowercase();
    if joined.contains("allow for this session")
        || joined.contains("? for shortcuts")
        || joined.contains("esc to interrupt")
    {
        return Some(AgentKind::ClaudeCode);
    }
    None
}

fn contains_spinner(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c,
            '⠋' | '⠙'
                | '⠹'
                | '⠸'
                | '⠼'
                | '⠴'
                | '⠦'
                | '⠧'
                | '⠇'
                | '⠏'
                | '⣾'
                | '⣽'
                | '⣻'
                | '⢿'
                | '⡿'
                | '⣟'
                | '⣯'
                | '⣷'
                | '◐'
                | '◓'
                | '◑'
                | '◒'
        )
    })
}

fn is_ask_line(lower: &str) -> bool {
    lower.contains("[y/n]")
        || lower.contains("(y/n)")
        || lower.contains("[y/n/a]")
        || lower.contains("allow once")
        || lower.contains("allow for this session")
        || lower.contains("do you want to proceed")
        || lower.contains("do you want to")
        || lower.contains("press enter to continue")
        || lower.contains("select an option")
        || lower.contains("waiting for your")
}

fn is_running_line(raw: &str, lower: &str) -> bool {
    contains_spinner(raw)
        || lower.contains("esc to interrupt")
        || lower.contains("ctrl+c to cancel")
        || lower.contains("ctrl+c to interrupt")
        || lower.contains("thinking...")
        || lower.contains("working...")
}

fn is_done_line(lower: &str) -> bool {
    lower.contains("task completed")
        || lower.contains("completed successfully")
        || lower.contains("all done")
}

fn contains_tui_box_char(line: &str) -> bool {
    line.chars().any(|c| {
        matches!(
            c,
            '│' | '┃' | '─' | '━' | '╭' | '╮' | '╰' | '╯' | '┌' | '┐' | '└' | '┘'
        )
    })
}

fn is_shell_prompt(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() || is_agent_prompt(t) || contains_tui_box_char(t) {
        return false;
    }
    // `50%` や `100%` などのパーセンテージ表記を zsh プロンプトの `%` と誤認しない
    if let Some(before) = t.strip_suffix('%') {
        if before.chars().last().is_some_and(|c| c.is_ascii_digit()) {
            return false;
        }
        return before.is_empty() || before.ends_with(' ');
    }
    if let Some(before) = t.strip_suffix('$') {
        return before.is_empty() || before.ends_with(' ') || before.ends_with(':');
    }
    if let Some(before) = t.strip_suffix('#') {
        return before.is_empty() || before.ends_with(' ');
    }
    // Starship 等のプロンプト (`~/work $`, `at 10:08:11 AM ❯`, `user@host ❯ cmd`)
    if let Some(idx) = t.find(" ❯") {
        let prefix = &t[..idx];
        return !prefix.is_empty() && (t.ends_with('❯') || t[idx..].starts_with(" ❯ "));
    }
    false
}

fn is_agent_prompt(line: &str) -> bool {
    let t = line.trim();
    t == ">"
        || t == "❯"
        || t.starts_with("> ")
        || t.starts_with("❯ ")
        || t.starts_with("codex>")
        || t.starts_with("aider>")
        || t.starts_with("gemini>")
}

fn truncate_summary(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn has_active_agent_ui(lines: &[String]) -> bool {
    let tail_start = lines.len().saturating_sub(6);
    lines[tail_start..].iter().any(|line| {
        let lower = line.to_lowercase();
        is_ask_line(&lower)
            || is_running_line(line, &lower)
            || is_agent_prompt(line)
            || lower.contains("? for shortcuts")
    })
}

/// ペインの表示行から、現在稼働中の AI Agent 種別とステータス（Ask / Running / Waiting / Done）を判定する。
/// Agent が起動していない、または `/quit` 等でシェルプロンプトへ戻った場合は `None` を返す。
pub fn detect_pane_agent(
    lines: &[String],
    text_hash: u64,
    now_ms: u64,
    prev: Option<&PaneAgentState>,
) -> Option<PaneAgentState> {
    if lines.is_empty() {
        return None;
    }

    // 末尾がシェルプロンプトに戻っている（または直近で /quit / /exit 実行済み）なら停止扱いとして非表示にする
    let tail_start = lines.len().saturating_sub(6);
    let tail = &lines[tail_start..];
    if let Some(last) = tail.last() {
        let lower_last = last.to_lowercase();
        if is_shell_prompt(last)
            || tail
                .iter()
                .rev()
                .take(3)
                .any(|l| l.trim() == "/quit" || l.trim() == "/exit")
            || lower_last.contains("goodbye")
        {
            return None;
        }
    }

    // 最後のシェルプロンプト行より後の範囲だけを見て過去の出力による誤検知を防ぐ
    let active_start = lines
        .iter()
        .rposition(|l| is_shell_prompt(l))
        .map(|i| i + 1)
        .unwrap_or(0);
    let active_lines = &lines[active_start..];
    if active_lines.is_empty() {
        return None;
    }

    let kind = detect_explicit_agent_kind(active_lines)
        .or_else(|| prev.filter(|_| has_active_agent_ui(active_lines)).map(|p| p.kind))
        .or_else(|| detect_fallback_agent_kind(active_lines))?;

    let last_changed_ms = match prev {
        Some(p) if p.kind == kind && p.last_hash == text_hash => p.last_changed_ms,
        _ => now_ms,
    };

    let active_tail_start = active_lines.len().saturating_sub(6);
    let active_tail = &active_lines[active_tail_start..];

    let mut status = None;
    let mut summary_line: Option<&str> = None;

    for line in active_tail.iter().rev() {
        let lower = line.to_lowercase();
        if is_ask_line(&lower) {
            status = Some(AgentStatus::Ask);
            summary_line = Some(line.as_str());
            break;
        }
    }

    if status.is_none() {
        for line in active_tail.iter().rev() {
            let lower = line.to_lowercase();
            if is_running_line(line, &lower) {
                status = Some(AgentStatus::Running);
                summary_line = Some(line.as_str());
                break;
            }
        }
    }

    if status.is_none() {
        if let Some(last) = active_tail.last() {
            let lower = last.to_lowercase();
            if is_done_line(&lower) {
                status = Some(AgentStatus::Done);
            } else if is_agent_prompt(last) {
                status = Some(AgentStatus::Waiting);
            }
        }
    }

    if status.is_none() {
        let recently_active = prev.is_some_and(|p| {
            p.kind == kind
                && (text_hash != p.last_hash
                    || now_ms.saturating_sub(last_changed_ms)
                        < AGENT_RUNNING_GRACE.as_millis() as u64)
        });
        status = Some(if recently_active {
            AgentStatus::Running
        } else {
            AgentStatus::Waiting
        });
    }

    let summary = summary_line
        .or_else(|| active_tail.last().map(String::as_str))
        .map(|s| truncate_summary(s, 44))
        .unwrap_or_default();

    Some(PaneAgentState {
        kind,
        status: status.unwrap_or(AgentStatus::Waiting),
        summary,
        last_hash: text_hash,
        last_changed_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_shell_mentioning_agent_words_is_ignored() {
        let lines = vec![
            "~/work/claude-demo $ ls".to_string(),
            "gemini.md  codex.txt".to_string(),
            "~/work/claude-demo $".to_string(),
        ];
        assert_eq!(detect_pane_agent(&lines, 10, 1000, None), None);
    }

    #[test]
    fn detects_claude_ask_status_with_priority() {
        let lines = vec![
            "Claude Code v1.0".to_string(),
            "Allow once? [y/n]".to_string(),
        ];
        let state = detect_pane_agent(&lines, 100, 1000, None).unwrap();
        assert_eq!(state.kind, AgentKind::ClaudeCode);
        assert_eq!(state.status, AgentStatus::Ask);
        assert_eq!(state.summary, "Allow once? [y/n]");
    }

    #[test]
    fn removes_agent_when_stopped_and_returned_to_shell() {
        let running_lines = vec![
            "OpenAI Codex".to_string(),
            "⠋ Thinking about layout...".to_string(),
        ];
        let running = detect_pane_agent(&running_lines, 200, 1000, None).unwrap();
        assert_eq!(running.kind, AgentKind::Codex);
        assert_eq!(running.status, AgentStatus::Running);

        // /quit 等でシェルプロンプトへ戻ったら None になりリストから消える
        let exited_lines = vec![
            "OpenAI Codex".to_string(),
            "/quit".to_string(),
            "~/work $".to_string(),
        ];
        assert_eq!(detect_pane_agent(&exited_lines, 201, 5000, Some(&running)), None);
    }

    #[test]
    fn detects_gemini_waiting_prompt() {
        let lines = vec!["Gemini CLI".to_string(), "> ".to_string()];
        let state = detect_pane_agent(&lines, 300, 1000, None).unwrap();
        assert_eq!(state.kind, AgentKind::GeminiCli);
        assert_eq!(state.status, AgentStatus::Waiting);
    }

    #[test]
    fn tui_percent_and_box_prompt_do_not_drop_active_agent() {
        let lines = vec![
            "Claude Code v1.0".to_string(),
            "│ ❯ fix the bug".to_string(),
            "? for shortcuts   context 45%".to_string(),
        ];
        let state = detect_pane_agent(&lines, 400, 1000, None).unwrap();
        assert_eq!(state.kind, AgentKind::ClaudeCode);
        assert_eq!(state.status, AgentStatus::Waiting);
    }

    #[test]
    fn detects_antigravity_even_with_shared_shortcuts_footer() {
        let lines = vec![
            "Antigravity CLI".to_string(),
            "⠋ Thinking...".to_string(),
            "? for shortcuts".to_string(),
        ];
        let state = detect_pane_agent(&lines, 500, 1000, None).unwrap();
        assert_eq!(state.kind, AgentKind::Antigravity);
        assert_eq!(state.status, AgentStatus::Running);
    }

    #[test]
    fn detects_codex_tui_and_preserves_kind_when_banner_scrolls() {
        let lines = vec![
            "╭─ codex (gpt-5)".to_string(),
            "100% context left · tab to queue".to_string(),
        ];
        let initial = detect_pane_agent(&lines, 600, 1000, None).unwrap();
        assert_eq!(initial.kind, AgentKind::Codex);

        // バナーがスクロールアウトしても、UI フッターがアクティブなら Codex のまま維持する
        let scrolled = vec![
            "Updated src/main.rs".to_string(),
            "› ".to_string(),
            "esc to interrupt".to_string(),
        ];
        let continued = detect_pane_agent(&scrolled, 601, 2000, Some(&initial)).unwrap();
        assert_eq!(continued.kind, AgentKind::Codex);
        assert_eq!(continued.status, AgentStatus::Running);
    }
}
