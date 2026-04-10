//! Event loop — polls crossterm events with a tick for data refresh.
//!
//! In Agent focus mode, all keys are forwarded to the PTY stdin
//! except double-Esc which detaches back to the sidebar.

use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use std::time::Duration;

use super::agent::key_to_bytes;
use super::app::{AgentEntry, App, Focus};
use super::ui;

type Terminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>;

/// Main event loop: draw → poll events → refresh data.
pub fn run_event_loop(terminal: &mut Terminal, app: &mut App) -> Result<()> {
    while app.running {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Shorter poll when agent is focused for responsive I/O
        let tick = if app.focus == Focus::Agent {
            Duration::from_millis(50)
        } else {
            Duration::from_secs(1)
        };

        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key(app, key.code, key.modifiers)?;
                }
            }
        }

        app.refresh()?;
    }

    app.cleanup();
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    match app.focus {
        Focus::Sidebar => handle_sidebar_key(app, code),
        Focus::LogPanel => handle_log_key(app, code),
        Focus::NewAgentDialog => handle_dialog_key(app, code),
        Focus::Agent => handle_agent_key(app, code, modifiers),
    }
}

fn handle_sidebar_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => app.running = false,
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
        KeyCode::Enter | KeyCode::Char('l') => {
            if matches!(app.selected_agent(), Some(AgentEntry::Interactive(_))) {
                app.focus = Focus::Agent;
            } else {
                app.focus = Focus::LogPanel;
            }
        }
        KeyCode::Char('e') | KeyCode::Char('d') => {
            let _ = app.toggle_enable();
        }
        KeyCode::Char('r') => {
            let _ = app.rerun_selected();
        }
        KeyCode::Char('n') => app.open_new_agent_dialog(),
        KeyCode::Char('x') => app.kill_selected_agent(),
        _ => {}
    }
    Ok(())
}

fn handle_log_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc | KeyCode::Char('h') => app.focus = Focus::Sidebar,
        KeyCode::Char('q') => app.running = false,
        KeyCode::Char('j') | KeyCode::Down => app.scroll_log_down(),
        KeyCode::Char('k') | KeyCode::Up => app.scroll_log_up(),
        _ => {}
    }
    Ok(())
}

/// In Agent focus: forward all keys to the PTY, except double-Esc to detach.
fn handle_agent_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // Double Esc = detach back to sidebar
    if code == KeyCode::Esc {
        if app.last_esc.elapsed() < Duration::from_millis(400) {
            app.focus = Focus::Sidebar;
            app.last_esc = std::time::Instant::now() - Duration::from_secs(10);
            return Ok(());
        }
        app.last_esc = std::time::Instant::now();
        // Single Esc still gets sent to the agent below
    }

    let Some(AgentEntry::Interactive(idx)) = app.selected_agent() else {
        app.focus = Focus::Sidebar;
        return Ok(());
    };
    let idx = *idx;

    // Shift+Up/Down or PageUp/PageDown = scroll through history
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    match code {
        KeyCode::Up if shift => {
            app.interactive_agents[idx].scroll_offset += 3;
            return Ok(());
        }
        KeyCode::Down if shift => {
            let agent = &mut app.interactive_agents[idx];
            agent.scroll_offset = agent.scroll_offset.saturating_sub(3);
            return Ok(());
        }
        KeyCode::PageUp => {
            app.interactive_agents[idx].scroll_offset += 15;
            return Ok(());
        }
        KeyCode::PageDown => {
            let agent = &mut app.interactive_agents[idx];
            agent.scroll_offset = agent.scroll_offset.saturating_sub(15);
            return Ok(());
        }
        _ => {}
    }

    // Any other key resets scroll to live view
    if app.interactive_agents[idx].scroll_offset > 0 {
        app.interactive_agents[idx].scroll_offset = 0;
    }

    let bytes = key_to_bytes(code, modifiers);
    if !bytes.is_empty() {
        let _ = app.interactive_agents[idx].write_to_pty(&bytes);
    }

    Ok(())
}

fn handle_dialog_key(app: &mut App, code: KeyCode) -> Result<()> {
    let Some(dialog) = &mut app.new_agent_dialog else {
        return Ok(());
    };

    match code {
        KeyCode::Esc => app.close_new_agent_dialog(),
        KeyCode::Tab => {
            if dialog.field == 1 {
                dialog.complete_path();
            } else {
                dialog.field = 1;
            }
        }
        KeyCode::Enter => {
            let _ = app.launch_new_agent();
        }
        _ => {
            let Some(dialog) = &mut app.new_agent_dialog else {
                return Ok(());
            };
            match dialog.field {
                0 => match code {
                    KeyCode::Left | KeyCode::Char('h') => dialog.prev_cli(),
                    KeyCode::Right | KeyCode::Char('l') => dialog.next_cli(),
                    _ => {}
                },
                1 => match code {
                    KeyCode::Char(c) => dialog.working_dir.push(c),
                    KeyCode::Backspace => {
                        dialog.working_dir.pop();
                    }
                    KeyCode::Up => dialog.field = 0,
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}
