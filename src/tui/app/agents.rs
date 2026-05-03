use super::types::{AgentEntry, App, Focus};
use crate::tui::agent::{AgentStatus, InteractiveAgent};

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

fn recent_output_snippet(agent: &InteractiveAgent, n: usize) -> String {
    let clean: Vec<String> = agent
        .last_output_lines(n)
        .into_iter()
        .map(|line| strip_ansi_codes(&line))
        .filter(|line| !line.is_empty())
        .collect();

    if clean.is_empty() {
        String::new()
    } else {
        format!("\n{}", clean.join("\n"))
    }
}

impl App {
    pub fn notify_mouse_move(&mut self) {
        if let Some(ref mut brain) = self.home_brain {
            brain.notify_mouse();
        }
        if let Some(ref mut brain) = self.sidebar_brain {
            brain.notify_mouse();
        }
    }

    pub fn tick_banner_glitch(&mut self) {
        // Always step the brain if it exists so animation continues
        // across focus changes (e.g. closing all sessions → Preview).
        if let Some(ref mut brain) = self.home_brain {
            brain.step();
        }

        // Only resize/reinitialize when on Home focus
        if self.focus != Focus::Home {
            return;
        }

        let (pw, ph) = self.last_panel_inner;
        let panel_cols = pw as usize;
        let panel_rows = ph as usize;

        if panel_cols < 6 || panel_rows < 3 {
            let (tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
            let fallback_cols = (tw / 2).saturating_sub(2) as usize;
            let fallback_rows = th.saturating_sub(3) as usize;
            if fallback_cols < 6 || fallback_rows < 3 {
                return;
            }
            let needs_reinit = match &self.home_brain {
                None => true,
                Some(b) => b.rows != fallback_rows || b.cols != fallback_cols,
            };
            if needs_reinit {
                let mut brain =
                    super::super::brians_brain::BriansBrain::new(fallback_rows, fallback_cols, 80);
                brain.last_step = std::time::Instant::now()
                    - std::time::Duration::from_millis(brain.step_interval_ms);
                self.home_brain = Some(brain);
            }
        } else {
            let needs_reinit = match &self.home_brain {
                None => true,
                Some(b) => b.rows != panel_rows || b.cols != panel_cols,
            };
            if needs_reinit {
                let mut brain =
                    super::super::brians_brain::BriansBrain::new(panel_rows, panel_cols, 80);
                brain.last_step = std::time::Instant::now()
                    - std::time::Duration::from_millis(brain.step_interval_ms);
                self.home_brain = Some(brain);
            }
        }
    }

    pub fn ensure_sidebar_brain(&mut self) {
        // The exact sidebar brain dimensions depend on layout (agent card count, etc.)
        // so we compute them the same way sidebar.rs does.
        let (_tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
        let sidebar_w = 30u16;
        let sidebar_h = th.saturating_sub(2); // minus header + footer

        // Approximate: inner width = sidebar - 2 borders
        let inner_w = sidebar_w.saturating_sub(2) as usize;
        // Dashboard takes 6 rows if sidebar is tall enough
        let dashboard_h = if sidebar_h >= 6 { 6 } else { 0 };
        let content_h = sidebar_h.saturating_sub(dashboard_h) as usize;

        // The brain gets whatever space is left after agent cards.
        // We can't know the exact amount without rendering, so use the full
        // content height as a max bound. The sidebar clips to brain_area anyway.
        let rows = content_h;
        let cols = inner_w;

        if cols < 6 || rows < 3 {
            return;
        }

        let needs_reinit = match &self.sidebar_brain {
            None => true,
            Some(b) => b.rows != rows || b.cols != cols,
        };
        if needs_reinit {
            let mut brain = super::super::brians_brain::BriansBrain::new(rows, cols, 60);
            // Allow immediate first step
            brain.last_step = std::time::Instant::now()
                - std::time::Duration::from_millis(brain.step_interval_ms);
            self.sidebar_brain = Some(brain);
        }

        if let Some(ref mut brain) = self.sidebar_brain {
            brain.step();
        }
    }

    pub fn dismiss_brain(&mut self) {
        if let Some(ref mut brain) = self.home_brain {
            *brain = super::super::brians_brain::BriansBrain::new(brain.rows, brain.cols, 80);
        }
    }

    pub(super) fn dismiss_copied(&mut self) {
        if self.show_copied && self.copied_at.elapsed() > std::time::Duration::from_secs(2) {
            self.show_copied = false;
        }
    }

    pub fn next_interactive(&mut self) {
        let focusable: Vec<usize> = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                matches!(
                    a,
                    AgentEntry::Interactive(_) | AgentEntry::Terminal(_) | AgentEntry::Group(_)
                )
            })
            .map(|(i, _)| i)
            .collect();

        if focusable.is_empty() {
            return;
        }

        let current_pos = focusable
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0);

        let next_pos = (current_pos + 1) % focusable.len();
        self.selected = focusable[next_pos];
        self.focus = Focus::Agent;
        self.activate_selected_entry();
    }

    pub fn prev_interactive(&mut self) {
        let focusable: Vec<usize> = self
            .agents
            .iter()
            .enumerate()
            .filter(|(_, a)| {
                matches!(
                    a,
                    AgentEntry::Interactive(_) | AgentEntry::Terminal(_) | AgentEntry::Group(_)
                )
            })
            .map(|(i, _)| i)
            .collect();

        if focusable.is_empty() {
            return;
        }

        let current_pos = focusable
            .iter()
            .position(|&i| i == self.selected)
            .unwrap_or(0);

        let prev_pos = if current_pos == 0 {
            focusable.len() - 1
        } else {
            current_pos - 1
        };
        self.selected = focusable[prev_pos];
        self.focus = Focus::Agent;
        self.activate_selected_entry();
    }

    /// Activate split or clear it based on the currently selected entry.
    fn activate_selected_entry(&mut self) {
        match &self.agents[self.selected] {
            AgentEntry::Group(idx) => {
                if let Some(group) = self.split_groups.get(*idx) {
                    self.active_split_id = Some(group.id.clone());
                    self.split_right_focused = false;
                }
            }
            AgentEntry::Interactive(idx) => {
                self.active_split_id = None;
                self.interactive_agents[*idx].mark_viewed();
            }
            AgentEntry::Terminal(idx) => {
                self.active_split_id = None;
                self.terminal_agents[*idx].mark_viewed();
            }
            _ => {
                self.active_split_id = None;
            }
        }
    }

    pub(super) fn resize_interactive_agents(&mut self) {
        let (cols, rows) = self.last_panel_inner;
        if cols == 0 || rows == 0 {
            return;
        }

        // In split mode, only resize the two sessions participating in the split.
        // Other sessions keep their last size to avoid unnecessary resize churn.
        let split_sessions: Option<(String, String)> =
            self.active_split_id.as_ref().and_then(|id| {
                self.split_groups
                    .iter()
                    .find(|g| g.id == *id)
                    .map(|g| (g.session_a.clone(), g.session_b.clone()))
            });

        for agent in &mut self.interactive_agents {
            let dominated = split_sessions
                .as_ref()
                .is_some_and(|(a, b)| agent.name != *a && agent.name != *b);
            if dominated {
                continue;
            }
            if agent.last_pty_cols != cols || agent.last_pty_rows != rows {
                agent.resize(cols, rows);
            }
        }
        for agent in &mut self.terminal_agents {
            let dominated = split_sessions
                .as_ref()
                .is_some_and(|(a, b)| agent.name != *a && agent.name != *b);
            if dominated {
                continue;
            }
            // Warp-mode terminals lose 3 rows for the input box
            let effective_rows = if agent.warp_mode {
                rows.saturating_sub(3)
            } else {
                rows
            };
            if agent.last_pty_cols != cols || agent.last_pty_rows != effective_rows {
                agent.resize(cols, effective_rows);
            }
        }
    }

    /// Poll terminal agent processes for exit status.
    pub(super) fn poll_terminal_agents(&mut self) {
        if matches!(self.focus, Focus::Agent | Focus::Preview) {
            if let Some(AgentEntry::Terminal(idx)) = self.agents.get(self.selected) {
                self.terminal_agents[*idx].mark_viewed();
            }
        }

        for agent in &mut self.terminal_agents {
            agent.poll();
        }

        let exited_indices: Vec<usize> = self
            .terminal_agents
            .iter()
            .enumerate()
            .filter(|(_, agent)| matches!(agent.status, AgentStatus::Exited(_)))
            .map(|(idx, _)| idx)
            .collect();
        if exited_indices.is_empty() {
            return;
        }

        for &idx in &exited_indices {
            let agent = &self.terminal_agents[idx];
            if agent.exit_notified {
                continue;
            }

            let AgentStatus::Exited(code) = agent.status else {
                continue;
            };
            let _ = self.db.finish_terminal_session(&agent.id);

            if code != 0 {
                let output_snippet = recent_output_snippet(agent, 5);
                tracing::warn!(
                    "Terminal '{}' ({}) exited with code {code}.{}",
                    agent.name,
                    agent.shell,
                    if output_snippet.is_empty() {
                        ""
                    } else {
                        &output_snippet
                    }
                );
            }

            self.terminal_agents[idx].exit_notified = true;
        }

        let mut removed = exited_indices;
        removed.sort_unstable();
        removed.reverse();
        for idx in removed {
            let _ = self.remove_terminal_session_entry(idx);
        }
        self.finish_session_mutation();
    }

    pub(super) fn poll_interactive_agents(&mut self) {
        if let Some(AgentEntry::Interactive(idx)) = self.agents.get(self.selected) {
            if matches!(self.focus, Focus::Agent | Focus::Preview) {
                self.interactive_agents[*idx].mark_viewed();
            }
        }

        for agent in &mut self.interactive_agents {
            agent.poll();
        }

        // Collect indices that just exited (any code) for notification handling.
        let newly_exited: Vec<usize> = self
            .interactive_agents
            .iter()
            .enumerate()
            .filter(|(_, a)| matches!(a.status, AgentStatus::Exited(_)))
            .map(|(i, _)| i)
            .collect();

        // Send notifications / record DB finish for all newly-exited agents.
        // Track which ones we already notified to avoid repeats on next poll.
        for &idx in &newly_exited {
            let agent = &self.interactive_agents[idx];
            if agent.exit_notified {
                continue;
            }
            let status = agent.status;
            let agent_id = agent.id.clone();
            match status {
                AgentStatus::Exited(0) => {
                    let _ = self.db.finish_interactive_session(&agent_id, 0);
                }
                AgentStatus::Exited(code) => {
                    let _ = self.db.finish_interactive_session(&agent_id, code);
                    let output_snippet = recent_output_snippet(&self.interactive_agents[idx], 5);
                    tracing::warn!(
                        "Agent '{agent_id}' ({}) exited with code {code}.{}",
                        self.interactive_agents[idx].cli.as_str(),
                        if output_snippet.is_empty() {
                            ""
                        } else {
                            &output_snippet
                        }
                    );
                    self.whimsg
                        .notify_event(crate::tui::whimsg::WhimContext::AgentFailed);
                    if self.notifications_enabled {
                        let output = if output_snippet.is_empty() {
                            String::new()
                        } else {
                            output_snippet.trim_start_matches('\n').to_string()
                        };
                        self.notification_service.notify_agent_failed(
                            &agent_id,
                            self.interactive_agents[idx].cli.as_str(),
                            code,
                            &output,
                        );
                    }
                }
                _ => {}
            }
            self.interactive_agents[idx].exit_notified = true;
        }

        // Only auto-remove agents that exited SUCCESSFULLY (code 0).
        // Failed agents stay in the list so the user can inspect output.
        let removed_indices: Vec<usize> = self
            .interactive_agents
            .iter()
            .enumerate()
            .filter(|(_, a)| matches!(a.status, AgentStatus::Exited(0)))
            .map(|(i, _)| i)
            .collect();

        if removed_indices.is_empty() {
            return;
        }

        let mut sorted = removed_indices;
        sorted.sort_unstable();
        sorted.reverse();
        for idx in sorted {
            let _ = self.remove_interactive_session_entry(idx);
        }
        self.finish_session_mutation();
    }

    pub fn rerun_selected(&self) -> anyhow::Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Interactive(_) | AgentEntry::Terminal(_) | AgentEntry::Group(_) => Ok(()),
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
        if self.close_interactive_session_at(*idx, 0) {
            self.finish_session_mutation();
        }
    }

    pub fn delete_selected(&mut self) -> anyhow::Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Agent(a) => {
                use crate::application::ports::AgentRepository;
                self.db.delete_agent(&a.id)?;
            }
            AgentEntry::Interactive(idx) => {
                if !self.close_interactive_session_at(*idx, 0) {
                    return Ok(());
                }
            }
            AgentEntry::Terminal(idx) => {
                if !self.close_terminal_session_at(*idx) {
                    return Ok(());
                }
            }
            AgentEntry::Group(idx) => {
                let idx = *idx;
                if idx < self.split_groups.len() {
                    let id = self.split_groups[idx].id.clone();
                    let _ = self.db.delete_group(&id);
                    self.split_groups.remove(idx);
                    if self.active_split_id.as_deref() == Some(&id) {
                        self.active_split_id = None;
                    }
                }
            }
        }
        self.finish_session_mutation();
        Ok(())
    }

    /// Dissolve all split groups that contain the given session name.
    fn dissolve_groups_for_session(&mut self, session_name: &str) {
        let ids_to_dissolve: Vec<String> = self
            .split_groups
            .iter()
            .filter(|g| g.session_a == session_name || g.session_b == session_name)
            .map(|g| g.id.clone())
            .collect();

        for id in &ids_to_dissolve {
            let _ = self.db.delete_group(id);
            if self.active_split_id.as_deref() == Some(id.as_str()) {
                self.active_split_id = None;
            }
        }
        self.split_groups
            .retain(|g| !ids_to_dissolve.contains(&g.id));
    }

    pub fn cleanup(&mut self) {
        for agent in &mut self.interactive_agents {
            agent.kill();
        }
        for agent in &mut self.terminal_agents {
            agent.kill();
        }
        // Clear any lingering toast notifications from the Windows Action Center
        crate::domain::notification::clear_notifications_on_exit();
    }

    /// Terminate the session(s) currently in focus.
    ///
    /// - Single agent/terminal: kill it and remove.
    /// - Active split group: kill both sessions and dissolve the group.
    pub fn terminate_focused_session(&mut self) {
        if let Some(ref split_id) = self.active_split_id.clone() {
            // Split mode — kill the focused panel's session, dissolve the group
            let group = self.split_groups.iter().find(|g| g.id == *split_id);
            let Some(group) = group else { return };
            let target = if self.split_right_focused {
                group.session_b.clone()
            } else {
                group.session_a.clone()
            };
            self.kill_session_by_name(&target);

            // Dissolve the group
            let _ = self.db.delete_group(split_id);
            self.split_groups.retain(|g| g.id != *split_id);
            self.active_split_id = None;
        } else {
            // Single session — kill the selected agent
            let selection = match self.selected_agent() {
                Some(AgentEntry::Interactive(idx)) => Some(("interactive", *idx)),
                Some(AgentEntry::Terminal(idx)) => Some(("terminal", *idx)),
                _ => None,
            };
            match selection {
                Some(("interactive", idx)) if self.close_interactive_session_at(idx, 0) => {}
                Some(("terminal", idx)) if self.close_terminal_session_at(idx) => {}
                _ => return,
            }
        }

        self.finish_session_mutation();
    }

    /// Kill and remove a session by name (interactive or terminal).
    fn kill_session_by_name(&mut self, name: &str) {
        if let Some(idx) = self.interactive_agents.iter().position(|a| a.name == name) {
            let _ = self.close_interactive_session_at(idx, 0);
        } else if let Some(idx) = self.terminal_agents.iter().position(|a| a.name == name) {
            let _ = self.close_terminal_session_at(idx);
        }
    }

    fn finish_session_mutation(&mut self) {
        let _ = self.refresh_agents();
        if self.selected >= self.agents.len() && !self.agents.is_empty() {
            self.selected = self.agents.len() - 1;
        }
        if self.focus == Focus::Agent
            || matches!(self.focus, Focus::ContextTransfer | Focus::RagTransfer)
        {
            self.focus = Focus::Preview;
        }
    }

    fn close_interactive_session_at(&mut self, idx: usize, exit_code: i32) -> bool {
        let Some(agent) = self.interactive_agents.get(idx) else {
            return false;
        };

        let agent_id = agent.id.clone();
        let _ = self.db.finish_interactive_session(&agent_id, exit_code);
        self.interactive_agents[idx].kill();
        self.remove_interactive_session_entry(idx)
    }

    fn close_terminal_session_at(&mut self, idx: usize) -> bool {
        let Some(agent) = self.terminal_agents.get(idx) else {
            return false;
        };

        let agent_id = agent.id.clone();
        let _ = self.db.finish_terminal_session(&agent_id);
        self.terminal_agents[idx].kill();
        self.remove_terminal_session_entry(idx)
    }

    fn remove_interactive_session_entry(&mut self, idx: usize) -> bool {
        if idx >= self.interactive_agents.len() {
            return false;
        }

        let agent_name = self.interactive_agents[idx].name.clone();
        self.interactive_agents.remove(idx);
        self.dissolve_groups_for_session(&agent_name);
        true
    }

    fn remove_terminal_session_entry(&mut self, idx: usize) -> bool {
        if idx >= self.terminal_agents.len() {
            return false;
        }

        let agent_name = self.terminal_agents[idx].name.clone();
        self.terminal_agents.remove(idx);
        self.dissolve_groups_for_session(&agent_name);
        true
    }
}
