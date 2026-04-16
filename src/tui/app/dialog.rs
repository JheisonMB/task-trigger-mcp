//! `NewAgentDialog` — state and logic for the "new agent" creation overlay.

use ratatui::style::Color;

use crate::domain::models::Cli;
use crate::domain::models_db::{self, ModelCatalog, ModelEntry};

use super::Focus;

/// Type of background_agent to create.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NewTaskType {
    Interactive,
    Scheduled,
    Watcher,
}

/// Launch mode for interactive agents.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NewTaskMode {
    /// Start a fresh interactive session.
    Interactive,
    /// Resume a previous session.
    Resume,
}

/// State for the "new agent" dialog.
pub struct NewAgentDialog {
    /// When `Some(id)`, the dialog is in edit mode for an existing agent.
    pub edit_id: Option<String>,
    pub task_type: NewTaskType,
    pub task_mode: NewTaskMode,
    /// Optional user-provided name for interactive agents.
    pub agent_name: String,
    pub cli_index: usize,
    pub available_clis: Vec<Cli>,
    pub cli_configs: Vec<Option<crate::domain::cli_config::CliConfig>>,
    pub working_dir: String,
    pub model: String,
    pub prompt: String,
    pub cron_expr: String,
    pub watch_path: String,
    pub watch_events: Vec<String>,
    /// Which field is focused: 0=type, 1=mode (interactive), 2=CLI, 3=dir, 4=model, 5=prompt, 6=cron/watch
    pub field: usize,
    pub dir_entries: Vec<String>,
    pub dir_selected: usize,
    pub dir_scroll: usize,
    pub current_path: String,
    pub prev_focus: Option<Focus>,
    // ── Model suggestions ──
    pub model_catalog: Option<ModelCatalog>,
    pub model_suggestions: Vec<ModelEntry>,
    pub model_suggestion_idx: usize,
    pub model_picker_open: bool,
    // ── Session picker (canopy-side, for CLIs with session_list_cmd) ──
    pub session_picker_open: bool,
    /// Parsed sessions: (id, display_label)
    pub session_entries: Vec<(String, String)>,
    pub session_picker_idx: usize,
    /// The session the user confirmed, if any.
    pub selected_session: Option<(String, String)>,
}

impl NewAgentDialog {
    pub fn new() -> Self {
        let (available, configs) = Self::load_available_clis();
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let catalog = models_db::load_catalog();
        let mut dialog = Self {
            edit_id: None,
            task_type: NewTaskType::Interactive,
            task_mode: NewTaskMode::Interactive,
            agent_name: String::new(),
            cli_index: 0,
            available_clis: if available.is_empty() {
                vec![Cli::new("opencode"), Cli::new("kiro"), Cli::new("qwen")]
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
            model_catalog: catalog,
            model_suggestions: Vec::new(),
            model_suggestion_idx: 0,
            model_picker_open: false,
            session_picker_open: false,
            session_entries: Vec::new(),
            session_picker_idx: 0,
            selected_session: None,
        };
        dialog.refresh_dir_entries();
        dialog.refresh_model_suggestions();
        dialog
    }

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
        self.available_clis[self.cli_index].clone()
    }

    pub fn selected_args(&self) -> Option<String> {
        let config = self
            .cli_configs
            .get(self.cli_index)
            .and_then(|c| c.as_ref())?;
        match self.task_mode {
            NewTaskMode::Resume => {
                // If the user picked a specific session via the canopy session picker,
                // use session_resume_cmd + id (e.g. `--session ses_abc123`).
                if let Some((ref id, _)) = self.selected_session {
                    if let Some(ref cmd) = config.session_resume_cmd {
                        return Some(format!("{cmd} {id}"));
                    }
                }
                // Otherwise fall back to resume_args (e.g. --continue) or interactive_args.
                config
                    .resume_args
                    .clone()
                    .or_else(|| config.interactive_args.clone())
            }
            NewTaskMode::Interactive => config.interactive_args.clone(),
        }
    }

    /// Returns true when the current CLI has no dedicated resume_args configured.
    pub fn is_edit_mode(&self) -> bool {
        self.edit_id.is_some()
    }

    pub fn resume_unconfigured(&self) -> bool {
        matches!(self.task_mode, NewTaskMode::Resume)
            && self
                .cli_configs
                .get(self.cli_index)
                .and_then(|c| c.as_ref())
                .map(|c| c.resume_args.is_none())
                .unwrap_or(true)
    }

    /// Returns true when the current CLI supports canopy-side session picking.
    pub fn has_session_picker(&self) -> bool {
        matches!(self.task_mode, NewTaskMode::Resume)
            && self
                .cli_configs
                .get(self.cli_index)
                .and_then(|c| c.as_ref())
                .map(|c| c.session_list_cmd.is_some())
                .unwrap_or(false)
    }

    /// Run the CLI's session_list_cmd, parse the output and populate session_entries.
    pub fn load_sessions(&mut self) {
        let Some(config) = self
            .cli_configs
            .get(self.cli_index)
            .and_then(|c| c.as_ref())
        else {
            return;
        };
        let Some(ref list_cmd) = config.session_list_cmd.clone() else {
            return;
        };
        let binary = config.binary.clone();

        let args: Vec<&str> = list_cmd.split_whitespace().collect();
        let Ok(output) = std::process::Command::new(&binary).args(&args).output() else {
            return;
        };

        let text = String::from_utf8_lossy(&output.stdout);
        self.session_entries = parse_session_list(&text);
        self.session_picker_idx = 0;
    }

    /// Open the session picker: load sessions and set picker_open = true.
    pub fn open_session_picker(&mut self) {
        self.load_sessions();
        if !self.session_entries.is_empty() {
            self.session_picker_open = true;
        }
    }

    /// Confirm the currently highlighted session.
    pub fn confirm_session_pick(&mut self) {
        if let Some(entry) = self.session_entries.get(self.session_picker_idx) {
            self.selected_session = Some(entry.clone());
        }
        self.session_picker_open = false;
    }

    /// Clear the selected session (fall back to --continue / resume_args).
    pub fn clear_selected_session(&mut self) {
        self.selected_session = None;
    }

    pub fn selected_fallback_args(&self) -> Option<String> {
        self.cli_configs
            .get(self.cli_index)
            .and_then(|c| c.as_ref())
            .and_then(|c| c.fallback_interactive_args.clone())
    }

    pub fn selected_accent_color(&self) -> Color {
        self.cli_configs
            .get(self.cli_index)
            .and_then(|c| c.as_ref())
            .and_then(|c| c.accent_color)
            .map(|[r, g, b]| Color::Rgb(r, g, b))
            .unwrap_or(Color::Rgb(102, 187, 106))
    }

    pub fn next_cli(&mut self) {
        self.cli_index = (self.cli_index + 1) % self.available_clis.len();
        self.selected_session = None;
    }

    pub fn prev_cli(&mut self) {
        self.cli_index = self
            .cli_index
            .checked_sub(1)
            .unwrap_or(self.available_clis.len() - 1);
        self.selected_session = None;
    }

    pub fn refresh_dir_entries(&mut self) {
        let Ok(entries) = std::fs::read_dir(&self.current_path) else {
            self.dir_entries.clear();
            return;
        };

        let include_files = self.task_type == NewTaskType::Watcher;

        self.dir_entries.clear();
        let all: Vec<_> = entries.filter_map(|e| e.ok()).collect();

        // Collect dirs (always) and files (watcher only), skip hidden entries
        let mut dirs: Vec<String> = all
            .iter()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    None
                } else {
                    Some(format!("📁 {name}"))
                }
            })
            .collect();

        let mut files: Vec<String> = if include_files {
            all.iter()
                .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with('.') {
                        None
                    } else {
                        Some(format!("  {name}"))
                    }
                })
                .collect()
        } else {
            Vec::new()
        };

        dirs.sort();
        files.sort();

        let mut result = dirs;
        result.extend(files);

        self.dir_entries = result;
        self.dir_selected = 0;
        self.dir_scroll = 0;
    }

    /// Go up one directory level (← key).
    pub fn go_up(&mut self) {
        if self.current_path == "/" {
            return;
        }
        let new_path = if let Some(pos) = self.current_path.rfind('/') {
            if pos == 0 {
                "/".to_string()
            } else {
                self.current_path[..pos].to_string()
            }
        } else {
            return;
        };
        self.current_path = new_path;
        self.working_dir = self.current_path.clone();
        if self.task_type == NewTaskType::Watcher {
            self.watch_path = self.current_path.clone();
        }
        self.refresh_dir_entries();
    }

    /// Enter the selected directory entry (→ key or Space).
    pub fn navigate_to_selected(&mut self) {
        if self.dir_selected >= self.dir_entries.len() {
            return;
        }

        let selected = self.dir_entries[self.dir_selected].clone();

        // Strip prefix icons to get actual name
        let name = selected.trim_start_matches("📁 ").trim_start_matches("  ");

        let full_path = format!("{}/{}", self.current_path.trim_end_matches('/'), name);
        let is_dir = std::fs::metadata(&full_path)
            .map(|m| m.is_dir())
            .unwrap_or(false);

        if is_dir {
            // Navigate into directory
            self.current_path = full_path;
            self.working_dir = self.current_path.clone();
            if self.task_type == NewTaskType::Watcher {
                self.watch_path = self.current_path.clone();
            }
            self.refresh_dir_entries();
        } else {
            // File selected (Watcher only) — set watch_path, stay in current dir
            self.watch_path = full_path;
        }
    }

    /// Recompute the filtered model suggestions based on current CLI and query.
    pub fn refresh_model_suggestions(&mut self) {
        let Some(catalog) = &self.model_catalog else {
            self.model_suggestions.clear();
            return;
        };
        let binding = self.selected_cli();
        let cli_name = binding.as_str();
        let cli_models = models_db::models_for_cli(catalog, cli_name);
        self.model_suggestions = models_db::filter_models(&cli_models, &self.model);
        // Clamp selection index
        if self.model_suggestion_idx >= self.model_suggestions.len() {
            self.model_suggestion_idx = 0;
        }
    }

    /// Accept the currently highlighted model suggestion.
    pub fn accept_model_suggestion(&mut self) {
        if let Some(entry) = self.model_suggestions.get(self.model_suggestion_idx) {
            self.model = entry.id.clone();
            self.model_picker_open = false;
        }
    }
}

// ── Dialog methods on App ───────────────────────────────────────

use super::AgentEntry;
use super::App;
use crate::application::ports::{
    BackgroundAgentRepository, WatcherFieldsUpdate, WatcherRepository,
};
use anyhow::Result;

impl App {
    pub fn open_edit_dialog(&mut self) {
        let prev_focus = self.focus;
        let Some(agent) = self.agents.get(self.selected) else {
            return;
        };
        let mut dialog = NewAgentDialog::new();
        dialog.prev_focus = Some(prev_focus);

        match agent {
            AgentEntry::BackgroundAgent(t) => {
                dialog.edit_id = Some(t.id.clone());
                dialog.task_type = NewTaskType::Scheduled;
                dialog.prompt = t.prompt.clone();
                dialog.cron_expr = t.schedule_expr.clone();
                dialog.working_dir = t.working_dir.clone().unwrap_or_default();
                dialog.model = t.model.clone().unwrap_or_default();
                if let Some(idx) = dialog
                    .available_clis
                    .iter()
                    .position(|c| c.as_str() == t.cli.as_str())
                {
                    dialog.cli_index = idx;
                }
                // Start on first editable field (prompt = field 2 in edit mode)
                dialog.field = 2;
            }
            AgentEntry::Watcher(w) => {
                dialog.edit_id = Some(w.id.clone());
                dialog.task_type = NewTaskType::Watcher;
                dialog.prompt = w.prompt.clone();
                dialog.watch_path = w.path.clone();
                dialog.watch_events = w
                    .events
                    .iter()
                    .map(|e| e.to_string().to_lowercase())
                    .collect();
                dialog.model = w.model.clone().unwrap_or_default();
                if let Some(idx) = dialog
                    .available_clis
                    .iter()
                    .position(|c| c.as_str() == w.cli.as_str())
                {
                    dialog.cli_index = idx;
                }
                dialog.field = 2;
            }
            AgentEntry::Interactive(_) => return, // editing interactive agents not supported
        }

        dialog.refresh_model_suggestions();
        self.new_agent_dialog = Some(dialog);
        self.focus = Focus::NewAgentDialog;
    }

    pub fn open_new_agent_dialog(&mut self) {
        let prev_focus = self.focus;
        self.new_agent_dialog = Some(NewAgentDialog::new());
        self.new_agent_dialog.as_mut().unwrap().prev_focus = Some(prev_focus);
        self.focus = Focus::NewAgentDialog;
    }

    pub fn close_new_agent_dialog(&mut self) {
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
        // Take dialog out of self to avoid borrow conflicts
        let Some(dialog) = self.new_agent_dialog.take() else {
            return Ok(());
        };

        let model = if dialog.model.is_empty() {
            None
        } else {
            Some(dialog.model.clone())
        };

        let was_interactive = matches!(dialog.task_type, NewTaskType::Interactive);
        let prev_focus = dialog.prev_focus;

        if let Some(ref edit_id) = dialog.edit_id {
            // ── Edit mode: partial-update existing agent ──────────────────
            let model_ref = model.as_deref();
            match dialog.task_type {
                NewTaskType::Scheduled => {
                    self.update_scheduled(&dialog, model_ref, edit_id)?;
                }
                NewTaskType::Watcher => {
                    self.update_watcher_edit(&dialog, model_ref, edit_id)?;
                }
                NewTaskType::Interactive => {}
            }
            self.new_agent_dialog = None;
            self.refresh_agents()?;
            self.focus = prev_focus.unwrap_or(Focus::Preview);
            return Ok(());
        }

        // ── Create mode ───────────────────────────────────────────────────
        match dialog.task_type {
            NewTaskType::Interactive => {
                self.launch_interactive(&dialog)?;
            }
            NewTaskType::Scheduled => {
                self.launch_scheduled(&dialog, model)?;
            }
            NewTaskType::Watcher => {
                self.launch_watcher(&dialog, model)?;
            }
        }

        self.new_agent_dialog = None;

        self.refresh_agents()?;
        self.selected = self.agents.len().saturating_sub(1);

        // Interactive background_agents go to full agent focus; background background_agents restore
        // to whatever focus was active before the dialog opened.
        self.focus = if was_interactive {
            Focus::Agent
        } else {
            prev_focus.unwrap_or(Focus::Home)
        };
        Ok(())
    }

    fn update_scheduled(
        &self,
        dialog: &NewAgentDialog,
        model: Option<&str>,
        id: &str,
    ) -> Result<()> {
        use crate::application::ports::BackgroundAgentFieldsUpdate;
        if dialog.prompt.is_empty() {
            return Ok(());
        }
        let dir = if dialog.working_dir.is_empty() {
            None
        } else {
            Some(dialog.working_dir.as_str())
        };
        let cli = dialog.selected_cli();
        let fields = BackgroundAgentFieldsUpdate {
            prompt: Some(&dialog.prompt),
            schedule_expr: Some(&dialog.cron_expr),
            cli: Some(cli.as_str()),
            model: Some(model),
            working_dir: Some(dir),
            expires_at: None,
        };
        self.db.update_background_agent_fields(id, &fields)?;
        Ok(())
    }

    fn update_watcher_edit(
        &self,
        dialog: &NewAgentDialog,
        model: Option<&str>,
        id: &str,
    ) -> Result<()> {
        if dialog.prompt.is_empty() || dialog.watch_path.is_empty() {
            return Ok(());
        }
        let events_str = dialog.watch_events.join(",");
        let cli = dialog.selected_cli();
        let fields = WatcherFieldsUpdate {
            prompt: Some(&dialog.prompt),
            path: Some(&dialog.watch_path),
            events: Some(&events_str),
            cli: Some(cli.as_str()),
            model: Some(model),
            debounce_seconds: None,
            recursive: None,
        };
        self.db.update_watcher_fields(id, &fields)?;
        Ok(())
    }

    fn launch_interactive(&mut self, dialog: &NewAgentDialog) -> Result<()> {
        use super::super::agent::InteractiveAgent;
        let cli = dialog.selected_cli();
        let dir = dialog.working_dir.clone();
        let args = dialog.selected_args();
        let fallback = dialog.selected_fallback_args();
        let accent = dialog.selected_accent_color();
        let name = if dialog.agent_name.trim().is_empty() {
            None
        } else {
            Some(dialog.agent_name.trim().to_string())
        };
        let model = if dialog.model.is_empty() {
            None
        } else {
            Some(dialog.model.clone())
        };
        let model_flag = dialog
            .cli_configs
            .get(dialog.cli_index)
            .and_then(|c| c.as_ref())
            .and_then(|c| c.model_flag.clone());
        let (cols, rows) = if self.last_panel_inner != (0, 0) {
            self.last_panel_inner
        } else {
            let (tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
            (tw.saturating_sub(28), th.saturating_sub(4))
        };
        let mut existing_ids: Vec<String> = self.interactive_agents.iter().map(|a| a.id.clone()).collect();
        // Include historical DB session names to avoid collisions
        if let Ok(history) = self.db.get_all_session_names() {
            existing_ids.extend(history);
        }
        let existing_refs: Vec<&str> = existing_ids.iter().map(|s| s.as_str()).collect();
        let agent = InteractiveAgent::spawn(
            cli,
            &dir,
            cols,
            rows,
            args.as_deref(),
            fallback.as_deref(),
            accent,
            name.as_deref(),
            &existing_refs,
            model.as_deref(),
            model_flag.as_deref(),
        )?;
        // Persist session in registry
        let _ = self.db.insert_interactive_session(
            &agent.id,
            &agent.id,
            agent.cli.as_str(),
            &dir,
            args.as_deref(),
        );
        self.interactive_agents.push(agent);
        self.whimsg
            .notify_event(crate::tui::whimsg::WhimContext::AgentSpawned);
        Ok(())
    }

    fn launch_scheduled(&mut self, dialog: &NewAgentDialog, model: Option<String>) -> Result<()> {
        use chrono::Utc;
        if dialog.prompt.is_empty() {
            return Ok(());
        }
        let cli = dialog.selected_cli();
        let id = format!(
            "background_agent-{}",
            &uuid::Uuid::new_v4().to_string()[..8]
        );
        let working_dir = if dialog.working_dir.is_empty() {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "/".to_string())
        } else {
            dialog.working_dir.clone()
        };
        let background_agent = crate::domain::models::BackgroundAgent {
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
        self.db
            .insert_or_update_background_agent(&background_agent)?;
        Ok(())
    }

    fn launch_watcher(&mut self, dialog: &NewAgentDialog, model: Option<String>) -> Result<()> {
        use chrono::Utc;
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
        Ok(())
    }
}

/// Parse the output of a CLI session list command into (id, label) pairs.
/// Handles the opencode `session list` table format:
///   ses_<id>  Title...   Updated
/// Lines that are headers, separators, or do not start with an identifier are skipped.
fn parse_session_list(output: &str) -> Vec<(String, String)> {
    output
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('\u{2500}') // ─ separator
        })
        .filter_map(|line| {
            let mut parts = line.splitn(2, |c: char| c.is_whitespace());
            let id = parts.next()?.trim().to_string();
            // Skip header rows — real IDs contain letters+digits+mixed case
            if id == "Session" || id.len() < 8 {
                return None;
            }
            let label = parts.next().unwrap_or("").trim().to_string();
            Some((id, label))
        })
        .collect()
}
