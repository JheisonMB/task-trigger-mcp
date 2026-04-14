//! Application state for the TUI.
//!
//! Holds cached data from the database, selection state, log content,
//! and interactive agent processes.

use anyhow::Result;
use chrono::{DateTime, Utc};
use ratatui::style::Color;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::ports::{
    RunRepository, StateRepository, TaskRepository, WatcherRepository,
};
use crate::db::Database;
use crate::domain::models::{Cli, RunLog, Task, Watcher};

use super::agent::{AgentStatus, InteractiveAgent};

/// Unified entry in the sidebar.
pub enum AgentEntry {
    Task(Task),
    Watcher(Watcher),
    Interactive(usize), // index into App::interactive_agents
}

impl AgentEntry {
    pub fn id<'a>(&'a self, app: &'a App) -> &'a str {
        match self {
            Self::Task(t) => &t.id,
            Self::Watcher(w) => &w.id,
            Self::Interactive(idx) => &app.interactive_agents[*idx].id,
        }
    }
}

/// Which panel has focus.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Home mode: sidebar navigation, banner or task details in right panel.
    Home,
    /// Preview mode: log output for tasks, read-only PTY for agents.
    Preview,
    NewAgentDialog,
    /// Focus mode: interactive PTY for agents, detailed log for tasks.
    Agent,
}

/// Type of task to create.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NewTaskType {
    Interactive,
    Scheduled,
    Watcher,
}

/// Launch mode for interactive agents.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NewTaskMode {
    /// Start a fresh interactive session (uses `interactive_args`).
    Interactive,
    /// Resume a previous session (uses `resume_args`).
    Resume,
}

/// State for the "new agent" dialog.
pub struct NewAgentDialog {
    pub task_type: NewTaskType,
    /// Launch mode for interactive agents (ignored for scheduled/watcher).
    pub task_mode: NewTaskMode,
    pub cli_index: usize,
    pub available_clis: Vec<Cli>,
    /// Registry configs parallel to `available_clis` (for `interactive_args` etc.)
    pub cli_configs: Vec<Option<crate::domain::cli_config::CliConfig>>,
    pub working_dir: String,
    pub model: String,
    /// Task/watch fields
    pub prompt: String,
    pub cron_expr: String,
    pub watch_path: String,
    pub watch_events: Vec<String>,
    /// Which field is focused: 0=type, 1=mode (interactive only), 2=CLI, 3=dir, 4=model, 5=prompt, 6=cron/watch
    pub field: usize,
    pub dir_entries: Vec<String>,
    pub dir_selected: usize,
    pub dir_scroll: usize,
    pub current_path: String,
    /// Focus state before opening the dialog, restored on cancel.
    pub prev_focus: Option<Focus>,
}

impl NewAgentDialog {
    pub fn new() -> Self {
        let (available, configs) = Self::load_available_clis();
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut dialog = Self {
            task_type: NewTaskType::Interactive,
            task_mode: NewTaskMode::Interactive,
            cli_index: 0,
            available_clis: if available.is_empty() {
                vec![Cli::OpenCode, Cli::Kiro, Cli::Qwen]
            } else {
                available
            },
            cli_configs: if configs.is_empty() {
                vec![None, None, None]
            } else {
                configs
            },
            working_dir: cwd.clone(),
            model: String::new(),
            prompt: String::new(),
            cron_expr: "0 9 * * *".to_string(),
            watch_path: cwd.clone(),
            watch_events: vec!["create".to_string(), "modify".to_string()],
            field: 1,
            dir_entries: Vec::new(),
            dir_selected: 0,
            dir_scroll: 0,
            current_path: cwd,
            prev_focus: None,
        };
        dialog.refresh_dir_entries();
        dialog
    }

    /// Load available CLIs from saved registry config, returning both
    /// the Cli enum list and their corresponding `CliConfig` for `interactive_args`.
    fn load_available_clis() -> (Vec<Cli>, Vec<Option<crate::domain::cli_config::CliConfig>>) {
        if let Some(home) = dirs::home_dir() {
            let config_path = home.join(".canopy/cli_config.json");
            if let Some(registry) = crate::domain::cli_config::CliRegistry::load(&config_path) {
                let mut clis = Vec::new();
                let mut configs = Vec::new();
                for c in &registry.available_clis {
                    if let Ok(cli) = Cli::resolve(Some(&c.name)) {
                        clis.push(cli);
                        configs.push(Some(c.clone()));
                    }
                }
                if !clis.is_empty() {
                    return (clis, configs);
                }
            }
        }
        let detected = Cli::detect_available();
        let none_configs = vec![None; detected.len()];
        (detected, none_configs)
    }

    pub fn selected_cli(&self) -> Cli {
        self.available_clis[self.cli_index]
    }

    /// Get the correct args for the current launch mode.
    pub fn selected_args(&self) -> Option<String> {
        let config = self
            .cli_configs
            .get(self.cli_index)
            .and_then(|c| c.as_ref())?;
        match self.task_mode {
            NewTaskMode::Resume => config.resume_args.clone(),
            NewTaskMode::Interactive => config.interactive_args.clone(),
        }
    }

    /// Get the fallback args for interactive mode (e.g. kiro `chat`).
    pub fn selected_fallback_args(&self) -> Option<String> {
        self.cli_configs
            .get(self.cli_index)
            .and_then(|c| c.as_ref())
            .and_then(|c| c.fallback_interactive_args.clone())
    }

    /// Get the accent color for the currently selected CLI.
    pub fn selected_accent_color(&self) -> Color {
        self.cli_configs
            .get(self.cli_index)
            .and_then(|c| c.as_ref())
            .and_then(|c| c.accent_color)
            .map(|[r, g, b]| Color::Rgb(r, g, b))
            .unwrap_or(Color::Rgb(102, 187, 106)) // fallback green
    }

    pub fn next_cli(&mut self) {
        self.cli_index = (self.cli_index + 1) % self.available_clis.len();
    }

    pub fn prev_cli(&mut self) {
        self.cli_index = self
            .cli_index
            .checked_sub(1)
            .unwrap_or(self.available_clis.len() - 1);
    }

    /// Refresh directory entries for current path
    pub fn refresh_dir_entries(&mut self) {
        let Ok(entries) = std::fs::read_dir(&self.current_path) else {
            self.dir_entries.clear();
            return;
        };

        self.dir_entries.clear();
        let mut dirs: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_string()
                    .strip_prefix('.')
                    .map(|_| None)
                    .unwrap_or_else(|| Some(e.file_name().to_string_lossy().to_string()))
            })
            .collect();

        dirs.sort();
        if self.current_path != "/" {
            dirs.insert(0, "..".to_string());
        }

        self.dir_entries = dirs;
        self.dir_selected = 0;
        self.dir_scroll = 0;
    }

    /// Navigate to selected directory
    pub fn navigate_to_selected(&mut self) {
        if self.dir_selected >= self.dir_entries.len() {
            return;
        }

        let selected = &self.dir_entries[self.dir_selected];
        let new_path = if selected == ".." {
            if let Some(pos) = self.current_path.rfind('/') {
                if pos == 0 {
                    "/".to_string()
                } else {
                    self.current_path[..pos].to_string()
                }
            } else {
                ".".to_string()
            }
        } else {
            format!("{}/{}", self.current_path.trim_end_matches('/'), selected)
        };

        self.current_path = new_path;
        self.working_dir = self.current_path.clone();
        self.refresh_dir_entries();
    }
}

/// Main application state.
pub struct App {
    pub db: Arc<Database>,
    pub data_dir: PathBuf,

    // Data cache (refreshed every tick)
    pub agents: Vec<AgentEntry>,
    pub active_runs: HashMap<String, RunLog>,
    pub recent_runs: Vec<RunLog>,
    pub interactive_agents: Vec<InteractiveAgent>,

    // Daemon info
    pub daemon_running: bool,
    pub daemon_pid: Option<u32>,
    pub daemon_version: String,

    // UI state
    pub selected: usize,
    pub focus: Focus,
    pub log_content: String,
    pub log_scroll: u16,
    pub running: bool,
    pub new_agent_dialog: Option<NewAgentDialog>,
    /// Timestamp of last Esc press (for double-Esc detection).
    pub last_esc: std::time::Instant,
    /// Whether the quit confirmation overlay is shown.
    pub quit_confirm: bool,

    // Brian's Brain automaton
    pub brain: Option<super::brians_brain::BriansBrain>,

    /// Sidebar agent cards layout: (`global_idx`, `y_start`, `y_end`) for click mapping.
    pub sidebar_click_map: Vec<(usize, u16, u16)>,
    /// Whether the sidebar is visible.
    pub sidebar_visible: bool,
    /// Last terminal width (for auto-hide detection).
    pub term_width: u16,
    /// Show color legend overlay.
    pub show_legend: bool,
    /// Show "COPIED" indicator.
    pub show_copied: bool,
    /// When the copy happened (for auto-dismiss).
    pub copied_at: std::time::Instant,
    /// Last known inner dimensions of the right panel (set by draw).
    pub last_panel_inner: (u16, u16),
    /// Whimsical header messages.
    pub whimsg: super::whimsg::Whimsg,
}

impl App {
    pub fn new(db: Arc<Database>, data_dir: &Path) -> Result<Self> {
        let mut app = Self {
            db,
            data_dir: data_dir.to_path_buf(),
            agents: Vec::new(),
            active_runs: HashMap::new(),
            recent_runs: Vec::new(),
            interactive_agents: Vec::new(),
            daemon_running: false,
            daemon_pid: None,
            daemon_version: String::new(),
            selected: 0,
            focus: Focus::Home,
            log_content: String::new(),
            log_scroll: 0,
            running: true,
            new_agent_dialog: None,
            last_esc: std::time::Instant::now() - std::time::Duration::from_secs(10),
            quit_confirm: false,
            brain: None,
            sidebar_click_map: Vec::new(),
            sidebar_visible: true,
            term_width: 0,
            show_legend: false,
            show_copied: false,
            copied_at: std::time::Instant::now() - std::time::Duration::from_secs(10),
            last_panel_inner: (0, 0),
            whimsg: super::whimsg::Whimsg::new(),
        };
        app.refresh()?;
        Ok(app)
    }

    /// Reload all data from the database and filesystem.
    pub fn refresh(&mut self) -> Result<()> {
        self.refresh_daemon_status();
        self.refresh_agents()?;
        self.refresh_active_runs()?;
        self.poll_interactive_agents();
        self.tick_brians_brain();
        self.refresh_log();
        self.auto_hide_sidebar();
        self.dismiss_copied();
        self.resize_interactive_agents();
        Ok(())
    }

    /// Auto-hide sidebar when in interactive agent mode with narrow console.
    /// Auto-show when terminal is wide enough again.
    fn auto_hide_sidebar(&mut self) {
        if let Ok((tw, _th)) = ratatui::crossterm::terminal::size() {
            self.term_width = tw;
            let should_hide = self.focus == Focus::Agent
                && self
                    .selected_agent()
                    .is_some_and(|a| matches!(a, AgentEntry::Interactive(_)))
                && tw < 80;
            let should_show = tw >= 80 && !self.sidebar_visible;
            if should_hide {
                self.sidebar_visible = false;
            } else if should_show {
                self.sidebar_visible = true;
            }
        }
    }

    /// Resize interactive agents' PTY to match the actual render area.
    fn resize_interactive_agents(&mut self) {
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

        // Initialize or reinitialize on terminal resize
        let needs_reinit = match &self.brain {
            None => true,
            Some(b) => b.rows != rows || b.cols != cols,
        };
        if needs_reinit {
            self.brain = Some(super::brians_brain::BriansBrain::new(rows, cols));
        }

        if let Some(ref mut brain) = self.brain {
            if brain.should_activate() {
                brain.activate();
            }
            if brain.active {
                brain.step();
            } else {
                // Advance the unfold animation
                brain.tick();
            }
        }
    }

    /// Dismiss the Brian's Brain screensaver and reset it for next time.
    pub fn dismiss_brain(&mut self) {
        if let Some(ref mut brain) = self.brain {
            brain.reset();
        }
    }

    /// Auto-dismiss the COPIED indicator after 2 seconds.
    fn dismiss_copied(&mut self) {
        if self.show_copied
            && self.copied_at.elapsed() > std::time::Duration::from_secs(2)
        {
            self.show_copied = false;
        }
    }

    /// Copy the current screen content to the system clipboard.
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

    /// Cycle to the next interactive agent and go to focus mode.
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

    fn refresh_daemon_status(&mut self) {
        let pid_path = self.data_dir.join("daemon.pid");
        self.daemon_pid = std::fs::read_to_string(&pid_path)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        self.daemon_running = self.daemon_pid.map(is_process_running).unwrap_or(false);
        self.daemon_version = self
            .db
            .get_state("version")
            .ok()
            .flatten()
            .unwrap_or_default();
    }

    fn refresh_agents(&mut self) -> Result<()> {
        let tasks = self.db.list_tasks()?;
        let watchers = self.db.list_watchers()?;

        self.agents.clear();
        for t in tasks {
            self.agents.push(AgentEntry::Task(t));
        }
        for w in watchers {
            self.agents.push(AgentEntry::Watcher(w));
        }
        // Append interactive agents
        for i in 0..self.interactive_agents.len() {
            self.agents.push(AgentEntry::Interactive(i));
        }

        let total = self.agents.len();
        if total > 0 && self.selected >= total {
            self.selected = total - 1;
        }

        Ok(())
    }

    fn refresh_active_runs(&mut self) -> Result<()> {
        self.active_runs.clear();
        for agent in &self.agents {
            let id = agent.id(self);
            if let Ok(Some(run)) = self.db.get_active_run(id) {
                self.active_runs.insert(id.to_string(), run);
            }
        }
        self.recent_runs = self.db.list_all_recent_runs(50)?;
        Ok(())
    }

    fn poll_interactive_agents(&mut self) {
        for agent in &mut self.interactive_agents {
            agent.poll();
        }

        // Remove exited agents and fix indices
        let mut removed_indices = Vec::new();
        for (i, agent) in self.interactive_agents.iter().enumerate() {
            if matches!(agent.status, AgentStatus::Exited(_)) {
                removed_indices.push(i);
            }
        }

        if removed_indices.is_empty() {
            return;
        }

        // Remove in reverse order and fix indices
        removed_indices.sort_unstable();
        removed_indices.reverse();

        for &old_idx in &removed_indices {
            self.interactive_agents.remove(old_idx);
        }

        // Fix all Interactive indices in agents list
        for agent in &mut self.agents {
            if let AgentEntry::Interactive(idx) = agent {
                // Count how many removed agents were before this index
                let shifts = removed_indices.iter().filter(|&&r| r < *idx).count();
                *idx -= shifts;
            }
        }

        // If the currently selected agent was removed, exit focus mode
        if self.focus == Focus::Agent {
            self.focus = Focus::Preview;
            if self.selected >= self.agents.len() && !self.agents.is_empty() {
                self.selected = self.agents.len() - 1;
            }
        }
    }

    /// Load the log/output for the currently selected agent.
    fn refresh_log(&mut self) {
        let Some(agent) = self.agents.get(self.selected) else {
            self.log_content = String::from("No agent selected");
            return;
        };

        match agent {
            AgentEntry::Interactive(idx) => {
                if *idx >= self.interactive_agents.len() {
                    self.log_content = String::from("Agent removed");
                    return;
                }
                let output = self.interactive_agents[*idx].output();
                self.log_content = if output.is_empty() {
                    format!(
                        "Agent '{}' — waiting for output...",
                        self.interactive_agents[*idx].id
                    )
                } else {
                    output
                };
            }
            _ => {
                let id = agent.id(self).to_string();
                let log_path = self.data_dir.join("logs").join(format!("{id}.log"));

                let mut content = match std::fs::read_to_string(&log_path) {
                    Ok(c) => tail_lines(&c, 200),
                    Err(_) => String::new(),
                };

                // If there's an active run, show status at the top
                if let Some(run) = self.active_runs.get(&id) {
                    let header = format!(
                        "⏳ Run {} in progress ({})\n{}\n",
                        &run.id[..8.min(run.id.len())],
                        relative_time(&run.started_at),
                        "─".repeat(40),
                    );
                    content = if content.is_empty() {
                        format!("{header}Waiting for output...")
                    } else {
                        format!("{header}{content}")
                    };
                } else if content.is_empty() {
                    content = format!("No logs yet for '{id}'");
                }

                self.log_content = content;
            }
        }
    }

    pub fn select_next(&mut self) {
        if !self.agents.is_empty() {
            self.selected = (self.selected + 1) % self.agents.len();
            self.log_scroll = 0;
        }
    }

    pub fn select_prev(&mut self) {
        if !self.agents.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.agents.len() - 1);
            self.log_scroll = 0;
        }
    }

    pub fn scroll_log_down(&mut self) {
        self.log_scroll = self.log_scroll.saturating_add(3);
    }

    pub fn scroll_log_up(&mut self) {
        self.log_scroll = self.log_scroll.saturating_sub(3);
    }

    pub fn selected_agent(&self) -> Option<&AgentEntry> {
        self.agents.get(self.selected)
    }

    /// Get the display ID for the selected agent.
    pub fn selected_id(&self) -> String {
        self.selected_agent()
            .map(|a| a.id(self).to_string())
            .unwrap_or_else(|| "—".to_string())
    }

    pub fn toggle_enable(&self) -> Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Task(t) => self.db.update_task_enabled(&t.id, !t.enabled)?,
            AgentEntry::Watcher(w) => self.db.update_watcher_enabled(&w.id, !w.enabled)?,
            AgentEntry::Interactive(_) => {}
        }
        Ok(())
    }

    pub fn rerun_selected(&self) -> Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Interactive(_) => Ok(()),
            _ => {
                let port = self
                    .db
                    .get_state("port")?
                    .unwrap_or_else(|| "7755".to_string());
                send_mcp_task_run(&port, agent.id(self))
            }
        }
    }

    pub fn open_new_agent_dialog(&mut self) {
        let prev_focus = self.focus;
        self.new_agent_dialog = Some(NewAgentDialog::new());
        // Store previous focus to restore on cancel
        self.new_agent_dialog.as_mut().unwrap().prev_focus = Some(prev_focus);
        self.focus = Focus::NewAgentDialog;
    }

    pub fn close_new_agent_dialog(&mut self) {
        // Restore the previous focus when closing the dialog
        if let Some(dialog) = &self.new_agent_dialog {
            if let Some(prev) = dialog.prev_focus {
                self.focus = prev;
            } else {
                self.focus = Focus::Home;
            }
        } else {
            self.focus = Focus::Home;
        }
        self.new_agent_dialog = None;
    }

    pub fn launch_new_agent(&mut self) -> Result<()> {
        let Some(dialog) = &self.new_agent_dialog else {
            return Ok(());
        };

        let model = if dialog.model.is_empty() {
            None
        } else {
            Some(dialog.model.clone())
        };

        match dialog.task_type {
            NewTaskType::Interactive => {
                let cli = dialog.selected_cli();
                let dir = dialog.working_dir.clone();
                let args = dialog.selected_args();
                let fallback = dialog.selected_fallback_args();
                let accent_color = dialog.selected_accent_color();
                // Use the actual panel inner dimensions stored by draw()
                let (cols, rows) = if self.last_panel_inner != (0, 0) {
                    self.last_panel_inner
                } else {
                    let (tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
                    (tw.saturating_sub(28), th.saturating_sub(4))
                };
                let agent = InteractiveAgent::spawn(
                    cli,
                    &dir,
                    cols,
                    rows,
                    args.as_deref(),
                    fallback.as_deref(),
                    accent_color,
                )?;
                self.interactive_agents.push(agent);
            }
            NewTaskType::Scheduled => {
                if dialog.prompt.is_empty() {
                    return Ok(());
                }
                let cli = dialog.selected_cli();
                let id = format!("task-{}", &uuid::Uuid::new_v4().to_string()[..8]);
                let working_dir = if dialog.working_dir.is_empty() {
                    std::env::current_dir()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| "/".to_string())
                } else {
                    dialog.working_dir.clone()
                };
                let task = crate::domain::models::Task {
                    id,
                    prompt: dialog.prompt.clone(),
                    schedule_expr: dialog.cron_expr.clone(),
                    cli,
                    model,
                    working_dir: Some(working_dir),
                    enabled: true,
                    created_at: Utc::now(),
                    last_run_at: None,
                    last_run_ok: None,
                    log_path: String::new(),
                    timeout_minutes: 15,
                    expires_at: None,
                };
                self.db.insert_or_update_task(&task)?;
            }
            NewTaskType::Watcher => {
                if dialog.prompt.is_empty() || dialog.watch_path.is_empty() {
                    return Ok(());
                }
                let cli = dialog.selected_cli();
                let id = format!("watch-{}", &uuid::Uuid::new_v4().to_string()[..8]);
                let events: Vec<_> = dialog
                    .watch_events
                    .iter()
                    .filter_map(|e| crate::domain::models::WatchEvent::from_str(e))
                    .collect();
                if events.is_empty() {
                    return Ok(());
                }
                let watcher = crate::domain::models::Watcher {
                    id,
                    path: dialog.watch_path.clone(),
                    events,
                    prompt: dialog.prompt.clone(),
                    cli,
                    model,
                    recursive: false,
                    debounce_seconds: 5,
                    enabled: true,
                    trigger_count: 0,
                    created_at: Utc::now(),
                    last_triggered_at: None,
                    timeout_minutes: 15,
                };
                self.db.insert_or_update_watcher(&watcher)?;
            }
        }

        self.refresh_agents()?;
        self.selected = self.agents.len().saturating_sub(1);

        // For interactive agents, go straight to Focus mode on the new agent.
        // For scheduled/watcher, go to Preview mode.
        let was_interactive = matches!(self.new_agent_dialog.as_ref(), Some(d) if matches!(d.task_type, NewTaskType::Interactive));
        self.close_new_agent_dialog();
        self.focus = if was_interactive {
            Focus::Agent
        } else {
            Focus::Preview
        };
        Ok(())
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
        // Exit focus mode after kill
        self.focus = Focus::Preview;
    }

    pub fn delete_selected(&mut self) -> Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Task(t) => {
                self.db.delete_task(&t.id)?;
            }
            AgentEntry::Watcher(w) => {
                self.db.delete_watcher(&w.id)?;
            }
            AgentEntry::Interactive(idx) => {
                self.interactive_agents[*idx].kill();
                self.interactive_agents.remove(*idx);
            }
        }
        self.refresh_agents()?;
        if self.selected >= self.agents.len() && !self.agents.is_empty() {
            self.selected = self.agents.len() - 1;
        }
        // Exit focus mode after delete
        self.focus = Focus::Preview;
        Ok(())
    }

    /// Clean up: kill all interactive agents on exit.
    pub fn cleanup(&mut self) {
        for agent in &mut self.interactive_agents {
            agent.kill();
        }
    }
}

pub fn relative_time(dt: &DateTime<Utc>) -> String {
    let delta = Utc::now().signed_duration_since(*dt);
    let secs = delta.num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

fn tail_lines(content: &str, n: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn send_mcp_task_run(port: &str, task_id: &str) -> Result<()> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "task_run",
            "arguments": { "id": task_id }
        }
    })
    .to_string();

    let request = format!(
        "POST /mcp HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    );

    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect_timeout(&addr.parse()?, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);
    Ok(())
}
