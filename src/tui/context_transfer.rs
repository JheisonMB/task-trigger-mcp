//! Context Transfer — capture and inject conversation context between agents.
//!
//! Builds a plain-text context block from the source agent's prompt history
//! and scrollback buffer, then drives the two-step TUI modal (preview → agent picker).
//! The transfer works entirely in memory — no disk I/O required.

use std::collections::VecDeque;

use super::agent::{InteractiveAgent, PromptEntry};

// ── Config ───────────────────────────────────────────────────────

/// Runtime defaults for context transfer (no external config file required).
pub struct ContextTransferConfig {
    pub default_prompt_history: usize,
    pub default_scrollback_lines: usize,
    pub max_scrollback_lines: usize,
    pub auto_switch_tab: bool,
}

impl Default for ContextTransferConfig {
    fn default() -> Self {
        Self {
            default_prompt_history: 3,
            default_scrollback_lines: 200,
            max_scrollback_lines: 2000,
            auto_switch_tab: true,
        }
    }
}

// ── Context builder ──────────────────────────────────────────────

/// Build the formatted context block from a source agent.
///
/// Format:
/// ```text
/// --- context from: <id> | workdir: <path> ---
/// [last prompts]
/// > prompt 1
/// ...response...
/// [scrollback excerpt — last N lines]
/// ...
/// --- end context ---
/// ```
pub fn build_context_payload(
    agent: &InteractiveAgent,
    n_prompts: usize,
    scrollback_lines: usize,
) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "--- context from: {} | workdir: {} ---\n",
        agent.id, agent.working_dir
    ));

    let prompts = collect_last_prompts(
        &agent
            .prompt_history
            .lock()
            .ok()
            .as_deref()
            .cloned()
            .unwrap_or_default(),
        n_prompts,
    );

    // Current history depth — used as the response-end boundary for the
    // last (still-open) prompt entry whose output_range.1 hasn't been
    // closed yet by a subsequent prompt.
    let current_depth = agent.max_scroll();

    if !prompts.is_empty() {
        out.push_str("[last prompts]\n");
        for entry in &prompts {
            out.push_str(&format!("> {}\n", entry.input));
            // Include the agent's response for this prompt.
            let resp_end = if entry.output_range.1 > entry.output_range.0 {
                entry.output_range.1
            } else {
                current_depth
            };
            if resp_end > entry.output_range.0 {
                let response = agent.lines_at_scrollback_range(entry.output_range.0, resp_end);
                if !response.is_empty() {
                    out.push_str(&response);
                    out.push('\n');
                }
            }
        }
    }

    let scrollback = agent.last_lines(scrollback_lines);
    if !scrollback.is_empty() {
        out.push_str(&format!(
            "[scrollback excerpt — last {} lines]\n",
            scrollback_lines
        ));
        out.push_str(&scrollback);
        out.push('\n');
    }

    out.push_str("--- end context ---\n");
    out
}

fn collect_last_prompts(history: &VecDeque<PromptEntry>, n: usize) -> Vec<PromptEntry> {
    history
        .iter()
        .rev()
        .take(n)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

// ── Persistence ──────────────────────────────────────────────────

// ── Modal state ──────────────────────────────────────────────────

/// Which step the two-step modal is on.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContextTransferStep {
    /// Step 1 — adjust n_prompts / scrollback_lines and preview the payload.
    Preview,
    /// Step 2 — pick the destination agent.
    AgentPicker,
}

/// State for the context transfer modal.
pub struct ContextTransferModal {
    pub step: ContextTransferStep,
    /// Index into `App::interactive_agents` for the source agent.
    pub source_agent_idx: usize,
    /// Number of recent prompts to include (adjustable in Step 1).
    pub n_prompts: usize,
    /// Number of scrollback lines to include (adjustable in Step 1).
    pub scrollback_lines: usize,
    /// Which input field has focus in Step 1 (0 = n_prompts, 1 = scrollback_lines).
    pub preview_field: usize,
    /// Currently highlighted agent in the picker (index into the picker list).
    pub picker_selected: usize,
    /// Precomputed payload shown as preview in Step 1.
    pub payload_preview: String,
}

impl ContextTransferModal {
    pub fn new(source_agent_idx: usize, config: &ContextTransferConfig) -> Self {
        Self {
            step: ContextTransferStep::Preview,
            source_agent_idx,
            n_prompts: config.default_prompt_history,
            scrollback_lines: config.default_scrollback_lines,
            preview_field: 0,
            picker_selected: 0,
            payload_preview: String::new(),
        }
    }

    /// Rebuild the payload preview from the source agent's current state.
    pub fn refresh_preview(&mut self, agent: &InteractiveAgent) {
        self.payload_preview = build_context_payload(agent, self.n_prompts, self.scrollback_lines);
    }

    pub fn increment_field(&mut self, max_scrollback: usize) {
        match self.preview_field {
            0 => self.n_prompts = (self.n_prompts + 1).min(20),
            _ => self.scrollback_lines = (self.scrollback_lines + 50).min(max_scrollback),
        }
    }

    pub fn decrement_field(&mut self) {
        match self.preview_field {
            0 => self.n_prompts = self.n_prompts.saturating_sub(1).max(1),
            _ => self.scrollback_lines = self.scrollback_lines.saturating_sub(50).max(10),
        }
    }
}
