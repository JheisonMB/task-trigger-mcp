//! `NewAgentDialog` — state and logic for the "new agent" creation overlay.

use ratatui::style::Color;

use crate::domain::models::Cli;

use super::Focus;

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

// ── Dialog methods on App ───────────────────────────────────────

use super::App;
use anyhow::Result;
use crate::application::ports::{TaskRepository, WatcherRepository};

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

        // Restore dialog briefly for close logic
        let prev_focus = dialog.prev_focus;
        // Don't put it back — close_new_agent_dialog expects it but we already took it
        if let Some(prev) = prev_focus {
            self.focus = prev;
        } else {
            self.focus = Focus::Home;
        }
        self.new_agent_dialog = None;

        self.refresh_agents()?;
        self.selected = self.agents.len().saturating_sub(1);

        self.focus = if was_interactive {
            Focus::Agent
        } else {
            Focus::Preview
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
        Ok(())
    }

    fn launch_scheduled(&mut self, dialog: &NewAgentDialog, model: Option<String>) -> Result<()> {
        use chrono::Utc;
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
