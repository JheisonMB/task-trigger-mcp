mod agents;
mod data;
pub mod dialog;

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::notification_service::{DefaultNotificationService, NotificationService};
use crate::application::ports::{BackgroundAgentRepository, WatcherRepository};
use crate::db::Database;
use crate::domain::models::{BackgroundAgent, RunLog, Watcher};

use super::agent::InteractiveAgent;
use super::context_transfer::{ContextTransferConfig, ContextTransferModal, ContextTransferStep};
use crate::tui::prompt_templates::PromptTemplates;
use dialog::SimplePromptDialog;

pub(crate) use data::send_mcp_task_run;
pub use dialog::NewAgentDialog;
pub use dialog::{NewTaskMode, NewTaskType};

// ── Types ───────────────────────────────────────────────────────

/// Unified entry in the sidebar.
pub enum AgentEntry {
    BackgroundAgent(BackgroundAgent),
    Watcher(Watcher),
    Interactive(usize), // index into App::interactive_agents
    Terminal(usize),    // index into App::terminal_agents
    Group(usize),       // index into App::split_groups
}

impl AgentEntry {
    pub fn id<'a>(&'a self, app: &'a App) -> &'a str {
        match self {
            Self::BackgroundAgent(t) => &t.id,
            Self::Watcher(w) => &w.id,
            Self::Interactive(idx) => app.interactive_agents.get(*idx).map_or("?", |a| &a.name),
            Self::Terminal(idx) => app.terminal_agents.get(*idx).map_or("?", |a| &a.name),
            Self::Group(idx) => app.split_groups.get(*idx).map_or("?", |g| &g.id),
        }
    }
}

/// Which panel has focus.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Home,
    Preview,
    NewAgentDialog,
    Agent,
    ContextTransfer,
    PromptTemplateDialog,
}

// ── App struct ──────────────────────────────────────────────────

/// Main application state.
pub struct App {
    pub db: Arc<Database>,
    pub data_dir: PathBuf,

    // Data cache (refreshed every tick)
    pub agents: Vec<AgentEntry>,
    pub active_runs: HashMap<String, RunLog>,
    pub recent_runs: Vec<RunLog>,
    pub interactive_agents: Vec<InteractiveAgent>,
    /// Raw terminal sessions (no AI CLI).
    pub terminal_agents: Vec<InteractiveAgent>,

    // Split group state
    pub split_groups: Vec<crate::domain::models::SplitGroup>,
    /// ID of the split group currently being viewed (if any).
    pub active_split_id: Option<String>,
    /// True = right/bottom panel is focused in split view.
    pub split_right_focused: bool,
    /// Whether the split picker overlay is open.
    pub split_picker_open: bool,
    pub split_picker_idx: usize,
    pub split_picker_orientation: crate::domain::models::SplitOrientation,
    /// (name, type_label) for each available session in the picker.
    pub split_picker_sessions: Vec<(String, String)>,

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
    pub quit_confirm: bool,

    // Brian's Brain automaton
    pub brain: Option<super::brians_brain::BriansBrain>,

    // Layout state
    pub sidebar_click_map: Vec<(usize, u16, u16)>,
    pub sidebar_visible: bool,
    pub term_width: u16,
    pub show_legend: bool,
    pub show_copied: bool,
    pub copied_at: std::time::Instant,
    pub last_scroll_at: std::time::Instant,
    pub last_panel_inner: (u16, u16),
    pub whimsg: super::whimsg::Whimsg,
    /// Hash of the last log chunk scanned for whimsg triggers — avoids re-firing
    /// on the same content every tick.
    whimsg_last_log_hash: u64,
    pub context_transfer_modal: Option<ContextTransferModal>,
    pub context_transfer_config: ContextTransferConfig,
    /// Prompt templates loaded from registry
    #[allow(dead_code)]
    pub prompt_templates: PromptTemplates,
    /// Current simple prompt dialog state
    pub simple_prompt_dialog: Option<SimplePromptDialog>,
    /// Whether to send OS-level desktop notifications (agent done/failed).
    pub notifications_enabled: bool,
    /// Notification service for sending cross-platform notifications.
    pub notification_service: Arc<dyn NotificationService>,
    /// IDs of runs that were active on the previous refresh tick.
    prev_active_run_ids: std::collections::HashSet<String>,
    /// Tick counter for animation (increments every refresh)
    pub animation_tick: u32,
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
            terminal_agents: Vec::new(),
            split_groups: Vec::new(),
            active_split_id: None,
            split_right_focused: false,
            split_picker_open: false,
            split_picker_idx: 0,
            split_picker_orientation: crate::domain::models::SplitOrientation::Horizontal,
            split_picker_sessions: Vec::new(),
            daemon_running: false,
            daemon_pid: None,
            daemon_version: String::new(),
            selected: 0,
            focus: Focus::Home,
            log_content: String::new(),
            log_scroll: 0,
            running: true,
            new_agent_dialog: None,
            quit_confirm: false,
            brain: None,
            sidebar_click_map: Vec::new(),
            sidebar_visible: true,
            term_width: 0,
            show_legend: false,
            show_copied: false,
            copied_at: std::time::Instant::now() - std::time::Duration::from_secs(10),
            last_scroll_at: std::time::Instant::now() - std::time::Duration::from_secs(999),
            last_panel_inner: (0, 0),
            whimsg: super::whimsg::Whimsg::new(),
            whimsg_last_log_hash: 0,
            context_transfer_modal: None,
            context_transfer_config: ContextTransferConfig::default(),
            prompt_templates: PromptTemplates::load_from_registry()
                .unwrap_or_else(|_| PromptTemplates::internal_templates()),
            simple_prompt_dialog: None,
            notifications_enabled: true,
            notification_service: Arc::new(DefaultNotificationService),
            prev_active_run_ids: std::collections::HashSet::new(),
            animation_tick: 0,
        };
        app.refresh()?;
        Ok(app)
    }

    /// Reload all data from the database and filesystem.
    pub fn refresh(&mut self) -> Result<()> {
        self.animation_tick = self.animation_tick.wrapping_add(1);
        self.refresh_daemon_status();
        self.refresh_agents()?;
        self.refresh_active_runs()?;
        self.poll_interactive_agents();
        self.poll_terminal_agents();
        self.tick_brians_brain();
        self.refresh_log();
        self.auto_hide_sidebar();
        self.dismiss_copied();
        self.update_whimsg_context();
        self.resize_interactive_agents();
        Ok(())
    }

    // ── Navigation ──────────────────────────────────────────────

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
            AgentEntry::BackgroundAgent(t) => {
                self.db.update_background_agent_enabled(&t.id, !t.enabled)?;
            }
            AgentEntry::Watcher(w) => {
                self.db.update_watcher_enabled(&w.id, !w.enabled)?;
            }
            AgentEntry::Interactive(_) => {}
            AgentEntry::Terminal(_) => {}
            AgentEntry::Group(_) => {}
        }
        Ok(())
    }

    fn auto_hide_sidebar(&mut self) {
        if let Ok((tw, _th)) = ratatui::crossterm::terminal::size() {
            self.term_width = tw;
            let should_hide = self.focus == Focus::Agent
                && self.selected_agent().is_some_and(|a| {
                    matches!(a, AgentEntry::Interactive(_) | AgentEntry::Terminal(_))
                })
                && tw < 80;
            let should_show = tw >= 80 && !self.sidebar_visible;
            if should_hide {
                self.sidebar_visible = false;
            } else if should_show {
                self.sidebar_visible = true;
            }
        }
    }

    fn update_whimsg_context(&mut self) {
        use crate::tui::whimsg::WhimContext;
        use std::time::Duration;

        // CRITICAL: If daemon is down, everything is an error state
        if !self.daemon_running {
            self.whimsg.set_ambient(WhimContext::AgentFailed);
            self.whimsg.notify_event(WhimContext::AgentFailed);
            return;
        }

        // Check global health: recent background agent failures
        let now = Utc::now();
        for run in &self.recent_runs {
            if let Some(finished) = run.finished_at {
                let seconds_since = (now - finished).num_seconds();
                if seconds_since < 60 {
                    match run.status {
                        crate::domain::models::RunStatus::Error
                        | crate::domain::models::RunStatus::Timeout => {
                            self.whimsg.notify_event(WhimContext::AgentFailed);
                        }
                        crate::domain::models::RunStatus::Success => {
                            self.whimsg.notify_event(WhimContext::AgentDone);
                        }
                        _ => {}
                    }
                }
            }
        }

        // Check if user scrolled recently
        if self.last_scroll_at.elapsed() < Duration::from_secs(5) {
            self.whimsg.set_ambient(WhimContext::Scrolling);
            return;
        }

        // Scan logs of selected agent for contextual triggers.
        // Only re-evaluate when the log content actually changes.
        if let Some(agent) = self.agents.get(self.selected) {
            let raw_log = match agent {
                AgentEntry::Interactive(idx) => {
                    if let Some(ia) = self.interactive_agents.get(*idx) {
                        ia.last_lines(50)
                    } else {
                        String::new()
                    }
                }
                AgentEntry::Terminal(idx) => {
                    if let Some(ia) = self.terminal_agents.get(*idx) {
                        ia.last_lines(50)
                    } else {
                        String::new()
                    }
                }
                _ => self.log_content.clone(),
            };

            if !raw_log.is_empty() {
                // Simple hash to detect changes — avoid re-firing on the same content
                let log_hash: u64 = raw_log.bytes().enumerate().fold(0u64, |acc, (i, b)| {
                    acc.wrapping_add((b as u64).wrapping_mul(i as u64 + 1))
                });

                if log_hash != self.whimsg_last_log_hash {
                    self.whimsg_last_log_hash = log_hash;

                    let log_up = raw_log.to_uppercase();

                    // Error keywords — specific phrases to reduce false positives.
                    // Avoid single "ERROR" or "FAILED" which appear in normal agent output
                    // (e.g. "no errors found", "error handling", "failed test cases: 0").
                    let is_error = log_up.contains("ERROR")
                        || log_up.contains("FAILED")
                        || log_up.contains("EXCEPTION")
                        || log_up.contains("PANIC")
                        || log_up.contains("SEGFAULT")
                        || log_up.contains("TIMED OUT")
                        || log_up.contains("CONNECTION REFUSED")
                        || log_up.contains("PERMISSION DENIED")
                        || log_up.contains("HALTED")
                        // Spanish
                        || log_up.contains("PROBLEMA")
                        || log_up.contains("FALLO")
                        || log_up.contains("FALLANDO");

                    let is_success = log_up.contains("SUCCESS")
                        || log_up.contains("ALL TESTS PASSED")
                        || log_up.contains("BUILD SUCCEEDED")
                        || log_up.contains("FINISHED")
                        || log_up.contains("COMPLETED")
                        || log_up.contains("DONE.")
                        || log_up.contains("STABILIZED")
                        || log_up.contains("READY")
                        || log_up.contains("CONVERGED")
                        || log_up.contains("DEPLOYED")
                        // Spanish
                        || log_up.contains("EXCELENTE")
                        || log_up.contains("COMPLETADO")
                        || log_up.contains("HECHO")
                        || log_up.contains("LISTO")
                        || log_up.contains("TERMINADO");

                    let is_spawn = log_up.contains("SPAWNING")
                        || log_up.contains("STARTING UP")
                        || log_up.contains("BOOTSTRAPPING")
                        || log_up.contains("INITIALIZING");

                    if is_error {
                        self.whimsg.notify_event(WhimContext::AgentFailed);
                    } else if is_success {
                        self.whimsg.notify_event(WhimContext::AgentDone);
                    } else if is_spawn {
                        self.whimsg.notify_event(WhimContext::AgentSpawned);
                    }
                }
            }
        }

        // Check how many interactive agents are running
        let running = self
            .interactive_agents
            .iter()
            .filter(|a| a.status == crate::tui::agent::AgentStatus::Running)
            .count();
        let has_active_runs = !self.active_runs.is_empty();

        if running >= 3 || (running >= 1 && has_active_runs) {
            self.whimsg.set_ambient(WhimContext::Busy);
        } else if has_active_runs {
            self.whimsg.set_ambient(WhimContext::TaskRunning);
        } else {
            self.whimsg.set_ambient(WhimContext::Idle);
        }
    }

    // ── Split Groups ────────────────────────────────────────────

    /// Open the split picker to pair the current session with another.
    pub fn open_split_picker(&mut self) {
        let mut sessions: Vec<(String, String)> = Vec::new();
        for a in &self.interactive_agents {
            sessions.push((a.name.clone(), "Interactive".to_string()));
        }
        for a in &self.terminal_agents {
            sessions.push((a.name.clone(), "Terminal".to_string()));
        }
        if sessions.len() < 2 {
            return;
        }
        self.split_picker_sessions = sessions;
        self.split_picker_idx = 0;
        self.split_picker_orientation = crate::domain::models::SplitOrientation::Horizontal;
        self.split_picker_open = true;
    }

    /// Create a split group from the current session and the picker selection.
    pub fn create_split(&mut self) {
        let current_name = match self.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => self.interactive_agents[*idx].name.clone(),
            Some(AgentEntry::Terminal(idx)) => self.terminal_agents[*idx].name.clone(),
            _ => return,
        };
        let Some((other_name, _)) = self
            .split_picker_sessions
            .get(self.split_picker_idx)
            .cloned()
        else {
            return;
        };
        if current_name == other_name {
            return;
        }
        let id = format!("split-{}", &uuid::Uuid::new_v4().to_string()[..8]);
        let group = crate::domain::models::SplitGroup {
            id: id.clone(),
            orientation: self.split_picker_orientation,
            session_a: current_name,
            session_b: other_name,
            created_at: Utc::now(),
        };
        let _ = self.db.insert_group(
            &group.id,
            group.orientation.as_str(),
            &group.session_a,
            &group.session_b,
        );
        self.active_split_id = Some(id);
        self.split_groups.push(group);
        self.split_picker_open = false;
        // Immediately enter split view in agent focus
        self.split_right_focused = false;
        self.focus = Focus::Agent;
    }

    /// Dissolve the currently active split group.
    pub fn dissolve_split(&mut self) {
        if let Some(id) = self.active_split_id.take() {
            let _ = self.db.delete_group(&id);
            self.split_groups.retain(|g| g.id != id);
        }
        self.split_picker_open = false;
    }

    // ── Context Transfer ────────────────────────────────────────

    /// Open the context transfer modal for the currently focused interactive or terminal agent.
    pub fn open_context_transfer_modal(&mut self) {
        match self.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => {
                let idx = *idx;
                if idx >= self.interactive_agents.len() {
                    return;
                }
                let mut modal = ContextTransferModal::new(idx, &self.context_transfer_config);
                modal.refresh_preview(&self.interactive_agents[idx]);
                self.context_transfer_modal = Some(modal);
                self.focus = Focus::ContextTransfer;
            }
            Some(AgentEntry::Terminal(idx)) => {
                let idx = *idx;
                if idx >= self.terminal_agents.len() {
                    return;
                }
                let mut modal =
                    ContextTransferModal::new_terminal(idx, &self.context_transfer_config);
                modal.refresh_preview(&self.terminal_agents[idx]);
                self.context_transfer_modal = Some(modal);
                self.focus = Focus::ContextTransfer;
            }
            _ => {}
        }
    }

    /// Open context transfer for the focused split panel's session.
    pub fn open_context_transfer_for_split(&mut self) {
        let session_name = match &self.active_split_id {
            Some(id) => self.split_groups.iter().find(|g| g.id == *id).map(|g| {
                if self.split_right_focused {
                    g.session_b.clone()
                } else {
                    g.session_a.clone()
                }
            }),
            None => return,
        };
        let Some(name) = session_name else { return };

        if let Some(idx) = self.interactive_agents.iter().position(|a| a.name == name) {
            let mut modal = ContextTransferModal::new(idx, &self.context_transfer_config);
            modal.refresh_preview(&self.interactive_agents[idx]);
            self.context_transfer_modal = Some(modal);
            self.focus = Focus::ContextTransfer;
        } else if let Some(idx) = self.terminal_agents.iter().position(|a| a.name == name) {
            let mut modal = ContextTransferModal::new_terminal(idx, &self.context_transfer_config);
            modal.refresh_preview(&self.terminal_agents[idx]);
            self.context_transfer_modal = Some(modal);
            self.focus = Focus::ContextTransfer;
        }
    }

    /// Close the modal and return focus to the agent.
    pub fn close_context_transfer_modal(&mut self) {
        self.context_transfer_modal = None;
        self.focus = Focus::Agent;
    }

    /// Advance the modal from Preview to AgentPicker.
    pub fn context_transfer_to_picker(&mut self) {
        if let Some(modal) = &mut self.context_transfer_modal {
            if modal.step == ContextTransferStep::Preview {
                modal.step = ContextTransferStep::AgentPicker;
                modal.picker_selected = 0;
            }
        }
    }

    /// Execute the context transfer to the selected destination agent.
    ///
    /// 1. Builds the payload.
    /// 2. Switches focus to destination.
    /// 3. Opens Prompt Template dialog with payload pre-filled in the "context" section.
    pub fn execute_context_transfer(&mut self, dest_entry_idx: usize) {
        let Some(modal) = self.context_transfer_modal.take() else {
            return;
        };

        let src_idx = modal.source_agent_idx;
        let source_is_terminal = modal.source_is_terminal;

        // Validate source index
        if source_is_terminal && src_idx >= self.terminal_agents.len() {
            return;
        }
        if !source_is_terminal && src_idx >= self.interactive_agents.len() {
            return;
        }

        let dest_agent_idx = {
            let picker_entries = self.picker_interactive_entries();
            picker_entries.get(dest_entry_idx).copied()
        };
        let Some(dest_ia_idx) = dest_agent_idx else {
            return;
        };

        if dest_ia_idx >= self.interactive_agents.len() {
            return;
        }

        let payload = if source_is_terminal {
            super::context_transfer::build_terminal_context_payload(
                &self.terminal_agents[src_idx],
                modal.n_prompts,
            )
        } else {
            super::context_transfer::build_context_payload(
                &self.interactive_agents[src_idx],
                modal.n_prompts,
            )
        };

        // Always switch tab to destination so the user sees where the context is going
        if let Some(entry_pos) = self
            .agents
            .iter()
            .position(|a| matches!(a, AgentEntry::Interactive(i) if *i == dest_ia_idx))
        {
            self.selected = entry_pos;
        }
        self.focus = Focus::Agent;

        // Prepare initial content for the simple prompt dialog
        let mut initial_content = HashMap::new();
        // Always put context transfer content in the "context" section
        initial_content.insert("context".to_string(), payload);

        // Open the prompt template dialog with the pre-filled context
        self.open_simple_prompt_dialog(Some(initial_content));
    }

    /// Collect interactive agent indices for use in the picker list.
    pub fn picker_interactive_entries(&self) -> Vec<usize> {
        self.interactive_agents
            .iter()
            .enumerate()
            .map(|(i, _)| i)
            .collect()
    }

    /// Auto-resume previously active interactive sessions from the registry.
    ///
    /// On startup, any sessions marked 'active' are from a previous canopy run
    /// where the PTY processes have since died. For CLIs that support resume
    /// (e.g. `--continue`), we re-launch in resume mode in the same directory.
    pub fn auto_resume_sessions(&mut self) {
        let Ok(sessions) = self.db.get_active_sessions() else {
            return;
        };

        if sessions.is_empty() {
            tracing::info!("No active sessions to resume");
            return;
        }

        tracing::info!("Resuming {} active session(s)", sessions.len());

        // Mark all old active sessions as orphaned first
        let _ = self.db.mark_orphaned_sessions();

        let home = dirs::home_dir().unwrap_or_default();
        let config_path = home.join(".canopy/cli_config.json");
        let registry = crate::domain::cli_config::CliRegistry::load(&config_path);

        let (cols, rows) = {
            let (tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
            (tw.saturating_sub(28), th.saturating_sub(4))
        };

        for session in &sessions {
            let cli = crate::domain::models::Cli::from_str(&session.cli);

            // Get CLI config for resume args and accent color
            let cli_config = registry.as_ref().and_then(|r| r.get(cli.as_str()));
            let resume_args = cli_config.and_then(|c| c.resume_args.as_deref());
            let fallback = cli_config.and_then(|c| c.fallback_interactive_args.as_deref());
            let accent = cli_config
                .and_then(|c| c.accent_color)
                .map(|[r, g, b]| ratatui::style::Color::Rgb(r, g, b))
                .unwrap_or(ratatui::style::Color::Rgb(102, 187, 106));

            // Use resume_args if available, otherwise fall back to original args.
            // If the original session was launched with the yolo flag, preserve it.
            let yolo_flag = cli_config.and_then(|c| c.yolo_flag.as_deref());
            let had_yolo = yolo_flag
                .map(|flag| {
                    session
                        .args
                        .as_deref()
                        .unwrap_or("")
                        .split_whitespace()
                        .any(|a| a == flag)
                })
                .unwrap_or(false);

            let args_str: Option<String> = if let Some(ra) = resume_args {
                if had_yolo {
                    Some(format!("{} {}", ra, yolo_flag.unwrap()))
                } else {
                    Some(ra.to_string())
                }
            } else {
                session.args.clone()
            };
            let args = args_str.as_deref();

            let existing_ids: Vec<&str> = self
                .interactive_agents
                .iter()
                .map(|a| a.name.as_str())
                .collect();

            match super::agent::InteractiveAgent::spawn(
                cli.clone(),
                &session.working_dir,
                cols,
                rows,
                args,
                fallback,
                accent,
                Some(&session.name),
                &existing_ids,
                None,
                None,
            ) {
                Ok(agent) => {
                    let _ = self.db.insert_interactive_session(
                        &agent.id,
                        &agent.name,
                        cli.as_str(),
                        &session.working_dir,
                        args,
                    );
                    self.interactive_agents.push(agent);
                }
                Err(e) => {
                    tracing::warn!("Failed to auto-resume session '{}': {e}", session.name);
                }
            }
        }

        if !self.interactive_agents.is_empty() {
            let _ = self.refresh_agents();
        }
    }

    /// Auto-resume previously active terminal sessions.
    ///
    /// Terminal sessions are simpler than interactive — no CLI resume args needed,
    /// just re-spawn a shell in the same working directory with the same name.
    pub fn auto_resume_terminal_sessions(&mut self) {
        let Ok(sessions) = self.db.get_active_terminal_sessions() else {
            return;
        };

        if sessions.is_empty() {
            tracing::info!("No active terminal sessions to resume");
            return;
        }

        tracing::info!("Resuming {} terminal session(s)", sessions.len());
        let _ = self.db.mark_orphaned_terminal_sessions();

        let (cols, rows) = {
            let (tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
            (tw.saturating_sub(28), th.saturating_sub(4))
        };

        for session in &sessions {
            let existing_refs: Vec<&str> = self
                .terminal_agents
                .iter()
                .map(|a| a.name.as_str())
                .collect();

            match super::agent::InteractiveAgent::spawn_terminal(
                &session.shell,
                &session.working_dir,
                cols,
                rows,
                Some(&session.name),
                &existing_refs,
                ratatui::style::Color::Green,
            ) {
                Ok(agent) => {
                    let _ = self.db.insert_terminal_session(
                        &agent.id,
                        &agent.name,
                        &session.shell,
                        &session.working_dir,
                    );
                    self.terminal_agents.push(agent);
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to auto-resume terminal session '{}': {e}",
                        session.name
                    );
                }
            }
        }

        if !self.terminal_agents.is_empty() {
            let _ = self.refresh_agents();
        }
    }
}

// ── Free functions ──────────────────────────────────────────────

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

pub(super) fn tail_lines(content: &str, n: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

pub(super) fn is_process_running(pid: u32) -> bool {
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
