mod agents;
mod data;
pub mod dialog;

use anyhow::Result;
use chrono::Utc;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::notification_service::DefaultNotificationService;
use crate::application::ports::AgentRepository;
use crate::db::Database;

use super::agent::InteractiveAgent;
use super::context_transfer::{
    build_context_payload_for, ContextSourceKind, ContextTransferConfig, ContextTransferModal,
    ContextTransferStep,
};
use crate::tui::prompt_templates::PromptTemplates;

pub(crate) use data::send_mcp_task_run;
pub use dialog::{BackgroundTrigger, NewTaskMode, NewTaskType};

// ── Types ───────────────────────────────────────────────────────

pub mod session_resume;
pub mod terminal_search;
pub mod types;
pub mod utils;

pub(crate) use session_resume::build_resumed_session_args;
pub use terminal_search::TerminalSearch;
pub(crate) use types::ContextTransferSource;
pub use types::{AgentEntry, App, Focus};

impl App {
    pub fn new(db: Arc<Database>, data_dir: &Path) -> Result<Self> {
        let home = dirs::home_dir().unwrap_or_default();
        let canopy_dir = home.join(".canopy");
        let canopy_config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);

        let (system_info_tx, system_info_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let initial = crate::system::SystemInfo::new();
            let _ = system_info_tx.send(initial);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let mut info = crate::system::SystemInfo::default();
                info.update();
                let _ = system_info_tx.send(info);
            }
        });

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
            sidebar_brain: None,
            home_brain: None,
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
            prompt_builder_sessions: HashMap::new(),
            notifications_enabled: true,
            notification_service: Arc::new(DefaultNotificationService),
            prev_active_run_ids: std::collections::HashSet::new(),
            animation_tick: 0,
            temperature_unit: canopy_config.temperature_unit,
            suggestion_picker: None,
            terminal_histories: HashMap::new(),
            terminal_search: None,
            system_info: crate::system::SystemInfo::default(),
            system_info_rx,
            last_system_update: std::time::Instant::now() - std::time::Duration::from_secs(10),
            process_start_time: std::time::Instant::now(),
            cli_usage: {
                let mut usage = dirs::home_dir()
                    .map(|h| crate::domain::usage_stats::CliUsage::load(&h.join(".canopy")))
                    .unwrap_or_default();
                if usage.ensure_first_run() {
                    let _ = dirs::home_dir()
                        .and_then(|h| usage.save(&h.join(".canopy")).ok().map(|_| ()));
                }
                usage
            },
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
        self.tick_banner_glitch();
        self.ensure_sidebar_brain();
        self.refresh_log();
        self.auto_hide_sidebar();
        self.dismiss_copied();
        self.update_whimsg_context();
        self.resize_interactive_agents();

        // Non-blocking check for updated system info from background thread
        while let Ok(info) = self.system_info_rx.try_recv() {
            self.system_info = info;
            self.last_system_update = std::time::Instant::now();
        }

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

    /// Return the working directory of the currently selected agent,
    /// or the parent of the data directory as a fallback.
    pub fn current_workdir(&self) -> PathBuf {
        self.selected_agent()
            .and_then(|a| match a {
                AgentEntry::Interactive(idx) => self
                    .interactive_agents
                    .get(*idx)
                    .map(|ia| PathBuf::from(&ia.working_dir)),
                AgentEntry::Terminal(idx) => self
                    .terminal_agents
                    .get(*idx)
                    .map(|ta| PathBuf::from(&ta.working_dir)),
                _ => None,
            })
            .unwrap_or_else(|| {
                self.data_dir
                    .parent()
                    .unwrap_or(&self.data_dir)
                    .to_path_buf()
            })
    }

    pub fn focused_agent_name(&self) -> String {
        match self.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => {
                self.interactive_agents.get(*idx).map(|a| a.name.clone())
            }
            Some(AgentEntry::Terminal(idx)) => {
                self.terminal_agents.get(*idx).map(|a| a.name.clone())
            }
            _ => None,
        }
        .unwrap_or_default()
    }

    pub fn selected_id(&self) -> String {
        self.selected_agent()
            .map(|a| a.id(self).to_string())
            .unwrap_or_else(|| "—".to_string())
    }

    /// Record a CLI launch in usage stats and persist to disk.
    pub fn record_cli_usage(&mut self, cli_name: &str) {
        self.cli_usage.record(cli_name);
        let _ =
            dirs::home_dir().and_then(|h| self.cli_usage.save(&h.join(".canopy")).ok().map(|_| ()));
    }

    pub fn toggle_enable(&self) -> Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Agent(a) => {
                self.db.update_agent_enabled(&a.id, !a.enabled)?;
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
        let source = self.selected_context_transfer_source();
        self.open_context_transfer_from_source(source);
    }

    /// Open context transfer for the focused split panel's session.
    pub fn open_context_transfer_for_split(&mut self) {
        let source = self
            .active_split_session_name()
            .and_then(|name| self.context_transfer_source_by_name(&name));
        self.open_context_transfer_from_source(source);
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

        let Some(payload) = self.build_context_transfer_payload(&modal) else {
            return;
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

    pub(crate) fn refresh_context_transfer_preview(&mut self) {
        let Some((source, n_prompts)) = self.context_transfer_modal.as_ref().and_then(|modal| {
            self.modal_source(modal)
                .map(|source| (source, modal.n_prompts))
        }) else {
            return;
        };

        let Some(preview) = self.build_context_transfer_payload_from_source(source, n_prompts)
        else {
            return;
        };

        if let Some(modal) = self.context_transfer_modal.as_mut() {
            modal.payload_preview = preview;
        }
    }

    pub(crate) fn context_transfer_max_units(&self) -> Option<usize> {
        let modal = self.context_transfer_modal.as_ref()?;
        let max_units = match self.modal_source(modal)? {
            ContextTransferSource::Interactive(idx) => self
                .interactive_agents
                .get(idx)
                .and_then(|agent| {
                    agent
                        .prompt_history
                        .lock()
                        .ok()
                        .map(|history| history.len())
                })
                .unwrap_or(0)
                .max(1),
            ContextTransferSource::Terminal(_) => 20,
        };
        Some(max_units)
    }

    fn selected_context_transfer_source(&self) -> Option<ContextTransferSource> {
        match self.selected_agent()? {
            AgentEntry::Interactive(idx) => self
                .interactive_agents
                .get(*idx)
                .map(|_| ContextTransferSource::Interactive(*idx)),
            AgentEntry::Terminal(idx) => self
                .terminal_agents
                .get(*idx)
                .map(|_| ContextTransferSource::Terminal(*idx)),
            _ => None,
        }
    }

    fn active_split_session_name(&self) -> Option<String> {
        let split_id = self.active_split_id.as_ref()?;
        let group = self
            .split_groups
            .iter()
            .find(|group| group.id == *split_id)?;
        Some(if self.split_right_focused {
            group.session_b.clone()
        } else {
            group.session_a.clone()
        })
    }

    fn context_transfer_source_by_name(&self, name: &str) -> Option<ContextTransferSource> {
        if let Some(idx) = self
            .interactive_agents
            .iter()
            .position(|agent| agent.name == name)
        {
            return Some(ContextTransferSource::Interactive(idx));
        }
        self.terminal_agents
            .iter()
            .position(|agent| agent.name == name)
            .map(ContextTransferSource::Terminal)
    }

    fn open_context_transfer_from_source(&mut self, source: Option<ContextTransferSource>) {
        let Some(source) = source else {
            return;
        };

        let Some(mut modal) = self.modal_for_context_transfer_source(source) else {
            return;
        };

        if let Some(preview) = self.build_context_transfer_payload(&modal) {
            modal.payload_preview = preview;
        }

        self.context_transfer_modal = Some(modal);
        self.focus = Focus::ContextTransfer;
    }

    fn modal_for_context_transfer_source(
        &self,
        source: ContextTransferSource,
    ) -> Option<ContextTransferModal> {
        match source {
            ContextTransferSource::Interactive(idx) => self
                .interactive_agents
                .get(idx)
                .map(|_| ContextTransferModal::new(idx, &self.context_transfer_config)),
            ContextTransferSource::Terminal(idx) => self
                .terminal_agents
                .get(idx)
                .map(|_| ContextTransferModal::new_terminal(idx, &self.context_transfer_config)),
        }
    }

    fn modal_source(&self, modal: &ContextTransferModal) -> Option<ContextTransferSource> {
        match modal.source_kind() {
            ContextSourceKind::Interactive => self
                .interactive_agents
                .get(modal.source_agent_idx)
                .map(|_| ContextTransferSource::Interactive(modal.source_agent_idx)),
            ContextSourceKind::Terminal => self
                .terminal_agents
                .get(modal.source_agent_idx)
                .map(|_| ContextTransferSource::Terminal(modal.source_agent_idx)),
        }
    }

    fn build_context_transfer_payload(&self, modal: &ContextTransferModal) -> Option<String> {
        self.build_context_transfer_payload_from_source(self.modal_source(modal)?, modal.n_prompts)
    }

    fn build_context_transfer_payload_from_source(
        &self,
        source: ContextTransferSource,
        n_prompts: usize,
    ) -> Option<String> {
        match source {
            ContextTransferSource::Interactive(idx) => {
                self.interactive_agents.get(idx).map(|agent| {
                    build_context_payload_for(agent, n_prompts, ContextSourceKind::Interactive)
                })
            }
            ContextTransferSource::Terminal(idx) => self.terminal_agents.get(idx).map(|agent| {
                build_context_payload_for(agent, n_prompts, ContextSourceKind::Terminal)
            }),
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
        let canopy_dir = home.join(".canopy");
        let canopy_config = crate::domain::canopy_config::CanopyConfig::load(&canopy_dir);

        let (cols, rows) = {
            let (tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
            (tw.saturating_sub(28), th.saturating_sub(4))
        };

        for session in &sessions {
            let cli = crate::domain::models::Cli::from_str(&session.cli);

            // Get CLI config for interactive args and accent color
            let cli_config = canopy_config.get_cli(cli.as_str());
            let interactive_args = cli_config.and_then(|c| c.interactive_args.as_deref());
            let fallback = cli_config.and_then(|c| c.fallback_interactive_args.as_deref());
            let accent = cli_config
                .and_then(|c| c.accent_color)
                .map(|[r, g, b]| ratatui::style::Color::Rgb(r, g, b))
                .unwrap_or(ratatui::style::Color::Rgb(102, 187, 106));

            let yolo_flag = cli_config.and_then(|c| c.yolo_flag.as_deref());
            let args_str = build_resumed_session_args(session, interactive_args, yolo_flag);
            let args = args_str.as_deref();
            let model: Option<String> = None; // No model info in session registry
            let model_flag = cli_config.and_then(|c| c.model_flag.clone());

            let existing_ids: Vec<&str> = self
                .interactive_agents
                .iter()
                .map(|a| a.name.as_str())
                .collect();

            match InteractiveAgent::spawn(
                cli.clone(),
                &session.working_dir,
                cols,
                rows,
                args,
                fallback,
                accent,
                Some(&session.name),
                &existing_ids,
                model.as_deref(),
                model_flag.as_deref(),
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

            match InteractiveAgent::spawn_terminal(
                &session.shell,
                &session.working_dir,
                cols,
                rows,
                Some(&session.name),
                &existing_refs,
                crate::tui::ui::ACCENT,
            ) {
                Ok(agent) => {
                    let _ = self.db.insert_terminal_session(
                        &agent.id,
                        &agent.name,
                        &session.shell,
                        &session.working_dir,
                    );
                    // Load command history into cache
                    let hist = super::terminal_history::load_history(&self.data_dir, &agent.name);
                    self.terminal_histories.insert(agent.name.clone(), hist);
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
#[cfg(test)]
mod tests {
    use super::build_resumed_session_args;
    use crate::db::session::InteractiveSession;

    #[test]
    fn test_yolo_mode_preservation_in_session_relaunch() {
        let session = InteractiveSession {
            id: "test-session".to_string(),
            name: "test-session".to_string(),
            cli: "opencode".to_string(),
            working_dir: "/tmp".to_string(),
            args: Some("--tui --yolo".to_string()),
            started_at: "2023-01-01T00:00:00Z".to_string(),
            status: "active".to_string(),
        };

        assert!(build_resumed_session_args(&session, None, Some("--yolo"))
            .as_deref()
            .is_some_and(|args| args.contains("--yolo")));
    }

    #[test]
    fn test_yolo_flag_not_duplicated_when_falling_back_to_original_args() {
        let session = InteractiveSession {
            id: "test-session".to_string(),
            name: "test-session".to_string(),
            cli: "opencode".to_string(),
            working_dir: "/tmp".to_string(),
            args: Some("--tui --yolo".to_string()),
            started_at: "2023-01-01T00:00:00Z".to_string(),
            status: "active".to_string(),
        };

        let args = build_resumed_session_args(&session, None, Some("--yolo")).unwrap();
        assert_eq!(args.matches("--yolo").count(), 1);
    }

    #[test]
    fn test_original_args_preserved_over_config_args() {
        let session = InteractiveSession {
            id: "test-session".to_string(),
            name: "test-session".to_string(),
            cli: "opencode".to_string(),
            working_dir: "/tmp".to_string(),
            args: Some("--tui --yolo".to_string()),
            started_at: "2023-01-01T00:00:00Z".to_string(),
            status: "active".to_string(),
        };

        // Even if config has different interactive_args, original persisted args win.
        let args = build_resumed_session_args(&session, Some("--chat"), Some("--yolo")).unwrap();
        assert!(args.contains("--tui"));
        assert!(args.contains("--yolo"));
        assert!(!args.contains("--chat"));
    }

    #[test]
    fn test_falls_back_to_config_interactive_args_when_no_original() {
        let session = InteractiveSession {
            id: "test-session".to_string(),
            name: "test-session".to_string(),
            cli: "kiro".to_string(),
            working_dir: "/tmp".to_string(),
            args: None,
            started_at: "2023-01-01T00:00:00Z".to_string(),
            status: "active".to_string(),
        };

        // When no original args are persisted, fall back to config interactive_args.
        // Yolo is not added because we don't know if the original session had it.
        let args = build_resumed_session_args(&session, Some("--tui"), Some("--yolo")).unwrap();
        assert!(args.contains("--tui"));
        assert!(!args.contains("--yolo"));
    }
}
