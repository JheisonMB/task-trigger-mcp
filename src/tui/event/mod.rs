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
        return Ok(());
    }

    if code == KeyCode::F(3) {
        app.toggle_sync_panel();
        return Ok(());
    }

    // Ctrl+F: search in scrollback (interactive or terminal agents)
    if code == KeyCode::Char('f')
        && modifiers.contains(KeyModifiers::CONTROL)
        && matches!(app.focus, Focus::Agent)
    {
        match app.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => {
                app.terminal_search = Some(crate::tui::app::TerminalSearch::new_interactive(*idx));
            }
            Some(AgentEntry::Terminal(idx)) => {
                app.terminal_search = Some(crate::tui::app::TerminalSearch::new(*idx));
            }
            _ => {}
        }
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
        Focus::PromptTemplateDialog => handle_prompt_template_key(app, code, modifiers),
    }
}

// ── Mouse: scroll wheel + Shift+Click to copy selection ─────────────

fn handle_mouse(app: &mut App, mouse: MouseEvent) -> Result<()> {
    let kind = mouse.kind;
    let modifiers = mouse.modifiers;
    let col = mouse.column;
    let row = mouse.row;

    // Try to forward mouse events to PTY when mouse protocol is active.
    // This enables TUI apps (opencode, vim, htop, etc.) to receive mouse events.
    let forwarded = if let Some(AgentEntry::Interactive(idx)) = app.selected_agent() {
        let idx = *idx;
        if let Some(agent) = app.interactive_agents.get(idx) {
            let sidebar_width = if app.sidebar_visible { 30 } else { 0 };
            let header_height = 1;
            let clicked_in_pty = col >= sidebar_width
                && col < sidebar_width + app.last_panel_inner.0
                && row >= header_height
                && row < header_height + app.last_panel_inner.1;

            if clicked_in_pty {
                let pty_col = col.saturating_sub(sidebar_width);
                let pty_row = row.saturating_sub(header_height);
                agent
                    .forward_mouse(kind, MouseButton::Left, pty_col, pty_row)
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        }
    } else if let Some(AgentEntry::Terminal(idx)) = app.selected_agent() {
        let idx = *idx;
        if let Some(agent) = app.terminal_agents.get(idx) {
            let sidebar_width = if app.sidebar_visible { 30 } else { 0 };
            let header_height = 1;
            let clicked_in_pty = col >= sidebar_width
                && col < sidebar_width + app.last_panel_inner.0
                && row >= header_height
                && row < header_height + app.last_panel_inner.1;

            if clicked_in_pty {
                let pty_col = col.saturating_sub(sidebar_width);
                let pty_row = row.saturating_sub(header_height);
                agent
                    .forward_mouse(kind, MouseButton::Left, pty_col, pty_row)
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };

    if forwarded {
        return Ok(());
    }

    // Normal Left click (no Shift) — copy line from PTY at click position
    if matches!(kind, MouseEventKind::Up(MouseButton::Left))
        && !modifiers.contains(KeyModifiers::SHIFT)
    {
        let sidebar_width = if app.sidebar_visible { 30 } else { 0 };
        let header_height = 1;
        let clicked_in_sidebar = mouse.column < sidebar_width;
        let clicked_in_header = mouse.row < header_height;
        let clicked_outside_pty = mouse.column >= sidebar_width + app.last_panel_inner.0
            || mouse.row >= header_height + app.last_panel_inner.1;

        if clicked_in_sidebar || clicked_in_header || clicked_outside_pty {
            return Ok(());
        }

        let pty_col = mouse.column.saturating_sub(sidebar_width);
        let pty_row = mouse.row.saturating_sub(header_height);

        let line_text = match app.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => app
                .interactive_agents
                .get(*idx)
                .and_then(|agent| agent.get_clean_pty_line_at_position(pty_col, pty_row)),
            Some(AgentEntry::Terminal(idx)) => app
                .terminal_agents
                .get(*idx)
                .and_then(|agent| agent.get_clean_pty_line_at_position(pty_col, pty_row)),
            _ => None,
        };

        if let Some(line_text) = line_text {
            std::thread::spawn(move || {
                let _ = arboard::Clipboard::new()
                    .and_then(|mut clipboard| clipboard.set_text(&line_text));
            });
            app.show_copied = true;
            app.copied_at = std::time::Instant::now();
        }
        return Ok(());
    }

    // Shift+Left release — copy both formatted and plain text
    if matches!(kind, MouseEventKind::Up(MouseButton::Left))
        && modifiers.contains(KeyModifiers::SHIFT)
    {
        app.show_copied = true;
        app.copied_at = std::time::Instant::now();

        // Also copy plain text to clipboard for better external paste
        let plain_text = match app.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => app
                .interactive_agents
                .get(*idx)
                .and_then(|agent| agent.get_plain_text_from_screen()),
            Some(AgentEntry::Terminal(idx)) => app
                .terminal_agents
                .get(*idx)
                .and_then(|agent| agent.get_plain_text_from_screen()),
            _ => None,
        };

        if let Some(plain_text) = plain_text {
            let _ =
                arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(&plain_text));
        }

        return Ok(());
    }

    let dir = match kind {
        MouseEventKind::ScrollUp => 1i32,
        MouseEventKind::ScrollDown => -1i32,
        _ => return Ok(()),
    };

    match app.focus {
        Focus::Agent => {
            app.last_scroll_at = std::time::Instant::now();
            if let Some(AgentEntry::Interactive(idx)) = app.selected_agent() {
                let idx = *idx;
                let agent = &mut app.interactive_agents[idx];
                if agent.in_alternate_screen() {
                    let _ = agent.forward_scroll(dir > 0);
                } else {
                    if dir > 0 {
                        let max = agent.max_scroll();
                        agent.scroll_offset = (agent.scroll_offset + 1).min(max);
                    } else {
                        agent.scroll_offset = agent.scroll_offset.saturating_sub(1);
                    }
                }
            } else if let Some(AgentEntry::Terminal(idx)) = app.selected_agent() {
                let idx = *idx;
                if idx < app.terminal_agents.len() {
                    let agent = &mut app.terminal_agents[idx];
                    if agent.in_alternate_screen() {
                        let _ = agent.forward_scroll(dir > 0);
                    } else if dir > 0 {
                        let max = agent.max_scroll();
                        agent.scroll_offset = (agent.scroll_offset + 1).min(max);
                    } else {
                        agent.scroll_offset = agent.scroll_offset.saturating_sub(1);
                    }
                }
            } else if dir > 0 {
                app.scroll_log_up();
            } else {
                app.scroll_log_down();
            }
        }
        Focus::Preview => {
            app.last_scroll_at = std::time::Instant::now();
            if let Some(AgentEntry::Interactive(idx)) = app.selected_agent() {
                let idx = *idx;
                if idx < app.interactive_agents.len() {
                    let agent = &mut app.interactive_agents[idx];
                    if agent.in_alternate_screen() {
                        let _ = agent.forward_scroll(dir > 0);
                    } else if dir > 0 {
                        let max = agent.max_scroll();
                        agent.scroll_offset = (agent.scroll_offset + 1).min(max);
                    } else {
                        agent.scroll_offset = agent.scroll_offset.saturating_sub(1);
                    }
                }
            } else if let Some(AgentEntry::Terminal(idx)) = app.selected_agent() {
                let idx = *idx;
                if idx < app.terminal_agents.len() {
                    let agent = &mut app.terminal_agents[idx];
                    if agent.in_alternate_screen() {
                        let _ = agent.forward_scroll(dir > 0);
                    } else if dir > 0 {
                        let max = agent.max_scroll();
                        agent.scroll_offset = (agent.scroll_offset + 1).min(max);
                    } else {
                        agent.scroll_offset = agent.scroll_offset.saturating_sub(1);
                    }
                }
            } else if dir > 0 {
                app.scroll_log_up();
            } else {
                app.scroll_log_down();
            }
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
                let filtered_len = dialog.filtered_dir_entries().len();
                if dir > 0 && dialog.dir_selected > 0 {
                    dialog.dir_selected -= 1;
                } else if dir < 0 && dialog.dir_selected + 1 < filtered_len {
                    dialog.dir_selected += 1;
                }
            }
        }
        Focus::ContextTransfer => {}
        Focus::PromptTemplateDialog => {}
    }
    Ok(())
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
            // Jump to current match and cycle to next
            let is_terminal = search.is_terminal;
            let idx = search.agent_idx;
            let agent = if is_terminal {
                &mut app.terminal_agents[idx]
            } else {
                &mut app.interactive_agents[idx]
            };
            search.jump_to_match(agent);
            search.next_match();
        }
        KeyCode::Up => {
            if let Some(s) = &mut app.terminal_search {
                s.prev_match();
                let is_terminal = s.is_terminal;
                let idx = s.agent_idx;
                let agent = if is_terminal {
                    &mut app.terminal_agents[idx]
                } else {
                    &mut app.interactive_agents[idx]
                };
                s.jump_to_match(agent);
            }
        }
        KeyCode::Down => {
            if let Some(s) = &mut app.terminal_search {
                s.next_match();
                let is_terminal = s.is_terminal;
                let idx = s.agent_idx;
                let agent = if is_terminal {
                    &mut app.terminal_agents[idx]
                } else {
                    &mut app.interactive_agents[idx]
                };
                s.jump_to_match(agent);
            }
        }
        KeyCode::Char(c) => {
            if let Some(s) = &mut app.terminal_search {
                s.query.push(c);
                let is_terminal = s.is_terminal;
                let idx = s.agent_idx;
                let agent = if is_terminal {
                    &app.terminal_agents[idx]
                } else {
                    &app.interactive_agents[idx]
                };
                s.search(agent);
                // Auto-jump to first match
                if !s.match_rows.is_empty() {
                    s.current_match = 0;
                    let agent = if is_terminal {
                        &mut app.terminal_agents[idx]
                    } else {
                        &mut app.interactive_agents[idx]
                    };
                    s.jump_to_match(agent);
                }
            }
        }
        KeyCode::Backspace => {
            if let Some(s) = &mut app.terminal_search {
                s.query.pop();
                let is_terminal = s.is_terminal;
                let idx = s.agent_idx;
                let agent = if is_terminal {
                    &app.terminal_agents[idx]
                } else {
                    &app.interactive_agents[idx]
                };
                s.search(agent);
            }
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
