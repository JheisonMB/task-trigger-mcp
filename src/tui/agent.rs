//! Interactive agent management — PTY + vt100 virtual terminal.
//!
//! Each agent runs in a PTY. A background thread reads PTY output and
//! feeds it into a `vt100::Parser` which maintains a virtual screen buffer.
//! The UI reads this screen buffer and renders it as ratatui cells inside
//! the right panel — fully embedded, with colors and cursor.

use anyhow::Result;
use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::VecDeque;
#[cfg(unix)]
use std::io;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ratatui::style::Color;

use crate::domain::models::Cli;

/// Status of an interactive agent.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Running,
    Exited(i32),
}

/// A recorded user prompt with its response range in scrollback.
#[derive(Clone)]
#[allow(dead_code)]
pub struct PromptEntry {
    pub input: String,
    /// (start_line, end_line) in the vt100 scrollback buffer
    /// representing the agent's response to this prompt.
    pub output_range: (usize, usize),
    pub timestamp: DateTime<Utc>,
}

/// Maximum number of prompt entries to keep in the ring buffer.
const MAX_PROMPT_HISTORY: usize = 20;
const TERMINAL_SHELL_PROMPTS: [&str; 5] = ["$ ", "# ", "> ", "% ", "❯ "];
const SENSITIVE_PROMPT_HINTS: [&str; 7] = [
    "passphrase",
    "password",
    "passcode",
    "pin",
    "otp",
    "token",
    "verification code",
];

/// Sanitize a line of terminal output: strip ANSI escape sequences and
/// control characters, but preserve printable text.
fn sanitize_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_escape = false;

    for ch in line.chars() {
        if ch == '\x1b' {
            // ESC — start of ANSI sequence
            in_escape = true;
        } else if in_escape {
            // Inside escape sequence: keep going until we see a letter or ~
            if ch.is_ascii_alphabetic() || ch == '~' || ch == 'K' || ch == 'H' {
                in_escape = false;
            }
            // Drop the escape char and the sequence
        } else if ch.is_control() && ch != '\t' {
            // Drop other control chars except tab
        } else {
            out.push(ch);
        }
    }

    out
}

fn line_looks_sensitive_prompt(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let lower = trimmed.to_ascii_lowercase();
    SENSITIVE_PROMPT_HINTS
        .iter()
        .any(|hint| lower.contains(hint))
        && (trimmed.ends_with(':') || trimmed.ends_with('?'))
}

fn strip_shell_prompt_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    for prefix in TERMINAL_SHELL_PROMPTS {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.to_string();
        }
    }
    trimmed.to_string()
}

/// Returns true if `c` is a box-drawing or block-element character
/// (Unicode ranges: Box Drawing U+2500–257F, Block Elements U+2580–259F).
fn is_decoration_char(c: char) -> bool {
    matches!(c,
        // Box Drawing (U+2500–U+257F)
        '─'..='╿'
        // Block Elements (U+2580–U+259F) — includes █ ░ ▒ ▓ and all half/quarter blocks
        | '▀'..='▟'
        // Dashes
        | '‐' | '–' | '—' | '−'
    )
}

/// Detect if a line is UI noise that should be excluded from context transfer.
///
/// Catches: box-drawing borders, block-element bars, CLI prompts,
/// status bars, tool-use indicators, MCP messages, and similar chrome.
/// Lines with box-drawing borders that contain text content between them
/// are NOT treated as UI lines — the content is extracted by `strip_borders`.
fn is_ui_line(line: &str) -> bool {
    let trimmed = line.trim();

    if trimmed.is_empty() {
        return true;
    }

    // Lines composed entirely of decoration chars + whitespace
    if trimmed.chars().all(|c| c == ' ' || is_decoration_char(c)) {
        return true;
    }

    // Lines with box-drawing borders: extract inner text and check if it's empty.
    // TUI agents (opencode, claude, copilot) render responses inside │ borders.
    if trimmed.starts_with('│') || trimmed.starts_with('┃') || trimmed.starts_with('║') {
        let inner = strip_borders(trimmed);
        // If inner is empty after stripping, it's a purely decorative border
        return inner.trim().is_empty();
    }

    // Common CLI prompts/status indicators
    if trimmed.starts_with('❯')
        || trimmed.starts_with('$')
        || trimmed.starts_with('#')
        || trimmed.starts_with("...")
        || trimmed.contains("───")
    {
        return true;
    }

    // Bullet/status symbols at start
    if trimmed.starts_with('●')
        || trimmed.starts_with('▌')
        || trimmed.starts_with('▣')
        || trimmed.starts_with('▹')
        || trimmed.starts_with('ℹ')
        || trimmed.starts_with('✓')
    {
        return true;
    }

    // Status bar / footer patterns
    if trimmed.contains("Environment")
        || trimmed.contains("remaining")
        || trimmed.contains("for shortcuts")
        || trimmed.contains("Shift+Tab")
        || trimmed.contains("MCP issues")
        || trimmed.contains("MCP servers")
        || trimmed.contains("workspace (")
    {
        return true;
    }

    false
}

/// Strip box-drawing border characters from the beginning and end of a line.
/// E.g. `│ Hello world │` → `Hello world`.
fn strip_borders(line: &str) -> &str {
    let trimmed = line.trim();
    // Strip leading border char(s) + whitespace
    let start = trimmed
        .char_indices()
        .find(|(_, c)| !is_decoration_char(*c) && *c != ' ')
        .map(|(i, _)| i)
        .unwrap_or(trimmed.len());
    let inner = &trimmed[start..];
    // Strip trailing border char(s) + whitespace
    let end = inner
        .char_indices()
        .rev()
        .find(|(_, c)| !is_decoration_char(*c) && *c != ' ')
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    &inner[..end]
}
/// Read absolute buffer lines [from_abs, to_abs) from a vt100 parser.
///
/// `set_scrollback(S)` shows the window at absolute positions
/// `[max_sb - S .. max_sb - S + rows - 1]`.  Stepping down from a high
/// offset (oldest) to 0 (current screen) in increments of `rows` gives
/// non-overlapping pages.  `next_expected` ensures each absolute index is
/// emitted exactly once even when the final clamped page overlaps with the
/// previous one.
fn read_abs_range(
    vt: &mut vt100::Parser,
    max_sb: usize,
    rows: usize,
    from_abs: usize,
    to_abs: usize,
) -> Vec<String> {
    if to_abs <= from_abs || rows == 0 {
        return Vec::new();
    }

    // Find the page-aligned scrollback offset that first covers `from_abs`.
    // set_scrollback(S) starts at abs = max_sb - S.
    // We need max_sb - S <= from_abs  =>  S >= max_sb - from_abs.
    // Round up to the nearest multiple of `rows`, capped at max_sb.
    let s_for_from = max_sb.saturating_sub(from_abs);
    let s_start = if s_for_from % rows == 0 {
        s_for_from
    } else {
        ((s_for_from / rows) + 1) * rows
    }
    .min(max_sb);

    let mut collected: Vec<String> = Vec::with_capacity(to_abs.saturating_sub(from_abs));
    let mut next_expected = from_abs;
    let mut s = s_start;

    loop {
        let clamped = s.min(max_sb);
        let page_start_abs = max_sb - clamped;
        vt.screen_mut().set_scrollback(clamped);
        let content = vt.screen().contents();

        for (i, line) in content.lines().enumerate() {
            let abs_idx = page_start_abs + i;
            if abs_idx == next_expected && abs_idx < to_abs {
                // Always advance the index — filtering only controls
                // whether the line is included in output, not whether
                // subsequent lines are reachable.
                next_expected += 1;
                let sanitized = sanitize_line(line).trim_end().to_string();
                if !sanitized.trim().is_empty() && !is_ui_line(&sanitized) {
                    // Strip box-drawing borders from response lines (TUI agents
                    // render output inside │ borders).
                    let cleaned = strip_borders(&sanitized);
                    if !cleaned.is_empty() {
                        collected.push(cleaned.to_string());
                    } else {
                        collected.push(sanitized);
                    }
                }
            }
        }

        if next_expected >= to_abs || s == 0 {
            break;
        }
        s = s.saturating_sub(rows);
    }

    collected
}

/// Install no-op handlers for SIGHUP and SIGPIPE so that when a PTY child
/// exits the canopy process is not accidentally terminated.
#[cfg(unix)]
fn ignore_signals() {
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Creative session names assigned when the user doesn't provide one (interactive agents).
const RANDOM_NAMES: &[&str] = &[
    "liquidambar",
    "wollemia",
    "metasequoia",
    "paulownia",
    "liriodendron",
    "cryptomeria",
    "cunninghamia",
    "nothofagus",
    "podocarpus",
    "fitzroya",
    "cephalotaxus",
    "taiwania",
    "sciadopitys",
    "toona",
    "cedrus",
    "sequoia",
    "juniperus",
    "stereum",
    "larix",
    "carpinus",
    "castanea",
    "aesculus",
    "juglans",
    "platanus",
    "agaricus",
    "araucaria",
    "zelkova",
    "magnolia",
    "ginkgo",
    "quercus",
    "amanita",
    "boletus",
    "morchella",
    "cantharellus",
    "pleurotus",
    "ganoderma",
    "lentinula",
    "psilocybe",
    "coprinus",
    "hydnum",
    "trametes",
    "russula",
    "lactarius",
    "populus",
    "laricifomes",
    "cordyceps",
    "hericium",
    "laetiporus",
    "armillaria",
    "clavaria",
    "geastrum",
    "lycoperdon",
    "mycena",
    "marasmius",
    "cortinarius",
    "hygrocybe",
    "xylaria",
    "fistulina",
    "grifola",
    "stereum",
    "daedalea",
    "clitocybe",
    "inocybe",
    "pholiota",
    "stropharia",
    "suillus",
    "omphalotus",
    "sparassis",
    "calvatia",
    "phallus",
];

/// Session names for background/scheduled agents (weather/nature terms).
const BACKGROUND_NAMES: &[&str] = &[
    "foehn",
    "mistral",
    "tramontana",
    "galerna",
    "fitoncida",
    "espora",
    "micela",
    "rizoma",
    "lignina",
    "tanino",
    "resina",
    "humus",
];

/// Session names for raw terminal sessions (minerals).
const TERMINAL_NAMES: &[&str] = &[
    "feldspato",
    "cuarzo",
    "olivino",
    "piroxeno",
    "anfíbol",
    "biotita",
    "moscovita",
    "clorita",
    "caolinita",
    "illita",
    "esmectita",
    "vermiculita",
    "haloisita",
    "sepiolita",
    "palygorskita",
    "bario",
    "estroncio",
    "rubidio",
    "vanadio",
    "cobalto",
    "molibdeno",
    "niquel",
    "cesio",
];

/// Pick a name from `names` that isn't already in `existing`.
///
/// First tries each name bare.  On collision appends `-2`, `-3`, …
/// Falls back to a UUID-based ID if every combination is taken.
fn pick_name_from(names: &[&str], existing: &[&str]) -> String {
    use rand::prelude::IndexedRandom;

    // First try: pick a random bare name that isn't in use
    let available: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !existing.contains(n))
        .collect();
    if let Some(&name) = available.choose(&mut rand::rng()) {
        return name.to_string();
    }

    // Second try: pick a random base name and try <name>-2, <name>-3, …
    if let Some(&base) = names.choose(&mut rand::rng()) {
        for n in 2..=999u32 {
            let candidate = format!("{}-{}", base, n);
            if !existing.contains(&candidate.as_str()) {
                return candidate;
            }
        }
    }

    format!("session-{}", &uuid::Uuid::new_v4().to_string()[..8])
}

/// Pick a random name for interactive agents (trees + fungi).
pub fn pick_random_name(existing: &[&str]) -> String {
    pick_name_from(RANDOM_NAMES, existing)
}

/// Pick a name for terminal sessions (minerals).
pub fn pick_terminal_name(existing: &[&str]) -> String {
    pick_name_from(TERMINAL_NAMES, existing)
}

/// Pick a name for background/scheduled agents (weather/nature terms).
#[allow(dead_code)]
pub fn pick_background_name(existing: &[&str]) -> String {
    pick_name_from(BACKGROUND_NAMES, existing)
}

/// An interactive agent with a virtual terminal screen.
pub struct InteractiveAgent {
    /// UUID-based permanent identifier
    pub id: String,
    /// Display name for personality (from RANDOM_NAMES)
    pub name: String,
    pub cli: Cli,
    #[allow(dead_code)]
    pub working_dir: String,
    #[allow(dead_code)]
    pub started_at: DateTime<Utc>,
    pub status: AgentStatus,
    /// Accent color for this agent's TUI elements (from `CliConfig`).
    pub accent_color: Color,
    /// Whether this is a raw terminal session (no AI CLI).
    #[allow(dead_code)]
    pub is_terminal: bool,
    /// Shell binary for terminal sessions (e.g. "zsh", "bash").
    pub shell: String,
    /// PTY writer — send bytes to the agent's stdin.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Virtual terminal screen — fed by PTY output (for live rendering with colors).
    pub(super) vt: Arc<Mutex<vt100::Parser>>,
    /// Child process handle.
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send>>>,
    /// PTY master — needed for resize.
    master: Arc<Mutex<Box<dyn portable_pty::MasterPty + Send>>>,
    /// Scroll offset (0 = bottom/live, positive = scrolled up).
    pub scroll_offset: usize,
    /// Last known PTY dimensions (for resize detection).
    pub last_pty_cols: u16,
    pub last_pty_rows: u16,
    /// Ring buffer of recent user prompts and their responses.
    pub prompt_history: Arc<Mutex<VecDeque<PromptEntry>>>,
    /// Current accumulated input (characters since last Enter).
    pub input_buffer: Arc<Mutex<String>>,
    /// Tracks when the PTY last received output (for detecting idle/waiting state).
    last_output_at: Arc<Mutex<DateTime<Utc>>>,
    /// Tracks when the user last viewed/focused this agent.
    last_viewed_at: Arc<Mutex<DateTime<Utc>>>,
    /// Whether the exit notification has already been sent (avoids repeats).
    pub exit_notified: bool,
    /// Warp-like input mode: accumulate keystrokes in input_buffer, send on Enter.
    /// Only used for terminal sessions (is_terminal == true).
    pub warp_mode: bool,
    /// Cursor position within the warp input buffer (byte offset).
    pub warp_cursor: usize,
    /// Index into session history for Up/Down browsing (None = not browsing).
    pub history_index: Option<usize>,
    /// True once the current shell line has been materialized in the PTY
    /// and warp input should stay synchronized from PTY edits.
    pub warp_passthrough: bool,
}

impl InteractiveAgent {
    /// Spawn a new interactive agent in a PTY with a virtual terminal.
    ///
    /// `cols` and `rows` should match the panel area where the agent will render.
    /// `interactive_args` come from the registry (e.g. `--tui`, `-c`, etc.).
    /// `fallback_args` are tried if the primary args fail (e.g. kiro `chat`).
    /// `name` is an optional user-provided session name (random if None).
    /// `existing_ids` is used to avoid name collisions.
    /// `model` and `model_flag` allow passing a model selection (e.g. `-m gpt-4`).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        cli: Cli,
        working_dir: &str,
        cols: u16,
        rows: u16,
        interactive_args: Option<&str>,
        fallback_args: Option<&str>,
        accent_color: Color,
        name: Option<&str>,
        existing_ids: &[&str],
        model: Option<&str>,
        model_flag: Option<&str>,
    ) -> Result<Self> {
        #[cfg(unix)]
        ignore_signals();

        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(cli.command_name());
        // Apply registry-driven interactive args (e.g. "--tui", "-c", etc.)
        // If primary args fail and fallback is available, try that instead.
        let args_to_use = if let Some(args) = interactive_args {
            Some(args)
        } else {
            fallback_args
        };
        if let Some(args) = args_to_use {
            for arg in args.split_whitespace() {
                if !arg.is_empty() {
                    cmd.arg(arg);
                }
            }
        }
        // Apply model flag if user selected a model (e.g. `-m gpt-4`)
        if let (Some(flag), Some(m)) = (model_flag, model) {
            if !m.is_empty() {
                cmd.arg(flag);
                cmd.arg(m);
            }
        }
        cmd.cwd(working_dir);

        // Advertise truecolor capability so child CLIs (Kiro, etc.) use
        // 24-bit RGB color sequences for their accent colors instead of
        // limited 16-color ANSI codes that get mapped to wrong hues.
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(cmd)?;
        // Drop slave so the PTY closes when the child exits
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let master = pair.master;

        let vt = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000)));
        let vt_clone = Arc::clone(&vt);

        let last_output_at = Arc::new(Mutex::new(Utc::now()));
        let last_output_at_clone = Arc::clone(&last_output_at);

        // Background thread: read PTY output → feed into vt100 parser
        std::thread::spawn(move || {
            let mut tmp = [0u8; 4096];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut parser) = vt_clone.lock() {
                            parser.process(&tmp[..n]);
                        }
                        // Stamp last output time so is_waiting_for_input() can detect idle
                        if let Ok(mut t) = last_output_at_clone.lock() {
                            *t = Utc::now();
                        }
                    }
                }
            }
        });

        let id = uuid::Uuid::new_v4().to_string();
        let name = if let Some(n) = name {
            n.to_string()
        } else {
            pick_random_name(existing_ids)
        };

        Ok(Self {
            id,
            name,
            cli,
            working_dir: working_dir.to_string(),
            started_at: Utc::now(),
            status: AgentStatus::Running,
            accent_color,
            is_terminal: false,
            shell: String::new(),
            writer: Arc::new(Mutex::new(writer)),
            vt,
            child: Arc::new(Mutex::new(child)),
            master: Arc::new(Mutex::new(master)),
            scroll_offset: 0,
            last_pty_cols: cols,
            last_pty_rows: rows,
            prompt_history: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_PROMPT_HISTORY))),
            input_buffer: Arc::new(Mutex::new(String::new())),
            last_output_at,
            last_viewed_at: Arc::new(Mutex::new(Utc::now())),
            exit_notified: false,
            warp_mode: false,
            warp_cursor: 0,
            history_index: None,
            warp_passthrough: false,
        })
    }

    /// Spawn a raw terminal session (no AI CLI model).
    ///
    /// Uses `shell` as the command (e.g. `"bash"`, `"zsh"`).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_terminal(
        shell: &str,
        working_dir: &str,
        cols: u16,
        rows: u16,
        name: Option<&str>,
        existing_ids: &[&str],
        accent_color: Color,
    ) -> Result<Self> {
        #[cfg(unix)]
        ignore_signals();

        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(shell);
        cmd.cwd(working_dir);
        // Compact prompt since warp mode shows its own prompt line
        cmd.env("PS1", "$ ");
        cmd.env("PROMPT_COMMAND", "");
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");

        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let master = pair.master;

        let vt = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000)));
        let vt_clone = Arc::clone(&vt);

        let last_output_at = Arc::new(Mutex::new(Utc::now()));
        let last_output_at_clone = Arc::clone(&last_output_at);

        std::thread::spawn(move || {
            let mut tmp = [0u8; 4096];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut parser) = vt_clone.lock() {
                            parser.process(&tmp[..n]);
                        }
                        if let Ok(mut t) = last_output_at_clone.lock() {
                            *t = Utc::now();
                        }
                    }
                }
            }
        });

        let id = uuid::Uuid::new_v4().to_string();
        let session_name = if let Some(n) = name {
            n.to_string()
        } else {
            pick_terminal_name(existing_ids)
        };
        let cli = Cli::new(shell);

        Ok(Self {
            id,
            name: session_name,
            cli,
            working_dir: working_dir.to_string(),
            started_at: Utc::now(),
            status: AgentStatus::Running,
            accent_color,
            is_terminal: true,
            shell: shell.to_string(),
            writer: Arc::new(Mutex::new(writer)),
            vt,
            child: Arc::new(Mutex::new(child)),
            master: Arc::new(Mutex::new(master)),
            scroll_offset: 0,
            last_pty_cols: cols,
            last_pty_rows: rows,
            prompt_history: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_PROMPT_HISTORY))),
            input_buffer: Arc::new(Mutex::new(String::new())),
            last_output_at,
            last_viewed_at: Arc::new(Mutex::new(Utc::now())),
            exit_notified: false,
            warp_mode: true,
            warp_cursor: 0,
            history_index: None,
            warp_passthrough: false,
        })
    }

    /// Mark the agent as having been viewed/attended by the user.
    /// This suppresses the waiting indicator until new output arrives.
    pub fn mark_viewed(&self) {
        if let Ok(mut t) = self.last_viewed_at.lock() {
            *t = Utc::now();
        }
    }

    /// Send raw bytes to the agent's PTY stdin.
    pub fn write_to_pty(&self, data: &[u8]) -> Result<()> {
        if let Ok(mut w) = self.writer.lock() {
            w.write_all(data)?;
            w.flush()?;
        }
        Ok(())
    }

    /// Record a user prompt submission. Called when Enter is pressed.
    /// Captures the input and the current scrollback depth as the start
    /// of the response range (visible screen content starts at max_sb).
    pub fn record_prompt(&self, input: &str) {
        // Use scrollback depth only — visible screen lines are indexed from max_sb
        // upward, so this is the correct start for the response range.
        let history_depth = if let Ok(mut vt) = self.vt.lock() {
            let prev = vt.screen().scrollback();
            vt.screen_mut().set_scrollback(usize::MAX);
            let depth = vt.screen().scrollback();
            vt.screen_mut().set_scrollback(prev);
            depth
        } else {
            0
        };

        if let Ok(mut history) = self.prompt_history.lock() {
            // Close out the previous entry's response range using total_depth
            // so that visible screen lines (not yet in scrollback) are included.
            if let Some(last) = history.back_mut() {
                last.output_range.1 = history_depth + {
                    // Re-lock vt to get rows for total depth
                    if let Ok(vt) = self.vt.lock() {
                        vt.screen().size().0 as usize
                    } else {
                        0
                    }
                };
            }
            history.push_back(PromptEntry {
                input: input.to_string(),
                output_range: (history_depth, history_depth),
                timestamp: Utc::now(),
            });
            while history.len() > MAX_PROMPT_HISTORY {
                history.pop_front();
            }
        }
    }

    /// Get a snapshot of the virtual terminal screen for rendering.
    ///
    /// Uses vt100's native scrollback: `set_scrollback(N)` shifts the
    /// viewport N rows up into history.  `cell()` then returns the
    /// visible (possibly scrolled) content with full colors.
    pub fn screen_snapshot(&self) -> Option<ScreenSnapshot> {
        let mut vt = self.vt.lock().ok()?;
        vt.screen_mut().set_scrollback(self.scroll_offset);

        let screen = vt.screen();
        let (rows, cols) = screen.size();

        let mut cells = Vec::with_capacity(rows as usize);
        for row in 0..rows {
            let mut row_cells = Vec::with_capacity(cols as usize);
            for col in 0..cols {
                row_cells.push(screen.cell(row, col).map(|c| VtCell {
                    ch: c.contents().to_string(),
                    fg: convert_color(c.fgcolor()),
                    bg: convert_color(c.bgcolor()),
                    bold: c.bold(),
                    underline: c.underline(),
                    inverse: c.inverse(),
                }));
            }
            cells.push(row_cells);
        }

        let cursor = screen.cursor_position();
        let scrolled = self.scroll_offset > 0;

        Some(ScreenSnapshot {
            cells,
            cursor_row: if scrolled { rows } else { cursor.0 },
            cursor_col: cursor.1,
            scrolled,
        })
    }

    /// Get a plain-text preview of the screen (for sidebar log preview).
    pub fn output(&self) -> String {
        if let Ok(vt) = self.vt.lock() {
            vt.screen().contents()
        } else {
            String::new()
        }
    }

    /// Get the last N lines of the entire history (scrollback + visible screen).
    pub fn last_lines(&self, n: usize) -> String {
        if n == 0 {
            return String::new();
        }
        let Ok(mut vt) = self.vt.lock() else {
            return String::new();
        };
        let (rows, _) = vt.screen().size();
        let rows = rows as usize;
        if rows == 0 {
            return String::new();
        }
        let prev_sb = vt.screen().scrollback();
        vt.screen_mut().set_scrollback(usize::MAX);
        let max_sb = vt.screen().scrollback();
        let total_lines = max_sb + rows;
        let from_abs = total_lines.saturating_sub(n);
        let result = read_abs_range(&mut vt, max_sb, rows, from_abs, total_lines);
        vt.screen_mut().set_scrollback(prev_sb);
        result.join("\n")
    }

    /// Extract lines at absolute buffer positions [from_abs, to_abs).
    ///
    /// `from_abs` and `to_abs` are the scrollback-history-depth values captured
    /// via `record_prompt` (i.e. the result of `set_scrollback(usize::MAX)` at
    /// the time of capture, not the current scroll offset).
    pub fn lines_at_scrollback_range(&self, from_abs: usize, to_abs: usize) -> String {
        if to_abs <= from_abs {
            return String::new();
        }
        let Ok(mut vt) = self.vt.lock() else {
            return String::new();
        };
        let (rows, _) = vt.screen().size();
        let rows = rows as usize;
        if rows == 0 {
            return String::new();
        }
        let prev_sb = vt.screen().scrollback();
        vt.screen_mut().set_scrollback(usize::MAX);
        let max_sb = vt.screen().scrollback();
        let result = read_abs_range(&mut vt, max_sb, rows, from_abs, to_abs);
        vt.screen_mut().set_scrollback(prev_sb);
        result.join("\n")
    }

    /// Detect if the agent appears to be waiting for user input / confirmation.
    ///
    /// Strategy (cursor-position only — no text-pattern matching):
    /// 1. Process is running AND idle for 1_000ms (1 second) with no output.
    /// 2. Cursor is on the last non-empty row of the visible screen.
    ///
    /// Text patterns were removed because box-drawing characters appear in all
    /// TUI agents' normal output and caused constant false positives.
    pub fn is_waiting_for_input(&self) -> bool {
        if self.status != AgentStatus::Running {
            return false;
        }

        let idle_threshold = std::time::Duration::from_millis(2000);
        let (is_idle, new_output_since_viewed) =
            if let (Ok(out), Ok(view)) = (self.last_output_at.lock(), self.last_viewed_at.lock()) {
                let elapsed = Utc::now().signed_duration_since(*out);
                (
                    elapsed.num_milliseconds() >= idle_threshold.as_millis() as i64,
                    *out > *view,
                )
            } else {
                (false, false)
            };

        if !is_idle || !new_output_since_viewed {
            return false;
        }

        let Some(screen) = self.screen_snapshot() else {
            return true;
        };

        let rows = screen.cells.len();
        if rows == 0 {
            return true;
        }

        // Find the last row that has any visible (non-space) content.
        let last_nonempty = (0..rows).rev().find(|&r| {
            screen.cells.get(r).is_some_and(|row| {
                row.iter()
                    .any(|c| c.as_ref().is_some_and(|cell| !cell.ch.trim().is_empty()))
            })
        });

        let Some(_last_content_row) = last_nonempty else {
            // Screen is blank but process is idle — assume waiting.
            return true;
        };

        // Cursor should be in the lower half of the screen (common for prompts/input fields).
        // This is more permissive than checking exact row position, reducing false negatives
        // while still being selective enough to avoid constant false positives.
        let cursor_in_lower_half = (screen.cursor_row as usize) > rows / 2;
        is_idle && cursor_in_lower_half
    }

    /// Check if the process has exited.
    pub fn poll(&mut self) {
        if self.status != AgentStatus::Running {
            return;
        }
        if let Ok(mut child) = self.child.lock() {
            if let Ok(Some(status)) = child.try_wait() {
                self.status = AgentStatus::Exited(status.exit_code().try_into().unwrap_or(-1));
            }
        }
    }

    /// Extract the last N non-empty lines from the PTY output.
    /// Useful for capturing error messages from agents that exit immediately.
    pub fn last_output_lines(&self, n: usize) -> Vec<String> {
        let Ok(parser) = self.vt.lock() else {
            return Vec::new();
        };
        let screen = parser.screen();
        let rows = screen.size().0;
        let mut lines: Vec<String> = Vec::new();
        for row in 0..rows {
            let line = screen
                .rows_formatted(row, row + 1)
                .next()
                .unwrap_or_default();
            let text = String::from_utf8_lossy(&line).trim().to_string();
            if !text.is_empty() {
                lines.push(text);
            }
        }
        // Also check scrollback
        let scrollback = screen.scrollback();
        if scrollback > 0 {
            let saved = parser.screen().rows_formatted(0, 0);
            for line in saved {
                let text = String::from_utf8_lossy(&line).trim().to_string();
                if !text.is_empty() {
                    lines.push(text);
                }
            }
        }
        lines
            .into_iter()
            .rev()
            .take(n)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    /// Kill the agent process.
    ///
    /// Sends SIGHUP + SIGKILL to the process group (Unix) or just kills the
    /// child process. Marks the agent as exited immediately — does NOT block
    /// on `child.wait()`. The background PTY reader thread will detect EOF and
    /// exit on its own; the OS reaps the child process.
    pub fn kill(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            #[cfg(unix)]
            send_sighup_to_group(child.as_mut());
            let _ = child.kill();
        }
        self.status = AgentStatus::Exited(-9);
    }

    /// Resize the PTY and virtual terminal (e.g. on terminal window resize).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.last_pty_cols = cols;
        self.last_pty_rows = rows;
        // Resize the actual PTY so the process knows about the new size
        if let Ok(m) = self.master.lock() {
            let _ = m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
        // Also resize the virtual terminal screen
        if let Ok(mut vt) = self.vt.lock() {
            vt.screen_mut().set_size(rows, cols);
        }
    }

    /// Maximum scroll offset — try setting a large value and read back
    /// the clamped result from vt100's scrollback.
    pub fn max_scroll(&self) -> usize {
        if let Ok(mut vt) = self.vt.lock() {
            let prev = vt.screen().scrollback();
            vt.screen_mut().set_scrollback(usize::MAX);
            let max = vt.screen().scrollback();
            vt.screen_mut().set_scrollback(prev);
            max
        } else {
            0
        }
    }

    /// Total lines available: scrollback history + visible screen rows.
    /// Use this as the upper bound for context capture so that content
    /// currently on screen (not yet scrolled into history) is included.
    pub fn total_depth(&self) -> usize {
        if let Ok(mut vt) = self.vt.lock() {
            let prev = vt.screen().scrollback();
            vt.screen_mut().set_scrollback(usize::MAX);
            let max_sb = vt.screen().scrollback();
            let (rows, _) = vt.screen().size();
            vt.screen_mut().set_scrollback(prev);
            max_sb + rows as usize
        } else {
            0
        }
    }

    fn current_visible_line_text(&self) -> Option<String> {
        let vt = self.vt.try_lock().ok()?;
        let screen = vt.screen();
        let (rows, cols) = screen.size();
        if rows == 0 || cols == 0 {
            return None;
        }

        let row = screen.cursor_position().0.min(rows.saturating_sub(1));
        let mut line = String::new();
        for col in 0..cols {
            if let Some(cell) = screen.cell(row, col) {
                line.push_str(cell.contents());
            }
        }

        Some(sanitize_line(&line).trim_end().to_string())
    }

    /// Get plain text from the current visible screen area.
    /// This is used for copying clean text without ANSI formatting.
    pub fn get_plain_text_from_screen(&self) -> Option<String> {
        let vt = self.vt.try_lock().ok()?;
        let screen = vt.screen();
        let (rows, cols) = screen.size();
        if rows == 0 || cols == 0 {
            return None;
        }

        let mut text = String::new();
        // Get text from all visible lines
        for row in 0..rows {
            let mut line = String::new();
            for col in 0..cols {
                if let Some(cell) = screen.cell(row, col) {
                    line.push_str(cell.contents());
                }
            }
            // Add line to text, preserving newlines
            if !line.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&sanitize_line(&line));
            }
        }

        Some(text)
    }

    /// Get plain text from a specific selection area.
    /// Used when user selects text with mouse.
    #[allow(dead_code)]
    pub fn get_plain_text_from_selection(
        &self,
        start_row: usize,
        end_row: usize,
    ) -> Option<String> {
        let vt = self.vt.try_lock().ok()?;
        let screen = vt.screen();
        let (rows, cols) = screen.size();
        if rows == 0 || cols == 0 {
            return None;
        }

        let mut text = String::new();
        let start_row = start_row.min(rows.saturating_sub(1) as usize);
        let end_row = end_row.min(rows.saturating_sub(1) as usize);

        for row in start_row..=end_row {
            let mut line = String::new();
            for col in 0..cols {
                if let Some(cell) = screen.cell(row as u16, col) {
                    line.push_str(cell.contents());
                }
            }
            if !line.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&sanitize_line(&line));
            }
        }

        Some(text)
    }

    /// Get plain text from the line at a specific screen position.
    #[allow(dead_code)]
    pub fn get_line_text_at_position(&self, col: u16, row: u16) -> Option<String> {
        let vt = self.vt.try_lock().ok()?;
        let screen = vt.screen();
        let (screen_rows, screen_cols) = screen.size();
        if screen_rows == 0 || screen_cols == 0 {
            return None;
        }

        // Adjust for scroll offset
        let actual_row = if self.in_alternate_screen() {
            // In alternate screen, row is relative to visible area
            row.saturating_add(self.scroll_offset as u16)
        } else {
            // In normal screen, row is absolute in scrollback
            row
        };

        // Check if position is within screen bounds
        if actual_row >= screen_rows || col >= screen_cols {
            return None;
        }

        let mut line = String::new();
        for c in 0..screen_cols {
            if let Some(cell) = screen.cell(actual_row, c) {
                line.push_str(cell.contents());
            }
        }

        let sanitized = sanitize_line(&line);
        if sanitized.trim().is_empty() {
            None
        } else {
            Some(sanitized)
        }
    }

    /// Get plain text from the current cursor line.
    #[allow(dead_code)]
    pub fn get_current_line_text(&self) -> Option<String> {
        let vt = self.vt.try_lock().ok()?;
        let screen = vt.screen();
        let (screen_rows, screen_cols) = screen.size();
        if screen_rows == 0 || screen_cols == 0 {
            return None;
        }

        let cursor_pos = screen.cursor_position();
        let cursor_row = cursor_pos.0;
        let actual_row = if self.in_alternate_screen() {
            // In alternate screen, cursor row is relative to visible area
            cursor_row.saturating_add(self.scroll_offset as u16)
        } else {
            // In normal screen, cursor row is absolute in scrollback
            cursor_row
        };

        // Check if cursor position is within screen bounds
        if actual_row >= screen_rows {
            return None;
        }

        let mut line = String::new();
        for c in 0..screen_cols {
            if let Some(cell) = screen.cell(actual_row, c) {
                line.push_str(cell.contents());
            }
        }

        let sanitized = sanitize_line(&line);
        if sanitized.trim().is_empty() {
            None
        } else {
            Some(sanitized)
        }
    }

    /// Get clean PTY line text at a specific screen position, excluding UI elements.
    /// This is a non-blocking, fast-path version that avoids expensive operations.
    pub fn get_clean_pty_line_at_position(&self, col: u16, row: u16) -> Option<String> {
        // Quick early return if position is obviously invalid
        if row > 1000 || col > 1000 {
            // Reasonable upper bounds
            return None;
        }

        let vt = self.vt.try_lock().ok()?;
        let screen = vt.screen();
        let (screen_rows, screen_cols) = screen.size();

        // Early return for empty screen
        if screen_rows == 0 || screen_cols == 0 {
            return None;
        }

        // Adjust for scroll offset and screen mode
        let actual_row = if self.in_alternate_screen() {
            // In alternate screen, row is relative to visible area
            row.saturating_add(self.scroll_offset as u16)
        } else {
            // In normal screen, row is absolute in scrollback
            row
        };

        // Check if position is within screen bounds
        if actual_row >= screen_rows || col >= screen_cols {
            return None;
        }

        // Get the full line with panic protection
        let line = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut line = String::with_capacity(screen_cols as usize);
            for c in 0..screen_cols {
                if let Some(cell) = screen.cell(actual_row, c) {
                    line.push_str(cell.contents());
                }
            }
            line
        }))
        .ok()?;

        let sanitized = sanitize_line(&line);

        // Quick check for empty or UI-only lines
        if sanitized.trim().is_empty() {
            return None;
        }

        // Check if this line is UI noise that should be excluded
        if is_ui_line(&sanitized) {
            return None;
        }

        // Extract clean content, stripping borders and UI elements
        let clean_content = strip_borders(&sanitized);

        if clean_content.trim().is_empty() {
            None
        } else {
            Some(clean_content.to_string())
        }
    }

    pub fn is_sensitive_input_active(&self) -> bool {
        self.current_visible_line_text()
            .is_some_and(|line| line_looks_sensitive_prompt(&line))
    }

    pub fn should_bypass_warp_input(&self) -> bool {
        self.in_alternate_screen() || self.is_sensitive_input_active()
    }

    pub fn sync_warp_input_from_pty(&self, wait: Duration) -> Option<String> {
        if wait > Duration::ZERO {
            std::thread::sleep(wait);
        }

        self.current_visible_line_text()
            .map(|line| strip_shell_prompt_prefix(&line))
    }

    /// Whether the child process is using alternate screen mode.
    pub fn in_alternate_screen(&self) -> bool {
        self.vt
            .try_lock()
            .map(|vt| vt.screen().alternate_screen())
            .unwrap_or(false)
    }

    /// Forward a mouse scroll event to the PTY.
    ///
    /// Checks the child's mouse protocol mode.  If mouse reporting is
    /// active, sends the wheel event in the correct encoding.  Otherwise
    /// falls back to arrow-key sequences.
    pub fn forward_scroll(&self, scroll_up: bool) -> Result<()> {
        let (mode, encoding, cols) = {
            let vt = self.vt.lock().map_err(|_| anyhow::anyhow!("vt lock"))?;
            let s = vt.screen();
            (
                s.mouse_protocol_mode(),
                s.mouse_protocol_encoding(),
                s.size().1,
            )
        };

        use vt100::MouseProtocolEncoding as MPE;
        use vt100::MouseProtocolMode as MPM;

        match mode {
            MPM::None => {
                // No mouse protocol — send PgUp/PgDn (works in most TUIs)
                let seq: &[u8] = if scroll_up { b"\x1b[5~" } else { b"\x1b[6~" };
                self.write_to_pty(seq)
            }
            _ => {
                let button: u8 = if scroll_up { 64 } else { 65 };
                let col: u16 = cols / 2;
                let row: u16 = 10;
                let single = match encoding {
                    MPE::Sgr => format!("\x1b[<{};{};{}M", button, col + 1, row + 1).into_bytes(),
                    _ => {
                        vec![
                            0x1b,
                            b'[',
                            b'M',
                            button + 32,
                            (col as u8).wrapping_add(33),
                            (row as u8).wrapping_add(33),
                        ]
                    }
                };
                self.write_to_pty(&single)
            }
        }
    }

    /// Update the working directory. Used when CD command is executed.
    pub fn update_working_dir(&mut self, new_dir: &str) {
        self.working_dir = new_dir.to_string();
    }
}

#[cfg(unix)]
fn send_sighup_to_group(child: &mut dyn portable_pty::Child) {
    let Some(pid) = child.process_id().map(|pid| pid as i32) else {
        return;
    };
    let _ = send_signal_to_group(pid, libc::SIGHUP);
}

#[cfg(unix)]
fn send_signal_to_group(pid: i32, signal: i32) -> io::Result<()> {
    let result = unsafe { libc::killpg(pid, signal) };
    if result == 0 {
        return Ok(());
    }

    let err = io::Error::last_os_error();
    if err.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(err)
}

/// A snapshot of the virtual terminal screen.
pub struct ScreenSnapshot {
    pub cells: Vec<Vec<Option<VtCell>>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub scrolled: bool,
}

/// A single cell from the virtual terminal.
pub struct VtCell {
    pub ch: String,
    pub fg: ratatui::style::Color,
    pub bg: ratatui::style::Color,
    pub bold: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// Convert vt100 color to ratatui color.
///
/// For ANSI indices 0-15 uses the standard xterm-256color palette
/// (what modern terminals and CLIs expect).  For 256-color extensions
/// (indices 16-255) delegates to `Color::Indexed` since those are
/// well‑standardised (6×6×6 colour cube + grayscale).  Truecolor RGB
/// passes through unchanged.
fn convert_color(color: vt100::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) if i < 16 => {
            const XTERM_16: [Color; 16] = [
                Color::Rgb(0, 0, 0),       // 0  black
                Color::Rgb(205, 0, 0),     // 1  red
                Color::Rgb(0, 205, 0),     // 2  green
                Color::Rgb(205, 205, 0),   // 3  yellow
                Color::Rgb(0, 0, 238),     // 4  blue
                Color::Rgb(180, 0, 180),   // 5  purple (Kiro's brand color)
                Color::Rgb(0, 205, 205),   // 6  cyan
                Color::Rgb(229, 229, 229), // 7  white
                Color::Rgb(127, 127, 127), // 8  bright black
                Color::Rgb(255, 0, 0),     // 9  bright red
                Color::Rgb(0, 255, 0),     // 10 bright green
                Color::Rgb(255, 255, 0),   // 11 bright yellow
                Color::Rgb(92, 92, 255),   // 12 bright blue
                Color::Rgb(200, 50, 255),  // 13 bright purple
                Color::Rgb(0, 255, 255),   // 14 bright cyan
                Color::Rgb(255, 255, 255), // 15 bright white
            ];
            XTERM_16[i as usize]
        }
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Convert a crossterm key event to raw bytes for the PTY.
pub fn key_to_bytes(
    code: ratatui::crossterm::event::KeyCode,
    modifiers: ratatui::crossterm::event::KeyModifiers,
) -> Vec<u8> {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    match code {
        KeyCode::Char(c) => {
            if modifiers.contains(KeyModifiers::CONTROL) {
                let ctrl = (c.to_ascii_lowercase() as u8)
                    .wrapping_sub(b'a')
                    .wrapping_add(1);
                vec![ctrl]
            } else {
                let mut buf = [0u8; 4];
                let s = c.encode_utf8(&mut buf);
                s.as_bytes().to_vec()
            }
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::{line_looks_sensitive_prompt, strip_shell_prompt_prefix};

    #[test]
    fn detects_sensitive_prompts() {
        assert!(line_looks_sensitive_prompt(
            "Enter passphrase for key '/tmp/id_rsa':"
        ));
        assert!(line_looks_sensitive_prompt(
            "Password for https://example.com?"
        ));
        assert!(!line_looks_sensitive_prompt("$ git push"));
    }

    #[test]
    fn strips_common_shell_prompts() {
        assert_eq!(strip_shell_prompt_prefix("$ git status"), "git status");
        assert_eq!(strip_shell_prompt_prefix("# cargo test"), "cargo test");
        assert_eq!(strip_shell_prompt_prefix("plain text"), "plain text");
    }
}
