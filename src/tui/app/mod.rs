mod agents;
mod data;
mod dialog;

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::ports::{BackgroundAgentRepository, WatcherRepository};
use crate::db::Database;
use crate::domain::models::{BackgroundAgent, RunLog, Watcher};

use super::agent::InteractiveAgent;
use super::context_transfer::{ContextTransferConfig, ContextTransferModal, ContextTransferStep};

pub(crate) use data::send_mcp_task_run;
pub use dialog::NewAgentDialog;
pub use dialog::{NewTaskMode, NewTaskType};

// ── Types ───────────────────────────────────────────────────────

/// Unified entry in the sidebar.
pub enum AgentEntry {
    BackgroundAgent(BackgroundAgent),
    Watcher(Watcher),
    Interactive(usize), // index into App::interactive_agents
}

impl AgentEntry {
    pub fn id<'a>(&'a self, app: &'a App) -> &'a str {
        match self {
            Self::BackgroundAgent(t) => &t.id,
            Self::Watcher(w) => &w.id,
            Self::Interactive(idx) => &app.interactive_agents[*idx].id,
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
    pub last_esc: std::time::Instant,
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
    pub context_transfer_modal: Option<ContextTransferModal>,
    pub context_transfer_config: ContextTransferConfig,
    /// Whether to send OS-level desktop notifications (agent done/failed).
    pub notifications_enabled: bool,
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
            last_scroll_at: std::time::Instant::now() - std::time::Duration::from_secs(999),
            last_panel_inner: (0, 0),
            whimsg: super::whimsg::Whimsg::new(),
            context_transfer_modal: None,
            context_transfer_config: ContextTransferConfig::default(),
            notifications_enabled: true,
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
        }
        Ok(())
    }

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

        // Scan logs of selected agent for contextual triggers
        if let Some(agent) = self.agents.get(self.selected) {
            let log_to_scan = match agent {
                AgentEntry::Interactive(idx) => {
                    if let Some(ia) = self.interactive_agents.get(*idx) {
                        ia.last_lines(50).to_uppercase()
                    } else {
                        String::new()
                    }
                }
                _ => self.log_content.to_uppercase(),
            };

            if !log_to_scan.is_empty() {
                // Priority: Errors > Success > Spawning
                if log_to_scan.contains("ERROR")
                    || log_to_scan.contains("EXCEPTION")
                    || log_to_scan.contains("FAILED")
                    || log_to_scan.contains("CRITICAL")
                    || log_to_scan.contains("PANIC")
                    || log_to_scan.contains("SEGFAULT")
                    || log_to_scan.contains("TIMEOUT")
                    || log_to_scan.contains("HALTED")
                {
                    self.whimsg.notify_event(WhimContext::AgentFailed);
                } else if log_to_scan.contains("SUCCESS")
                    || log_to_scan.contains("DONE")
                    || log_to_scan.contains("FINISHED")
                    || log_to_scan.contains("COMPLETED")
                    || log_to_scan.contains("STABILIZED")
                    || log_to_scan.contains("READY")
                    || log_to_scan.contains("CONVERGED")
                {
                    self.whimsg.notify_event(WhimContext::AgentDone);
                } else if log_to_scan.contains("SPAWN")
                    || log_to_scan.contains("STARTING")
                    || log_to_scan.contains("BOOTSTRAP")
                    || log_to_scan.contains("INITIALIZING")
                {
                    self.whimsg.notify_event(WhimContext::AgentSpawned);
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

    // ── Context Transfer ────────────────────────────────────────

    /// Open the context transfer modal for the currently focused interactive agent.
    pub fn open_context_transfer_modal(&mut self) {
        let Some(AgentEntry::Interactive(idx)) = self.selected_agent() else {
            return;
        };
        let idx = *idx;
        if idx >= self.interactive_agents.len() {
            return;
        }

        let mut modal = ContextTransferModal::new(idx, &self.context_transfer_config);
        modal.refresh_preview(&self.interactive_agents[idx]);
        self.context_transfer_modal = Some(modal);
        self.focus = Focus::ContextTransfer;
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
    /// 2. Persists it (non-fatal on failure).
    /// 3. Injects into destination PTY.
    /// 4. Optionally switches tab to destination.
    pub fn execute_context_transfer(&mut self, dest_entry_idx: usize) {
        let Some(modal) = self.context_transfer_modal.take() else {
            return;
        };

        let src_idx = modal.source_agent_idx;
        if src_idx >= self.interactive_agents.len() {
            return;
        }

        // Destination: resolve the picker index to an interactive agent index
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

        let payload = super::context_transfer::build_context_payload(
            &self.interactive_agents[src_idx],
            modal.n_prompts,
        );

        let _ = self.interactive_agents[src_idx].working_dir.clone(); // source workdir (available if needed)

        let _ = self.interactive_agents[dest_ia_idx].inject_context(&payload);

        if self.context_transfer_config.auto_switch_tab {
            // Find the sidebar entry index that points to dest_ia_idx
            if let Some(entry_pos) = self
                .agents
                .iter()
                .position(|a| matches!(a, AgentEntry::Interactive(i) if *i == dest_ia_idx))
            {
                self.selected = entry_pos;
            }
            self.focus = Focus::Agent;
        } else {
            self.focus = Focus::Agent;
        }
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

            // Use resume_args if available, otherwise fall back to original args
            let args = resume_args.or(session.args.as_deref());

            let existing_ids: Vec<&str> =
                self.interactive_agents.iter().map(|a| a.id.as_str()).collect();

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
            ) {
                Ok(agent) => {
                    let _ = self.db.insert_interactive_session(
                        &agent.id,
                        &agent.id,
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
