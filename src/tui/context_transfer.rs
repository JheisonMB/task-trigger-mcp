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
    pub auto_switch_tab: bool,
}

impl Default for ContextTransferConfig {
    fn default() -> Self {
        Self {
            default_prompt_history: 3,
            auto_switch_tab: true,
        }
    }
}

// ── Context builder ──────────────────────────────────────────────

/// Build the formatted context block from a source agent.
///
/// Includes everything from the Nth-to-last prompt through the current
/// scrollback position — prompt inputs, their responses, and all output
/// after the last prompt.
pub fn build_context_payload(agent: &InteractiveAgent, n_prompts: usize) -> String {
    let n_prompts = n_prompts.max(1);

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

    if !prompts.is_empty() {
        for (idx, entry) in prompts.iter().enumerate() {
            out.push_str(&format!("> {}\n", entry.input));

            // Determine the response end:
            // - Closed prompts (not the last): output_range.1 was set when the next
            //   prompt arrived, by which point the response had scrolled into history.
            // - Last prompt (open): the response is on the visible screen.
            //   Use total_depth() which includes scrollback + visible rows, so we
            //   capture content that hasn't yet scrolled into history.
            let is_last_prompt = idx == prompts.len() - 1;
            let resp_end = if !is_last_prompt && entry.output_range.1 > entry.output_range.0 {
                entry.output_range.1
            } else {
                agent.total_depth()
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
    /// Step 1 — adjust n_prompts and preview the payload.
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
            picker_selected: 0,
            payload_preview: String::new(),
        }
    }

    /// Rebuild the payload preview from the source agent's current state.
    pub fn refresh_preview(&mut self, agent: &InteractiveAgent) {
        self.payload_preview = build_context_payload(agent, self.n_prompts);
    }

    pub fn increment_field(&mut self) {
        self.n_prompts = (self.n_prompts + 1).min(20);
    }

    pub fn decrement_field(&mut self) {
        self.n_prompts = self.n_prompts.saturating_sub(1).max(1);
    }
}
