//! Application state for the TUI.
//!
//! Holds cached data from the database, selection state, log content,
//! and interactive agent processes.

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::application::ports::{RunRepository, StateRepository, TaskRepository, WatcherRepository};
use crate::db::Database;
use crate::domain::models::{Cli, RunLog, Task, Watcher};

use super::agent::InteractiveAgent;

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
    Sidebar,
    LogPanel,
    NewAgentDialog,
    /// Focused on an interactive agent — keys go to PTY.
    Agent,
}

/// State for the "new agent" dialog.
pub struct NewAgentDialog {
    pub cli_index: usize,
    pub available_clis: Vec<Cli>,
    pub working_dir: String,
    /// Which field is focused: 0 = CLI, 1 = working dir.
    pub field: usize,
}

impl NewAgentDialog {
    pub fn new() -> Self {
        let available = Cli::detect_available();
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        Self {
            cli_index: 0,
            available_clis: if available.is_empty() {
                vec![Cli::OpenCode, Cli::Kiro]
            } else {
                available
            },
            working_dir: cwd,
            field: 0,
        }
    }

    pub fn selected_cli(&self) -> Cli {
        self.available_clis[self.cli_index]
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

    /// Tab-complete the working_dir path.
    pub fn complete_path(&mut self) {
        let input = &self.working_dir;
        let (dir, prefix) = if let Some(pos) = input.rfind('/') {
            (&input[..=pos], &input[pos + 1..])
        } else {
            return;
        };

        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };

        let mut matches: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with(prefix) {
                    Some(format!("{dir}{name}/"))
                } else {
                    None
                }
            })
            .collect();

        matches.sort();
        if let Some(first) = matches.first() {
            self.working_dir = first.clone();
        }
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
            focus: Focus::Sidebar,
            log_content: String::new(),
            log_scroll: 0,
            running: true,
            new_agent_dialog: None,
            last_esc: std::time::Instant::now() - std::time::Duration::from_secs(10),
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
        self.refresh_log();
        Ok(())
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

        // Clamp selection
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
    }

    /// Load the log/output for the currently selected agent.
    fn refresh_log(&mut self) {
        let Some(agent) = self.agents.get(self.selected) else {
            self.log_content = String::from("No agent selected");
            return;
        };

        match agent {
            AgentEntry::Interactive(idx) => {
                let output = self.interactive_agents[*idx].output();
                self.log_content = if output.is_empty() {
                    format!("Agent '{}' — waiting for output...", self.interactive_agents[*idx].id)
                } else {
                    output
                };
            }
            _ => {
                let id = agent.id(self).to_string();
                let log_path = self.data_dir.join("logs").join(format!("{id}.log"));
                self.log_content = match std::fs::read_to_string(&log_path) {
                    Ok(content) => tail_lines(&content, 200),
                    Err(_) => format!("No logs yet for '{id}'"),
                };
            }
        }
    }

    // ── Navigation ───────────────────────────────────────────────

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

    // ── Actions ──────────────────────────────────────────────────

    pub fn toggle_enable(&self) -> Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Task(t) => self.db.update_task_enabled(&t.id, !t.enabled)?,
            AgentEntry::Watcher(w) => self.db.update_watcher_enabled(&w.id, !w.enabled)?,
            AgentEntry::Interactive(_) => {} // no-op for interactive
        }
        Ok(())
    }

    pub fn rerun_selected(&self) -> Result<()> {
        let Some(agent) = self.agents.get(self.selected) else {
            return Ok(());
        };
        match agent {
            AgentEntry::Interactive(_) => Ok(()), // can't rerun interactive
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
        self.new_agent_dialog = Some(NewAgentDialog::new());
        self.focus = Focus::NewAgentDialog;
    }

    pub fn close_new_agent_dialog(&mut self) {
        self.new_agent_dialog = None;
        self.focus = Focus::Sidebar;
    }

    pub fn launch_new_agent(&mut self) -> Result<()> {
        let Some(dialog) = &self.new_agent_dialog else {
            return Ok(());
        };
        let cli = dialog.selected_cli();
        let dir = dialog.working_dir.clone();

        // Use approximate panel size (total width - sidebar 26, total height - 3)
        let (tw, th) = ratatui::crossterm::terminal::size().unwrap_or((120, 40));
        let cols = tw.saturating_sub(28); // sidebar + borders
        let rows = th.saturating_sub(4);  // header + footer + borders

        let agent = InteractiveAgent::spawn(cli, &dir, cols, rows)?;
        self.interactive_agents.push(agent);

        self.close_new_agent_dialog();
        Ok(())
    }

    pub fn kill_selected_agent(&mut self) {
        let Some(AgentEntry::Interactive(idx)) = self.agents.get(self.selected) else {
            return;
        };
        let idx = *idx;
        self.interactive_agents[idx].kill();
    }

    /// Clean up: kill all interactive agents on exit.
    pub fn cleanup(&mut self) {
        for agent in &mut self.interactive_agents {
            agent.kill();
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────

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
    let mut stream =
        TcpStream::connect_timeout(&addr.parse()?, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);
    Ok(())
}
