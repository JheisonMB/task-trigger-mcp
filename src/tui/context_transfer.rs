//! Context Transfer — capture and inject conversation context between agents.
//!
//! Builds a plain-text context block from the source agent's prompt history,
//! then drives the two-step TUI modal (preview → agent picker).
//! The transfer includes everything from the selected prompt number through
//! the most recent output — no separate scrollback excerpt.

use std::collections::VecDeque;

use super::agent::{InteractiveAgent, PromptEntry};

// ── Config ───────────────────────────────────────────────────────

/// Runtime defaults for context transfer (no external config file required).
pub struct ContextTransferConfig {
    pub default_prompt_history: usize,
}

impl Default for ContextTransferConfig {
    fn default() -> Self {
        Self {
            default_prompt_history: 3,
        }
    }
}

// ── Context builder ──────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextSourceKind {
    Interactive,
    Terminal,
}

impl ContextSourceKind {
    fn header(self, agent: &InteractiveAgent) -> String {
        match self {
            Self::Interactive => {
                format!(
                    "--- context from: {} | workdir: {} ---\n",
                    agent.name, agent.working_dir
                )
            }
            Self::Terminal => {
                format!(
                    "--- context from terminal: {} | workdir: {} ---\n",
                    agent.name, agent.working_dir
                )
            }
        }
    }
}

pub fn build_context_payload_for(
    agent: &InteractiveAgent,
    n_prompts: usize,
    source_kind: ContextSourceKind,
) -> String {
    let mut out = source_kind.header(agent);

    match source_kind {
        ContextSourceKind::Interactive => append_interactive_context(&mut out, agent, n_prompts),
        ContextSourceKind::Terminal => append_terminal_context(&mut out, agent, n_prompts),
    }

    out.push_str("--- end context ---\n");
    clean_context_output(&out)
}

/// Post-process context payload: collapse blank runs, strip status-bar noise.
fn clean_context_output(raw: &str) -> String {
    let mut result = Vec::new();
    let mut blank_run = 0u8;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                result.push(String::new());
            }
            continue;
        }
        blank_run = 0;

        // Skip common TUI status-bar / sidebar artifacts
        if is_status_noise(trimmed) {
            continue;
        }
        result.push(line.to_string());
    }

    // Trim trailing blank lines before the footer
    while result.last().is_some_and(|l| l.trim().is_empty()) {
        result.pop();
    }

    let mut out = result.join("\n");
    out.push('\n');
    out
}

/// Lines that are TUI chrome / sidebar noise in CLI agents.
fn is_status_noise(line: &str) -> bool {
    // Token/cost counters
    if line.ends_with("tokens") || line.ends_with("used") || line.ends_with("spent") {
        return line.chars().any(|c| c.is_ascii_digit());
    }
    // OpenCode/Claude/Copilot status bar fragments
    let noise = [
        "ctrl+p commands",
        "ctrl+p ",
        "for shortcuts",
        "Shift+Tab",
        "MCP issues",
        "MCP servers",
        "workspace (",
        "Environment",
        "remaining",
        "LSPs will activate",
    ];
    if noise.iter().any(|n| line.contains(n)) {
        return true;
    }
    // Lines that are just "Context" or "LSP" headers from sidebars
    if matches!(line, "Context" | "LSP" | "MCP" | "Build" | "Sessions") {
        return true;
    }
    // File stat lines like "prompt.txt  -46" or "src/foo.rs  +150 -58"
    if line.contains('+') && line.contains('-') && line.chars().filter(|c| *c == ' ').count() >= 2 {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2
            && parts
                .last()
                .is_some_and(|p| p.starts_with('-') || p.starts_with('+'))
        {
            return true;
        }
    }
    false
}

fn collect_last_prompts(history: &VecDeque<PromptEntry>, n: usize) -> Vec<PromptEntry> {
    let keep = n.max(1);
    history
        .iter()
        .skip(history.len().saturating_sub(keep))
        .cloned()
        .collect()
}

fn append_interactive_context(out: &mut String, agent: &InteractiveAgent, n_prompts: usize) {
    let prompt_history = agent
        .prompt_history
        .lock()
        .ok()
        .as_deref()
        .cloned()
        .unwrap_or_default();
    let prompts = collect_last_prompts(&prompt_history, n_prompts);
    if prompts.is_empty() {
        return;
    }

    let total_depth = agent.total_depth();
    for (idx, entry) in prompts.iter().enumerate() {
        out.push_str(&format!("> {}\n", entry.input));

        let is_last_prompt = idx + 1 == prompts.len();
        let response_end = response_end_line(entry, is_last_prompt, total_depth);
        if response_end <= entry.output_range.0 {
            continue;
        }

        let response = agent.lines_at_scrollback_range(entry.output_range.0, response_end);
        if response.is_empty() {
            continue;
        }

        out.push_str(&response);
        out.push('\n');
    }
}

fn append_terminal_context(out: &mut String, agent: &InteractiveAgent, n_units: usize) {
    let scrollback = agent.last_lines((n_units.max(1)) * 50);
    if scrollback.is_empty() {
        return;
    }

    out.push_str(&scrollback);
    if !scrollback.ends_with('\n') {
        out.push('\n');
    }
}

fn response_end_line(entry: &PromptEntry, is_last_prompt: bool, total_depth: usize) -> usize {
    if !is_last_prompt && entry.output_range.1 > entry.output_range.0 {
        return entry.output_range.1;
    }
    total_depth
}

// ── Persistence ──────────────────────────────────────────────────

// ── Modal state ──────────────────────────────────────────────────

/// Which step the two-step modal is on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContextTransferStep {
    /// Step 1 — adjust n_prompts and preview the payload.
    Preview,
    /// Step 2 — pick the destination agent.
    AgentPicker,
}

/// State for the context transfer modal.
pub struct ContextTransferModal {
    pub step: ContextTransferStep,
    /// Index into `App::interactive_agents` (or `terminal_agents` when `source_is_terminal`).
    pub source_agent_idx: usize,
    /// Whether the source is a terminal session (indexes `terminal_agents`).
    pub source_is_terminal: bool,
    /// Number of recent prompts / scroll-back pages to include (adjustable in Step 1).
    pub n_prompts: usize,
    /// Currently highlighted agent in the picker (index into the picker list).
    pub picker_selected: usize,
    /// Precomputed payload shown as preview in Step 1.
    pub payload_preview: String,
}

impl ContextTransferModal {
    fn new_with_kind(
        source_agent_idx: usize,
        source_kind: ContextSourceKind,
        config: &ContextTransferConfig,
    ) -> Self {
        Self {
            step: ContextTransferStep::Preview,
            source_agent_idx,
            source_is_terminal: source_kind == ContextSourceKind::Terminal,
            n_prompts: config.default_prompt_history,
            picker_selected: 0,
            payload_preview: String::new(),
        }
    }

    pub fn new(source_agent_idx: usize, config: &ContextTransferConfig) -> Self {
        Self::new_with_kind(source_agent_idx, ContextSourceKind::Interactive, config)
    }

    pub fn new_terminal(source_agent_idx: usize, config: &ContextTransferConfig) -> Self {
        Self::new_with_kind(source_agent_idx, ContextSourceKind::Terminal, config)
    }

    pub fn decrement_field(&mut self) {
        self.n_prompts = self.n_prompts.saturating_sub(1).max(1);
    }

    pub fn increment_field(&mut self, max_value: usize) {
        self.n_prompts = (self.n_prompts + 1).min(max_value.max(1));
    }

    pub fn source_kind(&self) -> ContextSourceKind {
        if self.source_is_terminal {
            ContextSourceKind::Terminal
        } else {
            ContextSourceKind::Interactive
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{clean_context_output, collect_last_prompts, ContextSourceKind};
    use crate::tui::agent::PromptEntry;
    use chrono::Utc;
    use std::collections::VecDeque;

    #[test]
    fn collect_last_prompts_keeps_tail_in_order() {
        let history = VecDeque::from(vec![
            PromptEntry {
                input: "one".to_string(),
                output_range: (0, 1),
                timestamp: Utc::now(),
            },
            PromptEntry {
                input: "two".to_string(),
                output_range: (1, 2),
                timestamp: Utc::now(),
            },
            PromptEntry {
                input: "three".to_string(),
                output_range: (2, 3),
                timestamp: Utc::now(),
            },
        ]);

        let prompts = collect_last_prompts(&history, 2);
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].input, "two");
        assert_eq!(prompts[1].input, "three");
    }

    #[test]
    fn clean_context_output_drops_noise_and_collapses_blanks() {
        let raw = "--- context from: demo | workdir: /tmp ---\n\nContext\n\nhello\n\n\nremaining 12\n--- end context ---\n";
        let cleaned = clean_context_output(raw);
        assert!(cleaned.contains("hello"));
        assert!(!cleaned.contains("remaining 12"));
        assert!(cleaned.contains("hello\n\n--- end context ---"));
    }

    #[test]
    fn modal_source_kind_reflects_session_type() {
        let config = super::ContextTransferConfig::default();
        let interactive = super::ContextTransferModal::new(1, &config);
        let terminal = super::ContextTransferModal::new_terminal(2, &config);
        assert_eq!(interactive.source_kind(), ContextSourceKind::Interactive);
        assert_eq!(terminal.source_kind(), ContextSourceKind::Terminal);
    }
}
