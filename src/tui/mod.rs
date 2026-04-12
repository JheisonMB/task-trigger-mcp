//! Canopy Agent Hub — TUI for monitoring and managing agents.
//!
//! Reads the daemon's `SQLite` database in read-only mode (WAL allows
//! concurrent readers) and displays tasks, watchers, and their logs
//! in a card-based sidebar with a live log panel.

mod agent;
mod app;
mod brians_brain;
mod event;
mod ui;

use anyhow::{Context, Result};
use ratatui::crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io;
use std::sync::Arc;

use crate::db::Database;

use app::App;
use event::run_event_loop;

/// Entry point for `canopy tui`.
pub fn run_tui() -> Result<()> {
    let data_dir = crate::ensure_data_dir()?;
    let db_path = data_dir.join("tasks.db");

    if !db_path.exists() {
        anyhow::bail!(
            "No database found at {}. Is the daemon running?",
            db_path.display()
        );
    }

    let db = Arc::new(Database::new(&db_path).context("Failed to open database")?);
    let mut app = App::new(Arc::clone(&db), &data_dir)?;

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    // Run
    let result = run_event_loop(&mut terminal, &mut app);

    // Restore terminal — always, even on error
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}
