use super::{AgentEntry, App, Focus};
use crate::tui::agent::AgentStatus;

/// Strip ANSI escape sequences from a string for plain-text display.
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip CSI sequences: ESC [ ... final_byte
            if chars.peek() == Some(&'[') {
                chars.next();
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() || next == 'm' {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

impl App {
    pub fn tick_brians_brain(&mut self) {
        if self.focus != Focus::Home {
            return;
        }

        let (tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
        let cols = tw.saturating_sub(26) as usize;
        let rows = th.saturating_sub(4) as usize;

        if cols == 0 || rows == 0 {
            return;
        }

        let needs_reinit = match &self.brain {
            None => true,
            Some(b) => b.rows != rows || b.cols != cols,
        };
        if needs_reinit {
            self.brain = Some(super::super::brians_brain::BriansBrain::new(rows, cols));
        }

        if let Some(ref mut brain) = self.brain {
            if brain.should_activate() {
                brain.activate();
            }
            if brain.active {
                brain.step();
            } else {
                brain.tick();
            }
        }
    }

    pub fn dismiss_brain(&mut self) {
        if let Some(ref mut brain) = self.brain {
            brain.reset();
        }
    }

    pub(super) fn dismiss_copied(&mut self) {
        if self.show_copied && self.copied_at.elapsed() > std::time::Duration::from_secs(2) {
            self.show_copied = false;
        }
    }

    pub fn next_interactive(&mut self) {
        let interactive_indices: Vec<usize> = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, a)| matches!(a, AgentEntry::Interactive(_)))
            .map(|(i, _)| i)
            .collect();

        if interactive_indices.is_empty() {
            return;
        }

        let current_pos = interactive_indices
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0);

        let next_pos = (current_pos + 1) % interactive_indices.len();
        self.selected = interactive_indices[next_pos];
        self.focus = Focus::Agent;
    }

    pub(super) fn resize_interactive_agents(&mut self) {
        let (cols, rows) = self.last_panel_inner;
        if cols == 0 || rows == 0 {
            return;
        }

        for agent in &mut self.interactive_agents {
            if agent.last_pty_cols != cols || agent.last_pty_rows != rows {
                agent.resize(cols, rows);
            }
        }
    }

    pub(super) fn poll_interactive_agents(&mut self) {
        for agent in &mut self.interactive_agents {
            agent.poll();
        }

        let removed_indices: Vec<usize> = self
            .interactive_agents
            .iter()
            .enumerate()
            .filter(|(_, a)| matches!(a.status, AgentStatus::Exited(_)))
            .map(|(i, _)| i)
            .collect();

        if removed_indices.is_empty() {
            return;
        }

        // 1. Remove matching AgentEntry::Interactive from self.agents
        //    BEFORE touching interactive_agents so indices are still valid.
        self.agents.retain(|a| {
            if let AgentEntry::Interactive(idx) = a {
                !removed_indices.contains(idx)
            } else {
                true
            }
        });

        // 2. Remove from interactive_agents (reverse order preserves indices)
        let mut sorted = removed_indices;
        sorted.sort_unstable();
        sorted.reverse();
        for &old_idx in &sorted {
            // Notify whimsg about agent completion
            let status = self.interactive_agents[old_idx].status;
            let agent_id = self.interactive_agents[old_idx].id.clone();
            match status {
                AgentStatus::Exited(0) => {
                    let _ = self.db.finish_interactive_session(&agent_id, 0);
                    self.whimsg
                        .notify_event(crate::tui::whimsg::WhimContext::AgentDone);
                    if self.notifications_enabled {
                        crate::domain::notification::send_notification(
                            "Canopy — agent finished",
                            &format!("{agent_id} completed successfully"),
                        );
                    }
                }
                AgentStatus::Exited(code) => {
                    let _ = self.db.finish_interactive_session(&agent_id, code);
                    // Capture last PTY output for error diagnosis
                    let last_lines = self.interactive_agents[old_idx].last_output_lines(5);
                    let output_snippet = if last_lines.is_empty() {
                        String::new()
                    } else {
                        // Strip ANSI escape codes for notification readability
                        let clean: Vec<String> = last_lines
                            .iter()
                            .map(|l| strip_ansi_codes(l))
                            .filter(|l| !l.is_empty())
                            .collect();
                        if clean.is_empty() {
                            String::new()
                        } else {
                            format!("\n{}", clean.join("\n"))
                        }
                    };
                    tracing::warn!(
                        "Agent '{agent_id}' ({}) exited with code {code}.{}",
                        self.interactive_agents[old_idx].cli.as_str(),
                        if output_snippet.is_empty() { "" } else { &output_snippet }
                    );
                    self.whimsg
                        .notify_event(crate::tui::whimsg::WhimContext::AgentFailed);
                    if self.notifications_enabled {
                        let msg = if output_snippet.is_empty() {
                            format!("{agent_id} exited with code {code}")
                        } else {
                            format!("{agent_id} exited ({code}){output_snippet}")
                        };
                        crate::domain::notification::send_notification(
                            "Canopy — agent failed",
                            &msg,
                        );
                    }
                }
                _ => {}
            }
            self.interactive_agents.remove(old_idx);
        }

        // 3. Adjust remaining Interactive indices
        for agent in &mut self.agents {
            if let AgentEntry::Interactive(idx) = agent {
                let shifts = sorted.iter().filter(|&&r| r < *idx).count();
                *idx -= shifts;
            }
        }

        // 4. Fix focus and selection
        if self.focus == Focus::Agent {
            self.focus = Focus::Preview;
        }
        if self.selected >= self.agents.len() {
            self.selected = self.agents.len().saturating_sub(1);
        }
    }

    pub fn rerun_selected(&self) -> anyhow::Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Interactive(_) => Ok(()),
            _ => {
                use crate::application::ports::StateRepository;
                let port = self
                    .db
                    .get_state("port")?
                    .unwrap_or_else(|| "7755".to_string());
                super::send_mcp_task_run(&port, agent.id(self))
            }
        }
    }

    #[allow(dead_code)]
    pub fn kill_selected_agent(&mut self) {
        let Some(AgentEntry::Interactive(idx)) = self.agents.get(self.selected) else {
            return;
        };
        let idx = *idx;
        self.interactive_agents[idx].kill();
        self.interactive_agents.remove(idx);
        let _ = self.refresh_agents();
        if self.selected >= self.agents.len() && !self.agents.is_empty() {
            self.selected = self.agents.len() - 1;
        }
        self.focus = Focus::Preview;
    }

    pub fn delete_selected(&mut self) -> anyhow::Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::BackgroundAgent(t) => {
                use crate::application::ports::BackgroundAgentRepository;
                self.db.delete_background_agent(&t.id)?;
            }
            AgentEntry::Watcher(w) => {
                use crate::application::ports::WatcherRepository;
                self.db.delete_watcher(&w.id)?;
            }
            AgentEntry::Interactive(idx) => {
                self.interactive_agents[*idx].kill();
                self.interactive_agents.remove(*idx);
            }
        }
        let _ = self.refresh_agents();
        if self.selected >= self.agents.len() && !self.agents.is_empty() {
            self.selected = self.agents.len() - 1;
        }
        self.focus = Focus::Preview;
        Ok(())
    }

    pub fn cleanup(&mut self) {
        // Leave sessions marked 'active' so auto-resume picks them up on restart.
        // Only kill the PTY processes — the CLI's own session resume will handle reconnection.
        for agent in &mut self.interactive_agents {
            agent.kill();
        }
    }
}
