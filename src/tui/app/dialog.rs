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
    pub task_type: NewTaskType,
    pub task_mode: NewTaskMode,
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
}

impl NewAgentDialog {
    pub fn new() -> Self {
        let (available, configs) = Self::load_available_clis();
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let catalog = models_db::load_catalog();
        let mut dialog = Self {
            task_type: NewTaskType::Interactive,
            task_mode: NewTaskMode::Interactive,
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
            NewTaskMode::Resume => config.resume_args.clone(),
            NewTaskMode::Interactive => config.interactive_args.clone(),
        }
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
    }

    pub fn prev_cli(&mut self) {
        self.cli_index = self
            .cli_index
            .checked_sub(1)
            .unwrap_or(self.available_clis.len() - 1);
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

        if self.current_path != "/" {
            result.insert(0, "..".to_string());
        }

        self.dir_entries = result;
        self.dir_selected = 0;
        self.dir_scroll = 0;
    }

    pub fn navigate_to_selected(&mut self) {
        if self.dir_selected >= self.dir_entries.len() {
            return;
        }

        let selected = self.dir_entries[self.dir_selected].clone();

        // ".." — go up one level
        if selected == ".." {
            let new_path = if let Some(pos) = self.current_path.rfind('/') {
                if pos == 0 {
                    "/".to_string()
                } else {
                    self.current_path[..pos].to_string()
                }
            } else {
                ".".to_string()
            };
            self.current_path = new_path;
            self.working_dir = self.current_path.clone();
            if self.task_type == NewTaskType::Watcher {
                self.watch_path = self.current_path.clone();
            }
            self.refresh_dir_entries();
            return;
        }

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

use super::App;
use crate::application::ports::{BackgroundAgentRepository, WatcherRepository};
use anyhow::Result;

impl App {
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

        let prev_focus = dialog.prev_focus;
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

    fn launch_interactive(&mut self, dialog: &NewAgentDialog) -> Result<()> {
        use super::super::agent::InteractiveAgent;
        let cli = dialog.selected_cli();
        let dir = dialog.working_dir.clone();
        let args = dialog.selected_args();
        let fallback = dialog.selected_fallback_args();
        let accent = dialog.selected_accent_color();
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
            accent,
        )?;
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
