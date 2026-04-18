//! `NewAgentDialog` — state and logic for the "new agent" creation overlay.

use ratatui::style::Color;

use crate::domain::models::Cli;
use crate::domain::models_db::{self, ModelCatalog, ModelEntry};

use super::Focus;
use std::collections::HashMap;
use std::path::PathBuf;

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
    /// Whether to launch the agent in yolo (autonomous) mode.
    pub yolo_mode: bool,
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
            yolo_mode: false,
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

        let inter = config
            .interactive_args
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());

        match self.task_mode {
            NewTaskMode::Resume => {
                // If the user picked a specific session via the canopy session picker,
                // use interactive_args + session_resume_cmd + id.
                if let Some((ref id, _)) = self.selected_session {
                    if let Some(ref cmd) = config.session_resume_cmd {
                        return Some(match inter {
                            Some(ref i) => format!("{i} {cmd} {id}"),
                            None => format!("{cmd} {id}"),
                        });
                    }
                }
                // Build: interactive_args + resume_args (each optional).
                match (inter, config.resume_args.clone()) {
                    (Some(i), Some(r)) => Some(format!("{i} {r}")),
                    (Some(i), None) => Some(i),
                    (None, Some(r)) => Some(r),
                    (None, None) => None,
                }
            }
            NewTaskMode::Interactive => inter,
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

    /// Returns the yolo flag for the currently selected CLI, if any.
    pub fn selected_yolo_flag(&self) -> Option<String> {
        self.cli_configs
            .get(self.cli_index)
            .and_then(|c| c.as_ref())
            .and_then(|c| c.yolo_flag.clone())
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

    /// Open prompt template dialog with the specified template and optional initial content
    pub fn open_simple_prompt_dialog(&mut self, initial_content: Option<HashMap<String, String>>) {
        let prev_focus = self.focus;
        let mut dialog = SimplePromptDialog::new();
        if let Some(content) = initial_content {
            for (section_name, section_content) in content {
                if section_name == "instruction" {
                    let char_len = section_content.chars().count();
                    dialog
                        .sections
                        .insert("instruction".to_string(), section_content);
                    dialog
                        .section_cursors
                        .insert("instruction".to_string(), char_len);
                } else {
                    dialog.add_section_with_content(&section_name.clone(), section_content);
                }
            }
            dialog.focused_section = 0;
        }
        dialog.prev_focus = Some(prev_focus);
        self.simple_prompt_dialog = Some(dialog);
        self.focus = Focus::PromptTemplateDialog;
    }

    /// Close simple prompt dialog
    pub fn close_simple_prompt_dialog(&mut self) {
        if let Some(dialog) = &self.simple_prompt_dialog {
            if let Some(prev) = dialog.prev_focus {
                self.focus = prev;
            } else {
                self.focus = Focus::Agent;
            }
        } else {
            self.focus = Focus::Agent;
        }
        self.simple_prompt_dialog = None;
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
        // Append yolo flag to args when yolo mode is enabled
        let base_args = dialog.selected_args();
        let args = if dialog.yolo_mode {
            if let Some(ref flag) = dialog.selected_yolo_flag() {
                Some(match base_args {
                    Some(ref a) => format!("{a} {flag}"),
                    None => flag.clone(),
                })
            } else {
                base_args
            }
        } else {
            base_args
        };
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
        // Only consider active agent names for collision avoidance
        // This allows names to be reused when agents are closed
        let existing_refs: Vec<&str> = self
            .interactive_agents
            .iter()
            .map(|a| a.name.as_str())
            .collect();
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
            &agent.name,
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

/// Picker state for adding/removing sections
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SectionPickerMode {
    #[default]
    None,
    AddSection {
        selected: usize,
    },
    RemoveSection {
        selected: usize,
    },
    AddCustom {
        input: String,
    },
}

/// Directories ignored when walking for `@` file completion.
const AT_IGNORE_DIRS: &[&str] = &[
    ".git", ".svn", "target", "node_modules", ".idea", ".vscode",
    "build", "dist", "out", "bin", "obj", "__pycache__",
    ".pytest_cache", ".mypy_cache", ".tox", "venv", "env", ".venv",
];

/// A single entry shown in the `@`-file picker dropdown.
pub struct AtEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

/// Inline `@`-file picker state for `SimplePromptDialog`.
pub struct AtPicker {
    /// Root workdir — used for computing relative paths.
    pub workdir: PathBuf,
    /// Currently browsed directory (starts at `workdir`).
    pub current_dir: PathBuf,
    /// Filtered + sorted entries (dirs before files).
    pub entries: Vec<AtEntry>,
    /// Selected index into `entries`.
    pub selected: usize,
    /// Text typed after `@` — used for filtering.
    pub query: String,
    /// Char-index of the `@` character in the section text.
    pub trigger_pos: usize,
}

impl AtPicker {
    pub fn new(workdir: PathBuf, trigger_pos: usize) -> Self {
        let current_dir = workdir.clone();
        let mut p = Self {
            workdir,
            current_dir,
            entries: Vec::new(),
            selected: 0,
            query: String::new(),
            trigger_pos,
        };
        p.refresh();
        p
    }

    /// Rebuild `entries` from `current_dir` filtered by `query`.
    pub fn refresh(&mut self) {
        let q = self.query.to_lowercase();
        let mut dirs: Vec<AtEntry> = Vec::new();
        let mut files: Vec<AtEntry> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.current_dir) {
            for entry in rd.flatten() {
                let path = entry.path();
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                if !q.is_empty() && !name.to_lowercase().contains(&q) {
                    continue;
                }
                if path.is_dir() {
                    if AT_IGNORE_DIRS.contains(&name.as_str()) {
                        continue;
                    }
                    dirs.push(AtEntry { name, path, is_dir: true });
                } else {
                    files.push(AtEntry { name, path, is_dir: false });
                }
            }
        }
        dirs.sort_by(|a, b| a.name.cmp(&b.name));
        files.sort_by(|a, b| a.name.cmp(&b.name));
        dirs.extend(files);
        self.entries = dirs;
        self.selected = 0;
    }

    /// Navigate into the currently selected directory.
    pub fn enter_dir(&mut self) {
        if let Some(e) = self.entries.get(self.selected) {
            if e.is_dir {
                self.current_dir = e.path.clone();
                self.query.clear();
                self.refresh();
            }
        }
    }

    /// Navigate one level up — no upper limit, allows going above `workdir`.
    pub fn go_up(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.query.clear();
            self.refresh();
        }
    }

    /// Path of the selected entry: relative to workdir when inside it, absolute otherwise.
    pub fn relative_path_of_selected(&self) -> Option<String> {
        let e = self.entries.get(self.selected)?;
        if let Ok(rel) = e.path.strip_prefix(&self.workdir) {
            Some(rel.to_string_lossy().replace('\\', "/"))
        } else {
            // Outside workdir — use absolute path so the reference is unambiguous.
            Some(e.path.to_string_lossy().replace('\\', "/"))
        }
    }

    /// Absolute/full path of the selected entry.
    pub fn full_path_of_selected(&self) -> Option<PathBuf> {
        self.entries.get(self.selected).map(|e| e.path.clone())
    }

    /// Display title: `@` + current dir (relative inside workdir, absolute outside) + `/` + query.
    pub fn title(&self) -> String {
        let dir_label = if let Ok(rel) = self.current_dir.strip_prefix(&self.workdir) {
            if rel.as_os_str().is_empty() {
                String::new()
            } else {
                format!("{}/", rel.to_string_lossy())
            }
        } else {
            format!("{}/", self.current_dir.to_string_lossy())
        };
        format!("@{}{}", dir_label, self.query)
    }
}

/// New simplified prompt template dialog with dynamic sections
/// Now supports multiple instances of the same section type
pub struct SimplePromptDialog {
    /// Map of unique section IDs to their content
    pub sections: HashMap<String, String>,
    /// Ordered list of section IDs currently enabled
    pub enabled_sections: Vec<String>,
    /// Which section field is currently focused
    pub focused_section: usize,
    /// Previous focus before opening the dialog
    pub prev_focus: Option<Focus>,
    /// State for the section picker modal
    pub picker_mode: SectionPickerMode,
    /// Counter for generating unique IDs per section type
    pub section_counters: HashMap<String, usize>,
    /// Per-section cursor positions (char index)
    pub section_cursors: HashMap<String, usize>,
    /// Per-section scroll offsets (visual line)
    pub section_scrolls: HashMap<String, usize>,
    /// Active `@`-file picker (inline dropdown), if open.
    pub at_picker: Option<AtPicker>,
}

impl SimplePromptDialog {
    pub fn new() -> Self {
        let mut counters = HashMap::new();
        counters.insert("instruction".to_string(), 1usize);
        let mut cursors = HashMap::new();
        cursors.insert("instruction".to_string(), 0usize);
        let mut scrolls = HashMap::new();
        scrolls.insert("instruction".to_string(), 0usize);
        let mut dialog = Self {
            sections: HashMap::new(),
            enabled_sections: vec!["instruction".to_string()],
            focused_section: 0,
            prev_focus: None,
            picker_mode: SectionPickerMode::None,
            section_counters: counters,
            section_cursors: cursors,
            section_scrolls: scrolls,
            at_picker: None,
        };
        dialog
            .sections
            .insert("instruction".to_string(), String::new());
        dialog
    }

    /// Get cursor position for a section
    pub fn cursor(&self, section: &str) -> usize {
        self.section_cursors.get(section).copied().unwrap_or(0)
    }

    /// Get scroll offset for a section
    pub fn scroll(&self, section: &str) -> usize {
        self.section_scrolls.get(section).copied().unwrap_or(0)
    }

    /// Generate unique ID for a section instance
    fn generate_section_id(&mut self, section_name: &str) -> String {
        let counter = self
            .section_counters
            .entry(section_name.to_string())
            .or_insert(0);
        let id = if *counter == 0 {
            section_name.to_string()
        } else {
            format!("{}_{}", section_name, counter)
        };
        *counter += 1;
        id
    }

    /// Add a section instance (can be same type multiple times)
    pub fn add_section(&mut self, section_name: &str) {
        let unique_id = self.generate_section_id(section_name);
        self.enabled_sections.push(unique_id.clone());
        self.sections.insert(unique_id.clone(), String::new());
        self.section_cursors.insert(unique_id.clone(), 0);
        self.section_scrolls.insert(unique_id, 0);
        self.focused_section = self.enabled_sections.len() - 1;
    }

    /// Add a section with pre-existing content (used for context transfer and initial content)
    pub fn add_section_with_content(&mut self, section_name: &str, content: String) {
        let unique_id = self.generate_section_id(section_name);
        let cursor_pos = content.chars().count();
        self.enabled_sections.push(unique_id.clone());
        self.sections.insert(unique_id.clone(), content);
        self.section_cursors.insert(unique_id.clone(), cursor_pos);
        self.section_scrolls.insert(unique_id, 0);
        self.focused_section = self.enabled_sections.len() - 1;
    }

    /// Remove a specific section instance
    pub fn remove_section(&mut self, section_id: &str) {
        if section_id != "instruction" {
            self.enabled_sections.retain(|s| s != section_id);
            self.sections.remove(section_id);
            self.section_cursors.remove(section_id);
            self.section_scrolls.remove(section_id);
            if self.focused_section > 0 {
                self.focused_section = self.focused_section.saturating_sub(1);
            }
        }
    }

    /// Get available section types (these can always be added again)
    pub fn get_available_sections() -> Vec<(&'static str, &'static str)> {
        vec![
            ("instruction", "Instruction"),
            ("context", "Context"),
            ("resources", "Resources"),
            ("examples", "Examples"),
            ("constraints", "Constraints"),
            ("output_format", "Output Format"),
        ]
    }

    /// Get section types available to add (can always add more instances)
    pub fn get_addable_sections(&self) -> Vec<(&'static str, &'static str)> {
        Self::get_available_sections()
    }

    /// Get section instances available to remove (not instruction)
    pub fn get_removable_sections(&self) -> Vec<(String, String)> {
        self.enabled_sections
            .iter()
            .filter(|s| *s != "instruction")
            .map(|section_id| {
                // Extract section name from ID (e.g., "context_1" -> "context")
                let section_name = section_id.split('_').next().unwrap_or(section_id.as_str());
                let label = Self::get_available_sections()
                    .into_iter()
                    .find(|(name, _)| *name == section_name)
                    .map(|(_, label)| label)
                    .unwrap_or(section_name);

                // Build display label with instance number
                let display = if section_id.contains('_') {
                    format!("{} {}", label, section_id.rsplit('_').next().unwrap_or(""))
                } else {
                    label.to_string()
                };
                (section_id.clone(), display)
            })
            .collect()
    }

    /// Get the content for a section
    pub fn get_section_content(&self, section_name: &str) -> String {
        self.sections.get(section_name).cloned().unwrap_or_default()
    }

    /// Set the content for a section
    pub fn set_section_content(&mut self, section_name: &str, content: String) {
        self.sections.insert(section_name.to_string(), content);
    }

    /// Build the final prompt from the filled sections with structured format
    /// Supports multiple instances of each section type
    pub fn build_prompt(&self) -> Result<String> {
        let mut result = String::new();

        // Add all context sections (each in its own <problem_context> block)
        for section_id in &self.enabled_sections {
            if section_id.starts_with("context") {
                if let Some(content) = self.sections.get(section_id) {
                    if !content.is_empty() {
                        result.push_str("# [CONTEXT]: Project Background\n");
                        result.push_str("<problem_context>\n");
                        result.push_str(content);
                        result.push_str("\n</problem_context>\n\n");
                    }
                }
            }
        }

        // Add all instruction sections (base "instruction" + any "instruction_N")
        result.push_str("# [INSTRUCTIONS]: Execution Logic\n");
        result.push_str("<instruction_set>\n");
        let mut instr_count = 0;
        for section_id in &self.enabled_sections {
            if section_id == "instruction" || section_id.starts_with("instruction_") {
                if let Some(content) = self.sections.get(section_id) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        instr_count += 1;
                        result.push_str(&format!("  <instruction_{}>\n", instr_count));
                        result.push_str(&format!("    {}\n", trimmed));
                        result.push_str(&format!("  </instruction_{}>\n\n", instr_count));
                    }
                }
            }
        }
        result.push_str("</instruction_set>\n\n");

        // Add all resources sections (can have multiple)
        let mut resources_count = 0;
        for section_id in &self.enabled_sections {
            if section_id.starts_with("resources") {
                if let Some(content) = self.sections.get(section_id) {
                    if !content.is_empty() {
                        if resources_count == 0 {
                            result.push_str("# [RESOURCES]: Knowledge Base & Data\n");
                            result.push_str("<reference_materials>\n");
                        }
                        result.push_str("--- START DATA ---\n");
                        result.push_str(content);
                        result.push_str("\n--- END DATA ---\n");
                        resources_count += 1;
                    }
                }
            }
        }
        if resources_count > 0 {
            result.push_str("</reference_materials>\n\n");
        }

        // Add all examples sections (can have multiple)
        let mut examples_count = 0;
        for section_id in &self.enabled_sections {
            if section_id.starts_with("examples") {
                if let Some(content) = self.sections.get(section_id) {
                    if !content.is_empty() {
                        if examples_count == 0 {
                            result.push_str("# [EXAMPLES]: Multi-Shot Learning\n");
                            result.push_str("<example_gallery>\n");
                        }
                        let lines: Vec<&str> =
                            content.lines().filter(|s| !s.trim().is_empty()).collect();
                        for (i, line) in lines.into_iter().enumerate() {
                            result.push_str(&format!(
                                "  <example_{}>\n",
                                examples_count * 100 + i + 1
                            ));
                            result.push_str(&format!("    {}\n", line.trim()));
                            result.push_str(&format!(
                                "  </example_{}>\n\n",
                                examples_count * 100 + i + 1
                            ));
                        }
                        examples_count += 1;
                    }
                }
            }
        }
        if examples_count > 0 {
            result.push_str("</example_gallery>\n\n");
        }

        // Add constraints sections
        let mut constraints_count = 0;
        for section_id in &self.enabled_sections {
            if section_id == "constraints" || section_id.starts_with("constraints_") {
                if let Some(content) = self.sections.get(section_id) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if constraints_count == 0 {
                            result.push_str("# [CONSTRAINTS]: Behavioral Boundaries\n");
                            result.push_str("<constraints>\n");
                        }
                        constraints_count += 1;
                        result.push_str(&format!("  <constraint_{}>\n", constraints_count));
                        result.push_str(&format!("    {}\n", trimmed));
                        result.push_str(&format!("  </constraint_{}>\n\n", constraints_count));
                    }
                }
            }
        }
        if constraints_count > 0 {
            result.push_str("</constraints>\n\n");
        }

        // Add output format sections
        let mut output_count = 0;
        for section_id in &self.enabled_sections {
            if section_id == "output_format" || section_id.starts_with("output_format_") {
                if let Some(content) = self.sections.get(section_id) {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        if output_count == 0 {
                            result.push_str("# [OUTPUT FORMAT]: Response Contract\n");
                            result.push_str("<output_spec>\n");
                        }
                        output_count += 1;
                        result.push_str(&format!("  <spec_{}>\n", output_count));
                        result.push_str(&format!("    {}\n", trimmed));
                        result.push_str(&format!("  </spec_{}>\n\n", output_count));
                    }
                }
            }
        }
        if output_count > 0 {
            result.push_str("</output_spec>\n\n");
        }

        Ok(result)
    }

    /// Replace the `@`-trigger with `@rel_path` in the section text, and add the full path
    /// to a "resources" section (creating one if needed).
    pub fn insert_at_completion(
        &mut self,
        section_id: &str,
        rel_path: &str,
        full_path: &str,
        field_width: usize,
    ) {
        let Some(trigger_pos) = self.at_picker.as_ref().map(|p| p.trigger_pos) else {
            return;
        };
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        // The `@` is at trigger_pos; cursor is currently at trigger_pos + 1
        // (we never insert query chars into the text, only into picker.query).
        let replacement: String = format!("@{}", rel_path);
        let new_chars: Vec<char> = chars[..trigger_pos]
            .iter()
            .chain(replacement.chars().collect::<Vec<_>>().iter())
            .chain(chars[(trigger_pos + 1)..].iter())
            .cloned()
            .collect();
        let new_cursor = trigger_pos + replacement.chars().count();
        self.set_section_content(section_id, new_chars.into_iter().collect());
        self.section_cursors.insert(section_id.to_string(), new_cursor);
        self.update_section_scroll(section_id, field_width);

        // Preserve focused section — resource insertion must not steal focus.
        let saved_focus = self.focused_section;

        // Add or append to a "resources" section with the full path.
        let existing_resources = self.enabled_sections
            .iter()
            .find(|id| id.starts_with("resources"))
            .cloned();
        if let Some(res_id) = existing_resources {
            let res_content = self.get_section_content(&res_id);
            let new_res_content = if res_content.is_empty() {
                full_path.to_string()
            } else {
                format!("{}\n{}", res_content, full_path)
            };
            self.set_section_content(&res_id, new_res_content);
        } else {
            // Create a new resources section with this full path
            self.add_section_with_content("resources", full_path.to_string());
        }

        // Restore focus to the field the user was editing.
        self.focused_section = saved_focus;
    }

    /// Colorize `@word` tokens in rendered section text with a custom accent color.
    pub fn get_file_reference_with_styling(&self, text: &str, accent: Color) -> Vec<(String, Option<Color>)> {
        let mut result = Vec::new();
        let mut current_pos = 0;

        while let Some(at_pos) = text[current_pos..].find('@') {
            let absolute_pos = current_pos + at_pos;
            if absolute_pos > current_pos {
                result.push((text[current_pos..absolute_pos].to_string(), None));
            }
            let remaining = &text[absolute_pos..];
            let ref_end = remaining
                .find(|c: char| {
                    c.is_whitespace() || c == ',' || c == '!' || c == '?' || c == '│'
                })
                .unwrap_or(remaining.len());
            let file_ref = &remaining[..ref_end];
            if file_ref.len() > 1
                && file_ref[1..].chars().all(|c| {
                    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/'
                })
            {
                result.push((file_ref.to_string(), Some(accent)));
            } else {
                result.push((file_ref.to_string(), None));
            }
            current_pos = absolute_pos + ref_end;
        }
        if current_pos < text.len() {
            result.push((text[current_pos..].to_string(), None));
        }
        result
    }

    /// Count visual (wrapped) lines for a text given a field width
    pub fn visual_line_count(text: &str, field_width: usize) -> usize {
        if field_width == 0 {
            return 1;
        }
        let mut count = 0;
        for line in text.lines() {
            if line.is_empty() {
                count += 1;
            } else {
                count += line.chars().count().div_ceil(field_width);
            }
        }
        count.max(1)
    }

    /// Visual lines occupied by the first `char_idx` chars of text.
    fn visual_lines_to_cursor(text: &str, char_idx: usize, field_width: usize) -> usize {
        let prefix: String = text.chars().take(char_idx).collect();
        Self::visual_line_count(&prefix, field_width).max(1)
    }

    /// Max visible lines for a section type (instruction=5, others=3)
    pub fn max_visible_lines(section_id: &str) -> usize {
        if section_id == "instruction" || section_id.starts_with("instruction_") {
            5
        } else {
            3
        }
    }

    /// Update scroll for a section so the cursor stays visible.
    pub fn update_section_scroll(&mut self, section_id: &str, field_width: usize) {
        let max_vis = Self::max_visible_lines(section_id);
        let text = self
            .sections
            .get(section_id)
            .map(|s| s.as_str())
            .unwrap_or("");
        let cur = self.cursor(section_id);
        let cursor_visual_line =
            Self::visual_lines_to_cursor(text, cur, field_width).saturating_sub(1);

        let scroll = self
            .section_scrolls
            .entry(section_id.to_string())
            .or_insert(0);
        if cursor_visual_line < *scroll {
            *scroll = cursor_visual_line;
        } else if cursor_visual_line >= *scroll + max_vis {
            *scroll = cursor_visual_line + 1 - max_vis;
        }
    }

    /// Move cursor left one char in the given section.
    pub fn move_cursor_left(&mut self, section_id: &str, field_width: usize) {
        let cur = self.cursor(section_id);
        if cur > 0 {
            self.section_cursors.insert(section_id.to_string(), cur - 1);
            self.update_section_scroll(section_id, field_width);
        }
    }

    /// Move cursor right one char in the given section.
    pub fn move_cursor_right(&mut self, section_id: &str, field_width: usize) {
        let len = self
            .sections
            .get(section_id)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let cur = self.cursor(section_id);
        if cur < len {
            self.section_cursors.insert(section_id.to_string(), cur + 1);
            self.update_section_scroll(section_id, field_width);
        }
    }

    /// Move cursor up one visual line in the given section.
    pub fn move_cursor_up(&mut self, section_id: &str, field_width: usize) {
        let cur = self.cursor(section_id);
        self.section_cursors
            .insert(section_id.to_string(), cur.saturating_sub(field_width));
        self.update_section_scroll(section_id, field_width);
    }

    /// Move cursor down one visual line in the given section.
    pub fn move_cursor_down(&mut self, section_id: &str, field_width: usize) {
        let len = self
            .sections
            .get(section_id)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let cur = self.cursor(section_id);
        self.section_cursors
            .insert(section_id.to_string(), (cur + field_width).min(len));
        self.update_section_scroll(section_id, field_width);
    }

    /// Insert a character at cursor position in any section.
    pub fn insert_char_at_cursor(&mut self, section_id: &str, ch: char, field_width: usize) {
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        let cur = self.cursor(section_id).min(chars.len());
        let mut new_chars = chars;
        new_chars.insert(cur, ch);
        let new_content: String = new_chars.into_iter().collect();
        self.set_section_content(section_id, new_content);
        self.section_cursors.insert(section_id.to_string(), cur + 1);
        self.update_section_scroll(section_id, field_width);
    }

    /// Delete the character before cursor in any section.
    pub fn backspace_at_cursor(&mut self, section_id: &str, field_width: usize) {
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        let cur = self.cursor(section_id);
        if cur > 0 && cur <= chars.len() {
            let mut new_chars = chars;
            new_chars.remove(cur - 1);
            let new_content: String = new_chars.into_iter().collect();
            self.set_section_content(section_id, new_content);
            self.section_cursors.insert(section_id.to_string(), cur - 1);
            self.update_section_scroll(section_id, field_width);
        }
    }

    /// Insert a newline at cursor position in any section.
    pub fn insert_newline_at_cursor(&mut self, section_id: &str, field_width: usize) {
        let content = self.get_section_content(section_id);
        let chars: Vec<char> = content.chars().collect();
        let cur = self.cursor(section_id).min(chars.len());
        let before: String = chars[..cur].iter().collect();
        let after: String = chars[cur..].iter().collect();
        let new_content = format!("{}\n{}", before, after);
        self.set_section_content(section_id, new_content);
        self.section_cursors.insert(section_id.to_string(), cur + 1);
        self.update_section_scroll(section_id, field_width);
    }
}
