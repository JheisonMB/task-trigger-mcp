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
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

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

    let mut collected: Vec<String> = Vec::with_capacity(to_abs - from_abs);
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

/// Creative session names assigned when the user doesn't provide one.
const RANDOM_NAMES: &[&str] = &[
    "andromeda", "orion", "nova", "atlas", "phoenix",
    "nebula", "vega", "helios", "lyra", "titan",
    "aurora", "cosmo", "polaris", "iris", "zenith",
    "quasar", "celeste", "nimbus", "ember", "zephyr",
    "solaris", "astrid", "comet", "pulsar", "echo",
];

/// Pick a random name from `RANDOM_NAMES` that isn't already in use.
/// Falls back to UUID-based ID if all names are taken.
fn pick_random_name(existing_ids: &[&str]) -> String {
    use rand::prelude::IndexedRandom;
    let available: Vec<&str> = RANDOM_NAMES
        .iter()
        .copied()
        .filter(|n| !existing_ids.iter().any(|e| e == n))
        .collect();
    if let Some(name) = available.choose(&mut rand::rng()) {
        name.to_string()
    } else {
        format!("session-{}", &uuid::Uuid::new_v4().to_string()[..8])
    }
}

/// An interactive agent with a virtual terminal screen.
pub struct InteractiveAgent {
    pub id: String,
    pub cli: Cli,
    #[allow(dead_code)]
    pub working_dir: String,
    #[allow(dead_code)]
    pub started_at: DateTime<Utc>,
    pub status: AgentStatus,
    /// Accent color for this agent's TUI elements (from `CliConfig`).
    pub accent_color: Color,
    /// PTY writer — send bytes to the agent's stdin.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Virtual terminal screen — fed by PTY output (for live rendering with colors).
    vt: Arc<Mutex<vt100::Parser>>,
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
    /// Tracks when the screen last changed (for detecting idle state).
    last_screen_update: Arc<Mutex<DateTime<Utc>>>,
    /// Whether the exit notification has already been sent (avoids repeats).
    pub exit_notified: bool,
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

        let child = pair.slave.spawn_command(cmd)?;
        // Drop slave so the PTY closes when the child exits
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let master = pair.master;

        let vt = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000)));
        let vt_clone = Arc::clone(&vt);

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
                    }
                }
            }
        });

        let id = if let Some(n) = name {
            n.to_string()
        } else {
            pick_random_name(existing_ids)
        };

        Ok(Self {
            id,
            cli,
            working_dir: working_dir.to_string(),
            started_at: Utc::now(),
            status: AgentStatus::Running,
            accent_color,
            writer: Arc::new(Mutex::new(writer)),
            vt,
            child: Arc::new(Mutex::new(child)),
            master: Arc::new(Mutex::new(master)),
            scroll_offset: 0,
            last_pty_cols: cols,
            last_pty_rows: rows,
            prompt_history: Arc::new(Mutex::new(VecDeque::with_capacity(MAX_PROMPT_HISTORY))),
            input_buffer: Arc::new(Mutex::new(String::new())),
            last_screen_update: Arc::new(Mutex::new(Utc::now())),
            exit_notified: false,
        })
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
    /// Captures the input and the current scrollback length as the start
    /// of the response range.
    pub fn record_prompt(&self, input: &str) {
        // Use the actual scrollback history depth (not the current scroll offset).
        // set_scrollback(usize::MAX) clamps to the real history size.
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
            // Close out the previous entry's response range.
            if let Some(last) = history.back_mut() {
                last.output_range.1 = history_depth;
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

    /// Inject a context block into the agent's PTY as a single paste.
    ///
    /// Uses bracketed paste mode (`ESC[200~` … `ESC[201~`) so the agent
    /// treats the whole block as one pasted input rather than interpreting
    /// each newline as a separate Enter press.
    pub fn inject_context(&self, ctx_block: &str) -> Result<()> {
        // Begin bracketed paste
        self.write_to_pty(b"\x1b[200~")?;
        let bytes: Vec<u8> = ctx_block
            .bytes()
            .map(|b| if b == b'\n' { b'\r' } else { b })
            .collect();
        self.write_to_pty(&bytes)?;
        // End bracketed paste
        self.write_to_pty(b"\x1b[201~")?;
        // Final Enter to submit
        self.write_to_pty(b"\r")?;
        Ok(())
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

        // Update last access time (used for idle detection)
        if let Ok(mut last_update) = self.last_screen_update.lock() {
            *last_update = Utc::now();
        }

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

    /// Detect if the agent appears to be waiting for user input.
    ///
    /// Heuristics:
    /// - Cursor is on the last row (indicates prompt area)
    /// - Screen hasn't been accessed for at least 100ms (idle)
    /// - Process is still running
    pub fn is_waiting_for_input(&self) -> bool {
        if self.status != AgentStatus::Running {
            return false;
        }

        let Some(screen) = self.screen_snapshot() else {
            return false;
        };

        let (rows, _cols) = (
            screen.cells.len() as u16,
            screen.cells.first().map(|r| r.len() as u16).unwrap_or(0),
        );

        // Not at the bottom of visible area
        if screen.cursor_row < rows.saturating_sub(2) {
            return false;
        }

        // Check if screen is idle (no access in the last 150ms)
        let idle_threshold = std::time::Duration::from_millis(150);
        let last_update = self.last_screen_update.lock().ok();
        if let Some(last_update) = last_update {
            let elapsed = Utc::now().signed_duration_since(*last_update);
            if elapsed.num_milliseconds() < idle_threshold.as_millis() as i64 {
                return false;
            }
        }

        true
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
            let line = screen.rows_formatted(row, row + 1).next().unwrap_or_default();
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
        lines.into_iter().rev().take(n).collect::<Vec<_>>().into_iter().rev().collect()
    }

    /// Kill the agent process.
    pub fn kill(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
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

    /// Whether the child process is using alternate screen mode.
    pub fn in_alternate_screen(&self) -> bool {
        self.vt
            .lock()
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
                // Send 3 scroll events for smoother scrolling
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
                let bytes: Vec<u8> = single.repeat(3);
                self.write_to_pty(&bytes)
            }
        }
    }
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
/// For the standard 16 ANSI colors (indices 0-15), we map to explicit RGB
/// values instead of `Color::Indexed`, because ratatui's indexed palette
/// uses terminal-dependent colors that don't match what the agent expects.
fn convert_color(color: vt100::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) if i < 16 => {
            // Standard 16 ANSI colors with explicit RGB values.
            const ANSI_16: [Color; 16] = [
                Color::Rgb(0, 0, 0),       // 0  black
                Color::Rgb(170, 0, 0),     // 1  red
                Color::Rgb(0, 170, 0),     // 2  green
                Color::Rgb(170, 85, 0),    // 3  yellow
                Color::Rgb(0, 0, 170),     // 4  blue
                Color::Rgb(170, 0, 170),   // 5  magenta
                Color::Rgb(0, 170, 170),   // 6  cyan
                Color::Rgb(170, 170, 170), // 7  white (dark white = light gray)
                Color::Rgb(85, 85, 85),    // 8  bright black (gray)
                Color::Rgb(255, 85, 85),   // 9  bright red
                Color::Rgb(85, 255, 85),   // 10 bright green
                Color::Rgb(255, 255, 85),  // 11 bright yellow
                Color::Rgb(85, 85, 255),   // 12 bright blue
                Color::Rgb(255, 85, 255),  // 13 bright magenta
                Color::Rgb(85, 255, 255),  // 14 bright cyan
                Color::Rgb(255, 255, 255), // 15 bright white
            ];
            ANSI_16[i as usize]
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
