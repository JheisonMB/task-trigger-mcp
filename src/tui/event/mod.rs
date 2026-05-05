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
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::time::Duration;

use crate::tui::app::types::{AgentEntry, App, Focus};
use crate::tui::ui;

use agent_focus::handle_agent_key;
use context_transfer::handle_context_transfer_key;
use home_preview::{handle_home_key, handle_preview_key};
use new_agent_dialog::handle_dialog_key;
use paste::handle_paste;
use prompt_template::handle_prompt_template_key;
use rag_transfer::handle_rag_transfer_key;

type Terminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>;

/// Main event loop: draw → poll events → refresh data.
pub fn run_event_loop(terminal: &mut Terminal, app: &mut App) -> Result<()> {
    while app.running {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Tick speed adapts to what needs frequent repaints.
        // All interactive states use 50ms for responsive PTY rendering.
        // Home without brain uses 200ms (nothing animating).
        let tick = match app.focus {
            Focus::Agent
            | Focus::NewAgentDialog
            | Focus::ContextTransfer
            | Focus::RagTransfer
            | Focus::PromptTemplateDialog => Duration::from_millis(50),
            Focus::Preview => Duration::from_millis(100),
            Focus::Home if app.home_brain.is_some() => Duration::from_millis(50),
            Focus::Home => Duration::from_millis(200),
        };

        if event::poll(tick)? {
            loop {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        handle_key(app, key.code, key.modifiers)?;
                    }
                    Event::Mouse(mouse) => {
                        app.notify_mouse_move();
                        handle_mouse(app, mouse)?;
                    }
                    Event::Resize(_, _) => {
                        // Resize is handled by refresh() on next tick
                    }
                    Event::Paste(text) => {
                        handle_paste(app, &text);
                    }
                    _ => {}
                }
                if !event::poll(Duration::from_millis(0))? {
                    break;
                }
            }
        }

        app.refresh()?;
    }

    app.cleanup();
    Ok(())
}

// ── Prompt Template Dialog ──────────────────────────────────────

mod agent_focus;
mod context_transfer;
mod home_preview;
mod new_agent_dialog;
mod paste;
mod prompt_template;
mod rag_transfer;
mod search_picker;
mod terminal_warp;

pub fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // Legend overlay intercepts ALL keys — closes on any key
    if app.show_legend {
        app.show_legend = false;
        return Ok(());
    }

    // Ctrl+N: new agent from any mode (works in Focus too)
    if code == KeyCode::Char('n') && modifiers.contains(KeyModifiers::CONTROL) {
        app.open_new_agent_dialog();
        return Ok(());
    }

    // Ctrl+B: open prompt builder dialog from focus mode
    if code == KeyCode::Char('b')
        && modifiers.contains(KeyModifiers::CONTROL)
        && matches!(app.focus, Focus::Agent)
    {
        app.open_simple_prompt_dialog(None);
        return Ok(());
    }

    if code == KeyCode::F(2) {
        app.toggle_sidebar_mode();
        // When in agent focus, step back to Preview so sidebar navigation
        // keys (Shift+arrows, Up/Down) go to the correct handler.
        if matches!(app.focus, Focus::Agent) {
            app.focus = Focus::Preview;
        }
        return Ok(());
    }

    if code == KeyCode::F(3) {
        app.toggle_sync_panel();
        return Ok(());
    }

    // Ctrl+F: search in scrollback
    if code == KeyCode::Char('f')
        && modifiers.contains(KeyModifiers::CONTROL)
        && matches!(app.focus, Focus::Agent)
    {
        open_terminal_search(app);
        return Ok(());
    }

    // Handle active terminal search overlay
    if app.terminal_search.is_some() {
        return handle_terminal_search_key(app, code);
    }

    match app.focus {
        Focus::Home => handle_home_key(app, code, modifiers),
        Focus::Preview => handle_preview_key(app, code, modifiers),
        Focus::NewAgentDialog => handle_dialog_key(app, code),
        Focus::Agent => handle_agent_key(app, code, modifiers),
        Focus::ContextTransfer => handle_context_transfer_key(app, code),
        Focus::RagTransfer => handle_rag_transfer_key(app, code),
        Focus::PromptTemplateDialog => handle_prompt_template_key(app, code, modifiers),
    }
}

// ── Mouse: scroll wheel + Shift+Click to copy selection ─────────────

fn handle_mouse(app: &mut App, mouse: MouseEvent) -> Result<()> {
    let kind = mouse.kind;
    let modifiers = mouse.modifiers;

    if try_forward_mouse_to_pty(app, &mouse) {
        return Ok(());
    }

    if matches!(kind, MouseEventKind::Up(MouseButton::Left)) {
        if modifiers.contains(KeyModifiers::SHIFT) {
            handle_shift_click_copy(app);
        } else {
            handle_left_click_copy(app, &mouse);
        }
        return Ok(());
    }

    let dir = match kind {
        MouseEventKind::ScrollUp => 1i32,
        MouseEventKind::ScrollDown => -1i32,
        _ => return Ok(()),
    };

    // If the cursor is over the sync panel, scroll it independently
    if let Some(sync_area) = app.last_sync_area {
        if mouse.column >= sync_area.x
            && mouse.column < sync_area.x + sync_area.width
            && mouse.row >= sync_area.y
            && mouse.row < sync_area.y + sync_area.height
        {
            if dir > 0 {
                app.sync_scroll_offset = app.sync_scroll_offset.saturating_sub(3);
            } else {
                app.sync_scroll_offset = app.sync_scroll_offset.saturating_add(3);
            }
            return Ok(());
        }
    }

    handle_scroll(app, dir);
    Ok(())
}

/// Try to forward the mouse event to the focused PTY agent.
/// Returns `true` if the event was consumed.
fn try_forward_mouse_to_pty(app: &mut App, mouse: &MouseEvent) -> bool {
    let sidebar_width = if app.sidebar_visible { 30 } else { 0 };
    let header_height = 1u16;
    let col = mouse.column;
    let row = mouse.row;

    let clicked_in_pty = col >= sidebar_width
        && col < sidebar_width + app.last_panel_inner.0
        && row >= header_height
        && row < header_height + app.last_panel_inner.1;

    if !clicked_in_pty {
        return false;
    }

    let pty_col = col.saturating_sub(sidebar_width);
    let pty_row = row.saturating_sub(header_height);

    match app.selected_agent() {
        Some(AgentEntry::Interactive(idx)) => {
            let idx = *idx;
            app.interactive_agents
                .get(idx)
                .and_then(|a| {
                    a.forward_mouse(mouse.kind, MouseButton::Left, pty_col, pty_row)
                        .ok()
                })
                .unwrap_or(false)
        }
        Some(AgentEntry::Terminal(idx)) => {
            let idx = *idx;
            app.terminal_agents
                .get(idx)
                .and_then(|a| {
                    a.forward_mouse(mouse.kind, MouseButton::Left, pty_col, pty_row)
                        .ok()
                })
                .unwrap_or(false)
        }
        _ => false,
    }
}

fn handle_left_click_copy(app: &mut App, mouse: &MouseEvent) {
    let sidebar_width = if app.sidebar_visible { 30 } else { 0 };
    let header_height = 1u16;

    if mouse.column < sidebar_width
        || mouse.row < header_height
        || mouse.column >= sidebar_width + app.last_panel_inner.0
        || mouse.row >= header_height + app.last_panel_inner.1
    {
        return;
    }

    let pty_col = mouse.column.saturating_sub(sidebar_width);
    let pty_row = mouse.row.saturating_sub(header_height);

    let line_text = match app.selected_agent() {
        Some(AgentEntry::Interactive(idx)) => app
            .interactive_agents
            .get(*idx)
            .and_then(|a| a.get_clean_pty_line_at_position(pty_col, pty_row)),
        Some(AgentEntry::Terminal(idx)) => app
            .terminal_agents
            .get(*idx)
            .and_then(|a| a.get_clean_pty_line_at_position(pty_col, pty_row)),
        _ => None,
    };

    if let Some(text) = line_text {
        std::thread::spawn(move || {
            let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(&text));
        });
        app.show_copied = true;
        app.copied_at = std::time::Instant::now();
    }
}

fn handle_shift_click_copy(app: &mut App) {
    app.show_copied = true;
    app.copied_at = std::time::Instant::now();

    let plain_text = match app.selected_agent() {
        Some(AgentEntry::Interactive(idx)) => app
            .interactive_agents
            .get(*idx)
            .and_then(|a| a.get_plain_text_from_screen()),
        Some(AgentEntry::Terminal(idx)) => app
            .terminal_agents
            .get(*idx)
            .and_then(|a| a.get_plain_text_from_screen()),
        _ => None,
    };

    if let Some(text) = plain_text {
        let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(&text));
    }
}

fn scroll_speed(app: &App) -> usize {
    let elapsed_ms = app.last_scroll_at.elapsed().as_millis();
    match elapsed_ms {
        0..=60 => 8,
        61..=120 => 4,
        121..=200 => 2,
        _ => 1,
    }
}

fn open_terminal_search(app: &mut App) {
    match app.selected_agent() {
        Some(AgentEntry::Interactive(idx)) => {
            app.terminal_search = Some(crate::tui::app::TerminalSearch::new_interactive(*idx));
        }
        Some(AgentEntry::Terminal(idx)) => {
            app.terminal_search = Some(crate::tui::app::TerminalSearch::new(*idx));
        }
        _ => {}
    }
}

fn handle_scroll(app: &mut App, dir: i32) {
    match app.focus {
        Focus::Agent | Focus::Preview => {
            let speed = scroll_speed(app);
            app.last_scroll_at = std::time::Instant::now();
            scroll_focused_agent(app, dir * speed as i32);
        }
        Focus::Home => {
            if dir > 0 {
                app.select_prev();
            } else {
                app.select_next();
            }
        }
        Focus::NewAgentDialog => {
            if let Some(dialog) = &mut app.new_agent_dialog {
                let len = dialog.filtered_dir_entries().len();
                if dir > 0 && dialog.dir_selected > 0 {
                    dialog.dir_selected -= 1;
                } else if dir < 0 && dialog.dir_selected + 1 < len {
                    dialog.dir_selected += 1;
                }
            }
        }
        Focus::ContextTransfer | Focus::RagTransfer | Focus::PromptTemplateDialog => {}
    }
}

fn scroll_focused_agent(app: &mut App, dir: i32) {
    let step = dir.unsigned_abs() as usize;
    match app.selected_agent() {
        Some(AgentEntry::Interactive(idx)) => {
            let idx = *idx;
            if idx < app.interactive_agents.len() {
                let agent = &mut app.interactive_agents[idx];
                if agent.in_alternate_screen() {
                    let _ = agent.forward_scroll(dir > 0);
                } else if dir > 0 {
                    let max = agent.max_scroll();
                    agent.scroll_offset = (agent.scroll_offset + step).min(max);
                } else {
                    agent.scroll_offset = agent.scroll_offset.saturating_sub(step);
                }
            }
        }
        Some(AgentEntry::Terminal(idx)) => {
            let idx = *idx;
            if idx < app.terminal_agents.len() {
                let agent = &mut app.terminal_agents[idx];
                if agent.in_alternate_screen() {
                    let _ = agent.forward_scroll(dir > 0);
                } else if dir > 0 {
                    let max = agent.max_scroll();
                    agent.scroll_offset = (agent.scroll_offset + step).min(max);
                } else {
                    agent.scroll_offset = agent.scroll_offset.saturating_sub(step);
                }
            }
        }
        _ => {
            for _ in 0..step {
                if dir > 0 {
                    app.scroll_log_up();
                } else {
                    app.scroll_log_down();
                }
            }
        }
    }
}

// ── Terminal scrollback search (Ctrl+F) ─────────────────────────────

fn handle_terminal_search_key(app: &mut App, code: KeyCode) -> Result<()> {
    let Some(search) = &mut app.terminal_search else {
        return Ok(());
    };

    match code {
        KeyCode::Esc => {
            app.terminal_search = None;
        }
        KeyCode::Enter => {
            let (is_terminal, idx) = (search.is_terminal, search.agent_idx);
            let agent = if is_terminal {
                &mut app.terminal_agents[idx]
            } else {
                &mut app.interactive_agents[idx]
            };
            let s = app.terminal_search.as_mut().unwrap();
            s.jump_to_match(agent);
            s.next_match();
        }
        KeyCode::Up => {
            let s = app.terminal_search.as_mut().unwrap();
            s.prev_match();
            let (is_terminal, idx) = (s.is_terminal, s.agent_idx);
            let agent = if is_terminal {
                &mut app.terminal_agents[idx]
            } else {
                &mut app.interactive_agents[idx]
            };
            app.terminal_search.as_mut().unwrap().jump_to_match(agent);
        }
        KeyCode::Down => {
            let s = app.terminal_search.as_mut().unwrap();
            s.next_match();
            let (is_terminal, idx) = (s.is_terminal, s.agent_idx);
            let agent = if is_terminal {
                &mut app.terminal_agents[idx]
            } else {
                &mut app.interactive_agents[idx]
            };
            app.terminal_search.as_mut().unwrap().jump_to_match(agent);
        }
        KeyCode::Char(c) => {
            let s = app.terminal_search.as_mut().unwrap();
            s.query.push(c);
            let (is_terminal, idx) = (s.is_terminal, s.agent_idx);
            let agent = if is_terminal {
                &app.terminal_agents[idx]
            } else {
                &app.interactive_agents[idx]
            };
            app.terminal_search.as_mut().unwrap().search(agent);
            if !app.terminal_search.as_ref().unwrap().match_rows.is_empty() {
                app.terminal_search.as_mut().unwrap().current_match = 0;
                let agent = if is_terminal {
                    &mut app.terminal_agents[idx]
                } else {
                    &mut app.interactive_agents[idx]
                };
                app.terminal_search.as_mut().unwrap().jump_to_match(agent);
            }
        }
        KeyCode::Backspace => {
            let s = app.terminal_search.as_mut().unwrap();
            s.query.pop();
            let (is_terminal, idx) = (s.is_terminal, s.agent_idx);
            let agent = if is_terminal {
                &app.terminal_agents[idx]
            } else {
                &app.interactive_agents[idx]
            };
            app.terminal_search.as_mut().unwrap().search(agent);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::search_picker::resolve_cd_picker_selection;
    use crate::tui::terminal_history::{PickerMode, SuggestionItem, SuggestionPicker};
    use std::path::PathBuf;

    #[test]
    fn test_cd_picker_selection_keeps_downstream_path() {
        let picker = SuggestionPicker {
            input: "./alpha".to_string(),
            mode: PickerMode::CdDirectory,
            all_items: vec![SuggestionItem {
                text: "./beta".to_string(),
                label: "./beta".to_string(),
                count: 0,
            }],
            items: vec![SuggestionItem {
                text: "./beta".to_string(),
                label: "./beta".to_string(),
                count: 0,
            }],
            selected: 0,
            scroll_offset: 0,
            cd_base_dir: Some(PathBuf::from("/repo")),
            cd_current_dir: Some(PathBuf::from("/repo/alpha")),
        };

        let resolved = resolve_cd_picker_selection(&picker).unwrap();
        assert_eq!(resolved, "alpha/beta");
    }

    #[test]
    fn test_cd_picker_selection_keeps_parent_path_relative_to_base() {
        let picker = SuggestionPicker {
            input: "./alpha/beta".to_string(),
            mode: PickerMode::CdDirectory,
            all_items: vec![SuggestionItem {
                text: "..".to_string(),
                label: "../".to_string(),
                count: 0,
            }],
            items: vec![SuggestionItem {
                text: "..".to_string(),
                label: "../".to_string(),
                count: 0,
            }],
            selected: 0,
            scroll_offset: 0,
            cd_base_dir: Some(PathBuf::from("/repo")),
            cd_current_dir: Some(PathBuf::from("/repo/alpha/beta")),
        };

        let resolved = resolve_cd_picker_selection(&picker).unwrap();
        assert_eq!(resolved, "alpha");
    }
}
