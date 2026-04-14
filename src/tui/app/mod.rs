//! Application state for the TUI.
//!
//! Holds cached data from the database, selection state, log content,
//! and interactive agent processes.

mod agents;
mod data;
mod dialog;

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::ports::{TaskRepository, WatcherRepository};
use crate::db::Database;
use crate::domain::models::{RunLog, Task, Watcher};

use super::agent::InteractiveAgent;

pub use dialog::NewAgentDialog;
pub use dialog::{NewTaskMode, NewTaskType};
pub(crate) use data::send_mcp_task_run;

// ── Types ───────────────────────────────────────────────────────

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
    Home,
    Preview,
    NewAgentDialog,
    Agent,
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
            AgentEntry::Task(t) => self.db.update_task_enabled(&t.id, !t.enabled)?,
            AgentEntry::Watcher(w) => self.db.update_watcher_enabled(&w.id, !w.enabled)?,
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

        // Check if user scrolled recently
        if self.last_scroll_at.elapsed() < Duration::from_secs(5) {
            self.whimsg.set_ambient(WhimContext::Scrolling);
            return;
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
