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
    pub started_at: DateTime<Utc>,
    pub status: AgentStatus,
    /// PTY writer — send bytes to the agent's stdin.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Virtual terminal screen — fed by PTY output.
    vt: Arc<Mutex<vt100::Parser>>,
    /// Child process handle.
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send>>>,
}

impl InteractiveAgent {
    /// Spawn a new interactive agent in a PTY with a virtual terminal.
    ///
    /// `cols` and `rows` should match the panel area where the agent will render.
    pub fn spawn(cli: Cli, working_dir: &str, cols: u16, rows: u16) -> Result<Self> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd = CommandBuilder::new(cli.command_name());
        match cli {
            Cli::OpenCode => {}
            Cli::Kiro => {
                cmd.arg("chat");
                cmd.arg("--trust-all-tools");
            }
            Cli::Copilot => {}
        }
        cmd.cwd(working_dir);

        let child = pair.slave.spawn_command(cmd)?;
        // Drop slave so the PTY closes when the child exits
        drop(pair.slave);

        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;

        let vt = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
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
            writer: Arc::new(Mutex::new(writer)),
            vt,
            child: Arc::new(Mutex::new(child)),
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
    pub fn screen_snapshot(&self) -> Option<ScreenSnapshot> {
        let vt = self.vt.lock().ok()?;
        let screen = vt.screen();
        let (rows, cols) = (screen.size().0, screen.size().1);

        let mut cells = Vec::with_capacity(rows as usize);
        for row in 0..rows {
            let mut row_cells = Vec::with_capacity(cols as usize);
            for col in 0..cols {
                let cell = screen.cell(row, col);
                row_cells.push(cell.map(|c| VtCell {
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
        })
    }

    /// Get a plain-text preview of the screen (for sidebar log preview).
    pub fn output(&self) -> String {
        let Some(snap) = self.screen_snapshot() else {
            return String::new();
        };
        snap.cells
            .iter()
            .map(|row| {
                row.iter()
                    .map(|c| {
                        c.as_ref()
                            .map(|c| c.ch.as_str())
                            .unwrap_or(" ")
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Check if the process has exited.
    pub fn poll(&mut self) {
        if self.status != AgentStatus::Running {
            return;
        }
        if let Ok(mut child) = self.child.lock() {
            if let Ok(Some(status)) = child.try_wait() {
                self.status =
                    AgentStatus::Exited(status.exit_code().try_into().unwrap_or(-1));
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
}

/// A snapshot of the virtual terminal screen.
#[allow(dead_code)]
pub struct ScreenSnapshot {
    pub cells: Vec<Vec<Option<VtCell>>>,
    pub cursor_row: u16,
    pub cursor_col: u16,
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
