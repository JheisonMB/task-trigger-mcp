use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::application::notification_service::NotificationService;
use crate::db::Database;
use crate::domain::models::{Agent, RunLog};
use crate::tui::agent::InteractiveAgent;
use crate::tui::app::dialog::{NewAgentDialog, SimplePromptDialog};
use crate::tui::app::terminal_search::TerminalSearch;

/// Unified entry in the sidebar.
#[allow(clippy::large_enum_variant)]
pub enum AgentEntry {
    Agent(Agent),
    Interactive(usize), // index into App::interactive_agents
    Terminal(usize),    // index into App::terminal_agents
    Group(usize),       // index into App::split_groups
}

impl AgentEntry {
    pub fn id<'a>(&'a self, app: &'a App) -> &'a str {
        match self {
            Self::Agent(a) => &a.id,
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

#[derive(Clone, Copy)]
pub(crate) enum ContextTransferSource {
    Interactive(usize),
    Terminal(usize),
}

// ── App struct ──────────────────────────────────────────────────

/// Main application state.
pub struct App {
    pub(crate) db: Arc<Database>,
    pub(crate) data_dir: PathBuf,

    // Data cache (refreshed every tick)
    pub(crate) agents: Vec<AgentEntry>,
    pub(crate) active_runs: HashMap<String, RunLog>,
    pub(crate) recent_runs: Vec<RunLog>,
    pub(crate) interactive_agents: Vec<InteractiveAgent>,
    /// Raw terminal sessions (no AI CLI).
    pub(crate) terminal_agents: Vec<InteractiveAgent>,

    // Split group state
    pub(crate) split_groups: Vec<crate::domain::models::SplitGroup>,
    /// ID of the split group currently being viewed (if any).
    pub(crate) active_split_id: Option<String>,
    /// True = right/bottom panel is focused in split view.
    pub(crate) split_right_focused: bool,
    /// Whether the split picker overlay is open.
    pub(crate) split_picker_open: bool,
    pub(crate) split_picker_idx: usize,
    pub(crate) split_picker_orientation: crate::domain::models::SplitOrientation,
    /// (name, type_label) for each available session in the picker.
    pub(crate) split_picker_sessions: Vec<(String, String)>,

    // Daemon info
    pub(crate) daemon_running: bool,
    pub(crate) daemon_pid: Option<u32>,
    pub(crate) daemon_version: String,

    // UI state
    pub(crate) selected: usize,
    pub(crate) focus: Focus,
    pub(crate) log_content: String,
    pub(crate) log_scroll: u16,
    pub(crate) running: bool,
    pub(crate) new_agent_dialog: Option<NewAgentDialog>,
    pub(crate) quit_confirm: bool,

    // Brian's Brain automaton (sidebar decoration)
    pub(crate) sidebar_brain: Option<crate::tui::brians_brain::BriansBrain>,
    // Brian's Brain for home banner background
    pub(crate) home_brain: Option<crate::tui::brians_brain::BriansBrain>,

    // System monitoring (updated asynchronously to avoid UI freezes)
    pub(crate) system_info: crate::system::SystemInfo,
    pub(crate) system_info_rx: std::sync::mpsc::Receiver<crate::system::SystemInfo>,
    pub(crate) last_system_update: std::time::Instant,
    pub(crate) process_start_time: std::time::Instant,

    // Layout state
    pub(crate) sidebar_click_map: Vec<(usize, u16, u16)>,
    pub(crate) sidebar_visible: bool,
    pub(crate) term_width: u16,
    pub(crate) show_legend: bool,
    pub(crate) show_copied: bool,
    pub(crate) copied_at: std::time::Instant,
    pub(crate) last_scroll_at: std::time::Instant,
    pub(crate) last_panel_inner: (u16, u16),
    pub(crate) whimsg: crate::tui::whimsg::Whimsg,
    /// Hash of the last log chunk scanned for whimsg triggers — avoids re-firing
    /// on the same content every tick.
    pub(crate) whimsg_last_log_hash: u64,
    pub(crate) context_transfer_modal: Option<crate::tui::context_transfer::ContextTransferModal>,
    pub(crate) context_transfer_config: crate::tui::context_transfer::ContextTransferConfig,
    /// Prompt templates loaded from registry
    #[allow(dead_code)]
    pub(crate) prompt_templates: crate::tui::prompt_templates::PromptTemplates,
    /// Current simple prompt dialog state
    pub(crate) simple_prompt_dialog: Option<SimplePromptDialog>,
    /// Persisted prompt-builder sessions per workdir (cleared on send).
    pub(crate) prompt_builder_sessions:
        HashMap<PathBuf, crate::tui::app::dialog::PromptBuilderSession>,
    /// Whether to send OS-level desktop notifications (agent done/failed).
    pub(crate) notifications_enabled: bool,
    /// Notification service for sending cross-platform notifications.
    pub(crate) notification_service: Arc<dyn NotificationService>,
    /// IDs of runs that were active on the previous refresh tick.
    pub(crate) prev_active_run_ids: std::collections::HashSet<String>,
    /// Tick counter for animation (increments every refresh)
    pub(crate) animation_tick: u32,
    /// Preferred unit for sysinfo temperature labels.
    pub(crate) temperature_unit: crate::domain::canopy_config::TemperatureUnit,
    /// Terminal autocomplete suggestion picker (shown on Tab).
    pub(crate) suggestion_picker: Option<crate::tui::terminal_history::SuggestionPicker>,
    /// Per-session terminal histories (loaded on demand, cached in memory).
    pub(crate) terminal_histories: HashMap<String, crate::tui::terminal_history::SessionHistory>,
    /// Terminal scrollback search state (Ctrl+F).
    pub(crate) terminal_search: Option<TerminalSearch>,
    /// CLI launch usage counters (persisted to disk).
    pub(crate) cli_usage: crate::domain::usage_stats::CliUsage,
}
