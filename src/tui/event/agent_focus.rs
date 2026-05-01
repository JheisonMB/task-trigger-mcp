use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use super::context_transfer::resolve_session;
use super::search_picker::handle_suggestion_picker_key;
use super::terminal_warp::{
    handle_terminal_direct_pty_key, handle_terminal_warp_key, record_terminal_command,
};
use crate::tui::agent::key_to_bytes;
use crate::tui::app::{AgentEntry, App, Focus};

pub fn handle_agent_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // Suggestion picker intercepts keys when open (terminal autocomplete)
    if app.suggestion_picker.is_some() {
        return handle_suggestion_picker_key(app, code);
    }

    // Split picker intercepts ALL keys when open
    if app.split_picker_open {
        match code {
            KeyCode::Down => {
                let len = app.split_picker_sessions.len();
                if len > 0 {
                    app.split_picker_idx = (app.split_picker_idx + 1) % len;
                }
            }
            KeyCode::Up => {
                let len = app.split_picker_sessions.len();
                if len > 0 {
                    app.split_picker_idx = app.split_picker_idx.checked_sub(1).unwrap_or(len - 1);
                }
            }
            KeyCode::Tab => {
                app.split_picker_orientation = match app.split_picker_orientation {
                    crate::domain::models::SplitOrientation::Horizontal => {
                        crate::domain::models::SplitOrientation::Vertical
                    }
                    crate::domain::models::SplitOrientation::Vertical => {
                        crate::domain::models::SplitOrientation::Horizontal
                    }
                };
            }
            KeyCode::Enter => {
                app.create_split();
            }
            KeyCode::Esc => {
                app.split_picker_open = false;
            }
            _ => {}
        }
        return Ok(());
    }

    // Background agents (non-interactive, non-terminal, non-group): simple log-scrolling
    if !matches!(
        app.selected_agent(),
        Some(AgentEntry::Interactive(_))
            | Some(AgentEntry::Terminal(_))
            | Some(AgentEntry::Group(_))
    ) {
        match code {
            KeyCode::Esc | KeyCode::Char('h') => app.focus = Focus::Preview,
            KeyCode::Down | KeyCode::Char('j') => app.scroll_log_down(),
            KeyCode::Up | KeyCode::Char('k') => app.scroll_log_up(),
            KeyCode::Char('q') => app.running = false,
            KeyCode::F(1) => app.show_legend = !app.show_legend,
            _ => {}
        }
        return Ok(());
    }

    // Ctrl+T: open context transfer modal (Interactive and Terminal)
    if code == KeyCode::Char('t') && modifiers.contains(KeyModifiers::CONTROL) {
        if app.active_split_id.is_some() {
            // In split mode, open context transfer for the focused panel's session
            app.open_context_transfer_for_split();
        } else if matches!(
            app.selected_agent(),
            Some(AgentEntry::Interactive(_)) | Some(AgentEntry::Terminal(_))
        ) {
            app.open_context_transfer_modal();
        }
        return Ok(());
    }

    // Ctrl+S: open split picker
    if code == KeyCode::Char('s') && modifiers.contains(KeyModifiers::CONTROL) {
        app.open_split_picker();
        return Ok(());
    }

    // Ctrl+Left/Right: switch panel focus in split view
    if modifiers.contains(KeyModifiers::SHIFT) {
        match code {
            KeyCode::Left => {
                app.split_right_focused = false;
                return Ok(());
            }
            KeyCode::Right => {
                app.split_right_focused = true;
                return Ok(());
            }
            _ => {}
        }
    }

    // F10 = switch to Preview mode
    if code == KeyCode::F(10) {
        app.active_split_id = None;
        app.focus = Focus::Preview;
        return Ok(());
    }

    // F4 behavior depends on context:
    // - In split mode: dissolve split (keep sessions alive)
    // - In normal agent mode: terminate session
    if code == KeyCode::F(4) && !modifiers.contains(KeyModifiers::SHIFT) {
        if app.active_split_id.is_some() {
            // In split mode: dissolve only
            app.dissolve_split();
        } else {
            // In normal mode: terminate session
            app.terminate_focused_session();
        }
        return Ok(());
    }

    // Shift+F4 = terminate session AND dissolve split (only in split mode)
    if code == KeyCode::F(4) && modifiers.contains(KeyModifiers::SHIFT) {
        if app.active_split_id.is_some() {
            app.terminate_focused_session();
        }
        return Ok(());
    }

    // F1 = toggle legend (intercept before PTY)
    if code == KeyCode::F(1) {
        app.show_legend = !app.show_legend;
        return Ok(());
    }

    // Shift+Down = next interactive agent, Shift+Up = prev (focus mode)
    if modifiers.contains(KeyModifiers::SHIFT) {
        match code {
            KeyCode::Down => {
                app.next_interactive();
                return Ok(());
            }
            KeyCode::Up => {
                app.prev_interactive();
                return Ok(());
            }
            _ => {}
        }
    }

    // In split mode, direct input to the focused split panel's session
    let (agent_vec, idx) = if let Some(ref split_id) = app.active_split_id {
        let session_name = app
            .split_groups
            .iter()
            .find(|g| g.id == *split_id)
            .map(|g| {
                if app.split_right_focused {
                    g.session_b.clone()
                } else {
                    g.session_a.clone()
                }
            });
        match session_name {
            Some(name) => resolve_session(app, &name),
            None => {
                app.focus = Focus::Preview;
                return Ok(());
            }
        }
    } else {
        // Normal (non-split) mode: use the selected agent
        match app.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => {
                let idx = *idx;
                if idx >= app.interactive_agents.len() {
                    app.focus = Focus::Preview;
                    return Ok(());
                }
                ("interactive", idx)
            }
            Some(AgentEntry::Terminal(idx)) => {
                let idx = *idx;
                if idx >= app.terminal_agents.len() {
                    app.focus = Focus::Preview;
                    return Ok(());
                }
                ("terminal", idx)
            }
            _ => {
                app.focus = Focus::Home;
                return Ok(());
            }
        }
    };

    // Bounds check — if the resolved index is out-of-range, bail to Preview
    let in_bounds = if agent_vec == "interactive" {
        idx < app.interactive_agents.len()
    } else {
        idx < app.terminal_agents.len()
    };
    if !in_bounds {
        app.focus = Focus::Preview;
        return Ok(());
    }

    let pty_owns_navigation = if agent_vec == "interactive" {
        app.interactive_agents[idx].in_alternate_screen()
    } else {
        app.terminal_agents[idx].in_alternate_screen()
    };

    macro_rules! agent_ref {
        () => {
            if agent_vec == "interactive" {
                &app.interactive_agents[idx]
            } else {
                &app.terminal_agents[idx]
            }
        };
    }
    macro_rules! agent_mut {
        () => {
            if agent_vec == "interactive" {
                &mut app.interactive_agents[idx]
            } else {
                &mut app.terminal_agents[idx]
            }
        };
    }

    // Shift+Up/Down = always scroll (even when not already scrolled)
    if modifiers.contains(KeyModifiers::SHIFT) && !pty_owns_navigation {
        match code {
            KeyCode::Up => {
                let max = agent_ref!().max_scroll();
                agent_mut!().scroll_offset = (agent_ref!().scroll_offset + 3).min(max);
                return Ok(());
            }
            KeyCode::Down => {
                agent_mut!().scroll_offset = agent_ref!().scroll_offset.saturating_sub(3);
                return Ok(());
            }
            _ => {}
        }
    }

    // Up/Down = scroll PTY history when scrolled up, otherwise pass to PTY.
    let max_scroll = agent_ref!().max_scroll();
    let scrolled = agent_ref!().scroll_offset > 0;
    if !pty_owns_navigation {
        match code {
            KeyCode::Up if scrolled => {
                agent_mut!().scroll_offset = (agent_ref!().scroll_offset + 3).min(max_scroll);
                return Ok(());
            }
            KeyCode::Down if scrolled => {
                agent_mut!().scroll_offset = agent_ref!().scroll_offset.saturating_sub(3);
                return Ok(());
            }
            KeyCode::PageUp => {
                agent_mut!().scroll_offset = (agent_ref!().scroll_offset + 15).min(max_scroll);
                return Ok(());
            }
            KeyCode::PageDown => {
                agent_mut!().scroll_offset = agent_ref!().scroll_offset.saturating_sub(15);
                return Ok(());
            }
            _ => {}
        }
    }

    // Typing resets scroll to live view
    if agent_ref!().scroll_offset > 0 {
        let resets_scroll = matches!(
            code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Backspace | KeyCode::Tab
        );
        if resets_scroll {
            agent_mut!().scroll_offset = 0;
        }
    }

    // Record the prompt when the user presses Enter (interactive only)
    // Skip recording if a sensitive prompt (password/passphrase) is active
    if agent_vec == "interactive" {
        if code == KeyCode::Enter {
            let is_sensitive = app.interactive_agents[idx].is_sensitive_input_active();
            if let Ok(input) = app.interactive_agents[idx].input_buffer.lock() {
                let captured = input.trim().to_string();
                if !captured.is_empty() && !is_sensitive {
                    app.interactive_agents[idx].record_prompt(&captured);
                }
            }
            if let Ok(mut input) = app.interactive_agents[idx].input_buffer.lock() {
                input.clear();
            }
        } else if let KeyCode::Char(c) = code {
            if !modifiers.contains(KeyModifiers::CONTROL) {
                if let Ok(mut input) = app.interactive_agents[idx].input_buffer.lock() {
                    input.push(c);
                }
            }
        } else if code == KeyCode::Backspace {
            if let Ok(mut input) = app.interactive_agents[idx].input_buffer.lock() {
                input.pop();
            }
        }
    }

    // Terminal: track input buffer + record history on Enter
    if agent_vec == "terminal" {
        // Ctrl+W = toggle warp mode
        if code == KeyCode::Char('w') && modifiers.contains(KeyModifiers::CONTROL) {
            app.terminal_agents[idx].warp_mode = !app.terminal_agents[idx].warp_mode;
            app.terminal_agents[idx].warp_passthrough = false;
            return Ok(());
        }

        let warp = app.terminal_agents[idx].warp_mode;

        if warp {
            if app.terminal_agents[idx].should_bypass_warp_input() {
                return handle_terminal_direct_pty_key(app, idx, code, modifiers);
            }
            return handle_terminal_warp_key(app, idx, code, modifiers);
        }

        // Non-warp terminal: track input for history but forward keystrokes normally
        if code == KeyCode::Enter {
            let captured = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|buf| buf.trim().to_string())
                .unwrap_or_default();
            record_terminal_command(app, idx, &captured);
            if let Ok(mut input) = app.terminal_agents[idx].input_buffer.lock() {
                input.clear();
            }
        } else if code == KeyCode::Tab {
            // Non-warp mode: Tab goes directly to PTY (no suggestion picker)
        } else if let KeyCode::Char(c) = code {
            if !modifiers.contains(KeyModifiers::CONTROL) {
                if let Ok(mut input) = app.terminal_agents[idx].input_buffer.lock() {
                    input.push(c);
                }
            }
        } else if code == KeyCode::Backspace {
            if let Ok(mut input) = app.terminal_agents[idx].input_buffer.lock() {
                input.pop();
            }
        }
    }

    let bytes = key_to_bytes(code, modifiers);
    if !bytes.is_empty() {
        let _ = agent_mut!().write_to_pty(&bytes);
    }

    Ok(())
}
