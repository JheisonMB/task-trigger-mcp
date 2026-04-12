//! Interactive agent management — PTY + vt100 virtual terminal.
//!
//! Each agent runs in a PTY. A background thread reads PTY output and
//! feeds it into a `vt100::Parser` which maintains a virtual screen buffer.
//! The UI reads this screen buffer and renders it as ratatui cells inside
//! the right panel — fully embedded, with colors and cursor.

use anyhow::Result;
use chrono::{DateTime, Utc};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use crate::domain::models::Cli;

/// Status of an interactive agent.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Running,
    Exited(i32),
}

/// An interactive agent with a virtual terminal screen.
pub struct InteractiveAgent {
    pub id: String,
    pub cli: Cli,
    pub working_dir: String,
    #[allow(dead_code)]
    pub started_at: DateTime<Utc>,
    pub status: AgentStatus,
    /// PTY writer — send bytes to the agent's stdin.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Virtual terminal screen — fed by PTY output (for live rendering with colors).
    vt: Arc<Mutex<vt100::Parser>>,
    /// Child process handle.
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send>>>,
    /// Scroll offset (0 = bottom/live, positive = scrolled up).
    pub scroll_offset: usize,
    /// Accumulated plain-text output for scrollback history.
    output_buffer: Arc<Mutex<String>>,
    /// PTY dimensions.
    rows: u16,
    cols: u16,
}

impl InteractiveAgent {
    /// Spawn a new interactive agent in a PTY with a virtual terminal.
    ///
    /// `cols` and `rows` should match the panel area where the agent will render.
    /// Interactive args come from the registry (`CliConfig::interactive_args`).
    pub fn spawn(
        cli: Cli,
        working_dir: &str,
        cols: u16,
        rows: u16,
        interactive_args: Option<&str>,
    ) -> Result<Self> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(cli.command_name());
        // Apply registry-driven interactive args (e.g. "--tui", "-c", etc.)
        if let Some(args) = interactive_args {
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

        let vt = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 10_000)));
        let vt_clone = Arc::clone(&vt);
        let output_buffer = Arc::new(Mutex::new(String::new()));
        let output_clone = Arc::clone(&output_buffer);

        // Background thread: read PTY output → feed into vt100 parser and text buffer
        std::thread::spawn(move || {
            let mut tmp = [0u8; 4096];
            loop {
                match reader.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut parser) = vt_clone.lock() {
                            parser.process(&tmp[..n]);
                        }
                        // Append plain text to output buffer
                        if let Ok(mut buf) = output_clone.lock() {
                            if let Ok(text) = String::from_utf8(tmp[..n].to_vec()) {
                                buf.push_str(&text);
                                // Cap buffer at ~500KB to avoid memory issues
                                if buf.len() > 500_000 {
                                    let split = buf.len() - 400_000;
                                    if let Some(idx) = buf[split..].find('\n') {
                                        buf.drain(..split + idx);
                                    }
                                }
                            }
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
            writer: Arc::new(Mutex::new(writer)),
            vt,
            child: Arc::new(Mutex::new(child)),
            scroll_offset: 0,
            output_buffer,
            rows,
            cols,
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

    /// Get a snapshot of the virtual terminal screen for rendering.
    ///
    /// When `scroll_offset == 0`, returns the live vt100 screen with colors.
    /// When `scroll_offset > 0`, renders plain-text scrollback from the output buffer.
    pub fn screen_snapshot(&self) -> Option<ScreenSnapshot> {
        if self.scroll_offset == 0 {
            // Live view: use vt100 screen with full colors
            let mut vt = self.vt.lock().ok()?;
            vt.screen_mut().set_scrollback(0);
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
            Some(ScreenSnapshot {
                cells,
                cursor_row: cursor.0,
                cursor_col: cursor.1,
                scrolled: false,
                text_lines: None,
            })
        } else {
            // Scrolled back: render plain text from output buffer
            let output = self.output_buffer.lock().ok()?;
            let lines: Vec<&str> = output.lines().collect();
            let total_lines = lines.len();

            // Calculate which lines to show based on scroll_offset
            // scroll_offset=1 means show last (total-1) lines, etc.
            let visible_rows = self.rows as usize;
            let end = total_lines.saturating_sub(self.scroll_offset - 1);
            let start = end.saturating_sub(visible_rows);

            let text_lines: Vec<String> = lines[start..end.min(total_lines)]
                .iter()
                .map(|l| {
                    // Truncate/pad to fit cols
                    let ch_count = l.chars().count();
                    if ch_count > self.cols as usize {
                        l.chars().take(self.cols as usize).collect()
                    } else {
                        format!("{:<width$}", l, width = self.cols as usize)
                    }
                })
                .collect();

            // Pad to fill visible rows
            let padded_rows = text_lines.len();
            let mut padded: Vec<String> = text_lines;
            for _ in 0..(visible_rows.saturating_sub(padded_rows)) {
                padded.push(" ".repeat(self.cols as usize));
            }

            // Convert text lines to cell format (no colors for scrollback)
            let mut cells = Vec::with_capacity(padded.len());
            for line in &padded {
                let mut row_cells = Vec::with_capacity(self.cols as usize);
                for ch in line.chars() {
                    row_cells.push(Some(VtCell {
                        ch: ch.to_string(),
                        fg: ratatui::style::Color::Gray,
                        bg: ratatui::style::Color::Reset,
                        bold: false,
                        underline: false,
                        inverse: false,
                    }));
                }
                // Pad remaining columns
                for _ in 0..((self.cols as usize).saturating_sub(line.chars().count())) {
                    row_cells.push(Some(VtCell {
                        ch: " ".to_string(),
                        fg: ratatui::style::Color::Gray,
                        bg: ratatui::style::Color::Reset,
                        bold: false,
                        underline: false,
                        inverse: false,
                    }));
                }
                cells.push(row_cells);
            }

            Some(ScreenSnapshot {
                cells,
                cursor_row: self.rows, // hide cursor when scrolled
                cursor_col: 0,
                scrolled: true,
                text_lines: Some(padded),
            })
        }
    }

    /// Get a plain-text preview of the screen (for sidebar log preview).
    pub fn output(&self) -> String {
        if let Ok(output) = self.output_buffer.lock() {
            // Return last few lines of output buffer
            let lines: Vec<&str> = output.lines().collect();
            let take = lines.len().min(self.rows as usize);
            lines
                .iter()
                .rev()
                .take(take)
                .rev()
                .map(|l| l.trim_end())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            String::new()
        }
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

    /// Maximum scroll offset based on accumulated output.
    pub fn max_scroll(&self) -> usize {
        self.output_buffer
            .lock()
            .map(|b| b.lines().count())
            .unwrap_or(0)
    }
}

/// A snapshot of the virtual terminal screen.
#[allow(dead_code)]
pub struct ScreenSnapshot {
    pub cells: Vec<Vec<Option<VtCell>>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub scrolled: bool,
    /// Plain-text lines (only present when scrolled into history).
    pub text_lines: Option<Vec<String>>,
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
fn convert_color(color: vt100::Color) -> ratatui::style::Color {
    use ratatui::style::Color;
    match color {
        vt100::Color::Default => Color::Reset,
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
