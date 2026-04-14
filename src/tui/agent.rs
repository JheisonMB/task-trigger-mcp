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

/// Install no-op handlers for SIGHUP and SIGPIPE so that when a PTY child
/// exits the canopy process is not accidentally terminated.
#[cfg(unix)]
fn ignore_signals() {
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
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
}

impl InteractiveAgent {
    /// Spawn a new interactive agent in a PTY with a virtual terminal.
    ///
    /// `cols` and `rows` should match the panel area where the agent will render.
    /// `interactive_args` come from the registry (e.g. `--tui`, `-c`, etc.).
    /// `fallback_args` are tried if the primary args fail (e.g. kiro `chat`).
    pub fn spawn(
        cli: Cli,
        working_dir: &str,
        cols: u16,
        rows: u16,
        interactive_args: Option<&str>,
        fallback_args: Option<&str>,
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

        let id = format!("session-{}", &uuid::Uuid::new_v4().to_string()[..8]);

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
        let scrollback_len = if let Ok(vt) = self.vt.lock() {
            vt.screen().scrollback()
        } else {
            0
        };

        if let Ok(mut history) = self.prompt_history.lock() {
            history.push_back(PromptEntry {
                input: input.to_string(),
                output_range: (scrollback_len, scrollback_len), // end updated later
                timestamp: Utc::now(),
            });
            // Keep only the last MAX_PROMPT_HISTORY entries
            while history.len() > MAX_PROMPT_HISTORY {
                history.pop_front();
            }
        }
    }

    /// Inject a context block into the agent's PTY, followed by Enter.
    ///
    /// Replaces Unix newlines with carriage returns (PTY convention) and
    /// writes the whole payload in one shot rather than char-by-char.
    pub fn inject_context(&self, ctx_block: &str) -> Result<()> {
        let bytes: Vec<u8> = ctx_block
            .bytes()
            .map(|b| if b == b'\n' { b'\r' } else { b })
            .collect();
        self.write_to_pty(&bytes)?;
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
        let Ok(mut vt) = self.vt.lock() else {
            return String::new();
        };

        let (rows, _cols) = vt.screen().size();
        let total_available = vt.screen().scrollback() + rows as usize;
        let to_take = n.min(total_available);

        if to_take == 0 {
            return String::new();
        }

        let prev_scroll = vt.screen().scrollback();
        let mut lines = Vec::with_capacity(to_take);

        // We want the absolute last 'to_take' lines.
        // Screen rows are 0..rows. Scrollback 1..N are above row 0.
        // This is a bit complex in vt100, so we'll use a simpler heuristic:
        // Read the visible screen, and if we need more, read from scrollback.
        let visible = vt.screen().contents();
        let visible_lines: Vec<String> = visible.lines().map(|s| s.to_string()).collect();

        if visible_lines.len() >= to_take {
            let start = visible_lines.len() - to_take;
            return visible_lines[start..].join("\n");
        }

        // Need more from scrollback
        let remaining = to_take - visible_lines.len();
        for i in 1..=remaining {
            vt.screen_mut().set_scrollback(i);
            // Just take the last line of the screen at this scrollback offset
            let screen_at_offset = vt.screen().contents();
            if let Some(last_line) = screen_at_offset.lines().last() {
                lines.push(last_line.to_string());
            }
        }
        vt.screen_mut().set_scrollback(prev_scroll);

        lines.reverse();
        lines.extend(visible_lines);

        let start = lines.len().saturating_sub(to_take);
        lines[start..].join("\n")
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
