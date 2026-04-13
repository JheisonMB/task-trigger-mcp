//! Event loop — polls crossterm events with a tick for data refresh.
//!
//! Navigation flow:
//!   Home (screensaver) → Preview (agent details) → Focus (log / PTY)
//!
//! Keys:
//!   Home:    ↑↓ → Preview, q quit, Esc confirm-quit, n new agent
//!   Preview: ↑↓ navigate, Enter → Focus, Esc → Home, agent actions
//!   Focus:   background → scroll log, interactive → PTY, `EscEsc` → Preview

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

        // Tick speed adapts to what needs frequent repaints
        let tick = match app.focus {
            Focus::Agent | Focus::NewAgentDialog => Duration::from_millis(50),
            Focus::Preview if matches!(app.selected_agent(), Some(AgentEntry::Interactive(_))) => {
                Duration::from_millis(100)
            }
            Focus::Home if app.brain.as_ref().is_some_and(|b| b.active) => {
                Duration::from_millis(100)
            }
            _ => Duration::from_secs(1),
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
        Focus::Home => handle_home_key(app, code),
        Focus::Preview => handle_preview_key(app, code),
        Focus::NewAgentDialog => handle_dialog_key(app, code),
        Focus::Agent => handle_agent_key(app, code, modifiers),
    }
}

// ── Home: screensaver — arrows enter Preview ────────────────────────

fn handle_home_key(app: &mut App, code: KeyCode) -> Result<()> {
    // Quit-confirmation overlay intercepts all keys
    if app.quit_confirm {
        match code {
            KeyCode::Char('y') | KeyCode::Enter => app.running = false,
            _ => app.quit_confirm = false,
        }
        return Ok(());
    }

    match code {
        KeyCode::Char('q') => app.running = false,
        KeyCode::Esc => {
            app.quit_confirm = true;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !app.agents.is_empty() {
                app.dismiss_brain();
                app.selected = 0;
                app.log_scroll = 0;
                app.focus = Focus::Preview;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if !app.agents.is_empty() {
                app.dismiss_brain();
                app.selected = app.agents.len().saturating_sub(1);
                app.log_scroll = 0;
                app.focus = Focus::Preview;
            }
        }
        KeyCode::Enter => {
            if !app.agents.is_empty() {
                app.dismiss_brain();
                app.log_scroll = 0;
                app.focus = Focus::Preview;
            }
        }
        KeyCode::Char('n') => app.open_new_agent_dialog(),
        _ => {}
    }
    Ok(())
}

// ── Preview: navigate agents, Enter → Focus ─────────────────────────

fn handle_preview_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Esc | KeyCode::Char('h') => {
            app.focus = Focus::Home;
        }
        KeyCode::Enter | KeyCode::Char('l') => {
            app.log_scroll = 0;
            app.focus = Focus::Agent;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.select_next();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.select_prev();
        }
        KeyCode::Char('e') | KeyCode::Char('d') => {
            let _ = app.toggle_enable();
        }
        KeyCode::Char('r') => {
            let _ = app.rerun_selected();
        }
        KeyCode::Char('D') => {
            let _ = app.delete_selected();
        }
        KeyCode::Char('n') => app.open_new_agent_dialog(),
        KeyCode::Char('x') => app.kill_selected_agent(),
        KeyCode::Char('q') => app.running = false,
        _ => {}
    }
    Ok(())
}

// ── Focus: PTY interaction or log scroll ────────────────────────────

fn handle_agent_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // Color legend toggle
    if code == KeyCode::Char('?') {
        app.show_legend = !app.show_legend;
    }

    // Background agents: simple log-scrolling, single Esc → Preview
    if !matches!(app.selected_agent(), Some(AgentEntry::Interactive(_))) {
        match code {
            KeyCode::Esc | KeyCode::Char('h') => app.focus = Focus::Preview,
            KeyCode::Down | KeyCode::Char('j') => app.scroll_log_down(),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_log_up(),
            KeyCode::Char('q') => app.running = false,
            _ => {}
        }
        return Ok(());
    }

    // Interactive agents: double-Esc → Preview
    if code == KeyCode::Esc {
        if app.last_esc.elapsed() < Duration::from_millis(400) {
            app.focus = Focus::Preview;
            app.last_esc = std::time::Instant::now() - Duration::from_secs(10);
            return Ok(());
        }
        app.last_esc = std::time::Instant::now();
    }

    // Tab = cycle to next interactive agent (focus mode)
    // Shift+Tab = toggle sidebar
    if code == KeyCode::BackTab {
        app.toggle_sidebar();
        return Ok(());
    }
    if code == KeyCode::Tab {
        app.next_interactive();
        return Ok(());
    }

    let Some(AgentEntry::Interactive(idx)) = app.selected_agent() else {
        app.focus = Focus::Home;
        return Ok(());
    };
    let idx = *idx;

    // Shift+Up/Down or PageUp/PageDown = scroll history
    let shift = modifiers.contains(KeyModifiers::SHIFT);
    let max_scroll = app.interactive_agents[idx].max_scroll();
    match code {
        KeyCode::Up if shift => {
            app.interactive_agents[idx].scroll_offset =
                (app.interactive_agents[idx].scroll_offset + 3).min(max_scroll);
            return Ok(());
        }
        KeyCode::Down if shift => {
            let agent = &mut app.interactive_agents[idx];
            agent.scroll_offset = agent.scroll_offset.saturating_sub(3);
            return Ok(());
        }
        KeyCode::PageUp => {
            app.interactive_agents[idx].scroll_offset =
                (app.interactive_agents[idx].scroll_offset + 15).min(max_scroll);
            return Ok(());
        }
        KeyCode::PageDown => {
            let agent = &mut app.interactive_agents[idx];
            agent.scroll_offset = agent.scroll_offset.saturating_sub(15);
            return Ok(());
        }
        _ => {}
    }

    // Typing resets scroll to live view — but only for printable characters
    // and Backspace/Enter so that arrow keys can still navigate agent history
    if app.interactive_agents[idx].scroll_offset > 0 {
        let resets_scroll = matches!(
            code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace | KeyCode::Tab
        );
        if resets_scroll {
            app.interactive_agents[idx].scroll_offset = 0;
        }
    }

    let bytes = key_to_bytes(code, modifiers);
    if !bytes.is_empty() {
        let _ = app.interactive_agents[idx].write_to_pty(&bytes);
    }

    Ok(())
}

// ── Dialog: new agent creation ──────────────────────────────────────
//
// Flow: Tab/Shift+Tab switch fields, ←→ choose CLI, ↑↓ navigate dirs,
//       Space enter directory, Enter launch, Esc cancel.

fn handle_dialog_key(app: &mut App, code: KeyCode) -> Result<()> {
    if app.new_agent_dialog.is_none() {
        return Ok(());
    }

    match code {
        KeyCode::Esc => app.close_new_agent_dialog(),
        KeyCode::Enter => {
            let _ = app.launch_new_agent();
        }
        KeyCode::Tab => {
            let Some(dialog) = &mut app.new_agent_dialog else {
                return Ok(());
            };
            let max_field = match dialog.task_type {
                super::app::NewTaskType::Interactive => 3, // type, CLI, dir, model
                super::app::NewTaskType::Scheduled => 5,   // + prompt, cron
                super::app::NewTaskType::Watcher => 5,     // + prompt, watch_path
            };
            dialog.field = (dialog.field + 1).min(max_field);
        }
        KeyCode::BackTab => {
            let Some(dialog) = &mut app.new_agent_dialog else {
                return Ok(());
            };
            dialog.field = dialog.field.saturating_sub(1);
        }
        _ => {
            let Some(dialog) = &mut app.new_agent_dialog else {
                return Ok(());
            };
            match dialog.field {
                // Task type selector
                0 => match code {
                    KeyCode::Left | KeyCode::Up => {
                        dialog.task_type = match dialog.task_type {
                            super::app::NewTaskType::Interactive => {
                                super::app::NewTaskType::Watcher
                            }
                            super::app::NewTaskType::Scheduled => {
                                super::app::NewTaskType::Interactive
                            }
                            super::app::NewTaskType::Watcher => super::app::NewTaskType::Scheduled,
                        };
                    }
                    KeyCode::Right | KeyCode::Down => {
                        dialog.task_type = match dialog.task_type {
                            super::app::NewTaskType::Interactive => {
                                super::app::NewTaskType::Scheduled
                            }
                            super::app::NewTaskType::Scheduled => super::app::NewTaskType::Watcher,
                            super::app::NewTaskType::Watcher => {
                                super::app::NewTaskType::Interactive
                            }
                        };
                    }
                    _ => {}
                },
                // CLI selector
                1 => match code {
                    KeyCode::Left => dialog.prev_cli(),
                    KeyCode::Right => dialog.next_cli(),
                    _ => {}
                },
                // Directory browser
                2 => match code {
                    KeyCode::Up => {
                        if dialog.dir_selected > 0 {
                            dialog.dir_selected -= 1;
                        }
                    }
                    KeyCode::Down => {
                        if dialog.dir_selected + 1 < dialog.dir_entries.len() {
                            dialog.dir_selected += 1;
                        }
                    }
                    KeyCode::Char(' ') => {
                        dialog.navigate_to_selected();
                    }
                    KeyCode::Backspace => {
                        dialog.working_dir.pop();
                    }
                    _ => {}
                },
                // Model input
                3 => match code {
                    KeyCode::Char(c) => dialog.model.push(c),
                    KeyCode::Backspace => {
                        dialog.model.pop();
                    }
                    _ => {}
                },
                // Prompt (scheduled/watcher)
                4 => match code {
                    KeyCode::Char(c) => dialog.prompt.push(c),
                    KeyCode::Backspace => {
                        dialog.prompt.pop();
                    }
                    _ => {}
                },
                // Cron expr or watch path
                5 => match dialog.task_type {
                    super::app::NewTaskType::Scheduled => match code {
                        KeyCode::Char(c) => dialog.cron_expr.push(c),
                        KeyCode::Backspace => {
                            dialog.cron_expr.pop();
                        }
                        _ => {}
                    },
                    super::app::NewTaskType::Watcher => match code {
                        KeyCode::Char(c) => dialog.watch_path.push(c),
                        KeyCode::Backspace => {
                            dialog.watch_path.pop();
                        }
                        _ => {}
                    },
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}
