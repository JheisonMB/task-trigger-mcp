//! Agent lifecycle — PTY polling, Brian's Brain, clipboard, cleanup.

use super::{AgentEntry, App, Focus};
use crate::tui::agent::AgentStatus;

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
        if self.show_copied
            && self.copied_at.elapsed() > std::time::Duration::from_secs(2)
        {
            self.show_copied = false;
        }
    }

    pub fn copy_screen_to_clipboard(&mut self) {
        let text = match self.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => {
                let idx = *idx;
                if idx < self.interactive_agents.len() {
                    self.interactive_agents[idx].output()
                } else {
                    return;
                }
            }
            _ => self.log_content.clone(),
        };

        if text.is_empty() {
            return;
        }

        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(&text);
        }

        self.show_copied = true;
        self.copied_at = std::time::Instant::now();
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

        let mut removed_indices: Vec<usize> = self
            .interactive_agents
            .iter()
            .enumerate()
            .filter(|(_, a)| matches!(a.status, AgentStatus::Exited(_)))
            .map(|(i, _)| i)
            .collect();

        if removed_indices.is_empty() {
            return;
        }

        removed_indices.sort_unstable();
        removed_indices.reverse();

        for &old_idx in &removed_indices {
            self.interactive_agents.remove(old_idx);
        }

        for agent in &mut self.agents {
            if let AgentEntry::Interactive(idx) = agent {
                let shifts = removed_indices.iter().filter(|&&r| r < *idx).count();
                *idx -= shifts;
            }
        }

        if self.focus == Focus::Agent {
            self.focus = Focus::Preview;
            if self.selected >= self.agents.len() && !self.agents.is_empty() {
                self.selected = self.agents.len() - 1;
            }
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
            AgentEntry::Task(t) => {
                use crate::application::ports::TaskRepository;
                self.db.delete_task(&t.id)?;
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
        for agent in &mut self.interactive_agents {
            agent.kill();
        }
    }
}
