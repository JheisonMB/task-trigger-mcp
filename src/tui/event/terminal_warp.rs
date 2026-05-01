use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::time::Duration;

use super::search_picker::resolve_cd_path;
use crate::tui::agent::key_to_bytes;
use crate::tui::app::{App, TerminalSearch};

// ── Terminal warp-mode key handling ─────────────────────────────────

/// Handle keystrokes for a terminal agent in warp mode.
/// Keys are accumulated in the input buffer and only sent to PTY on Enter.
pub fn sync_terminal_warp_buffer_from_pty(app: &mut App, idx: usize, wait_ms: u64) {
    let synced = app.terminal_agents[idx].sync_warp_input_from_pty(Duration::from_millis(wait_ms));
    if let Some(input) = synced {
        if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
            buf.clear();
            buf.push_str(&input);
        }
        app.terminal_agents[idx].warp_cursor = input.len();
        app.terminal_agents[idx].warp_passthrough = true;
    }
}

pub fn handle_terminal_direct_pty_key(
    app: &mut App,
    idx: usize,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<()> {
    let sensitive_input = app.terminal_agents[idx].is_sensitive_input_active();
    let bytes = key_to_bytes(code, modifiers);
    if !bytes.is_empty() {
        let _ = app.terminal_agents[idx].write_to_pty(&bytes);
    }

    if sensitive_input {
        if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
            buf.clear();
        }
        app.terminal_agents[idx].warp_cursor = 0;
        app.terminal_agents[idx].history_index = None;
        app.terminal_agents[idx].warp_passthrough = false;
        return Ok(());
    }

    let direct_submit = matches!(code, KeyCode::Enter)
        || (code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL))
        || (code == KeyCode::Char('d') && modifiers.contains(KeyModifiers::CONTROL));

    if !bytes.is_empty() && !direct_submit && !app.terminal_agents[idx].in_alternate_screen() {
        let wait_ms = if code == KeyCode::Tab { 90 } else { 35 };
        sync_terminal_warp_buffer_from_pty(app, idx, wait_ms);
    }

    match code {
        KeyCode::Enter => {
            let captured = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|buf| buf.trim().to_string())
                .unwrap_or_default();
            record_terminal_command(app, idx, &captured);
            if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                buf.clear();
            }
            app.terminal_agents[idx].warp_cursor = 0;
            app.terminal_agents[idx].history_index = None;
            app.terminal_agents[idx].warp_passthrough = false;
        }
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                buf.clear();
            }
            app.terminal_agents[idx].warp_cursor = 0;
            app.terminal_agents[idx].history_index = None;
            app.terminal_agents[idx].warp_passthrough = false;
        }
        _ => {
            app.terminal_agents[idx].history_index = None;
        }
    }

    Ok(())
}

pub fn handle_terminal_warp_key(
    app: &mut App,
    idx: usize,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<()> {
    if app.terminal_agents[idx].warp_passthrough {
        return handle_terminal_direct_pty_key(app, idx, code, modifiers);
    }

    let agent = &mut app.terminal_agents[idx];

    match code {
        KeyCode::Enter => {
            let captured = agent
                .input_buffer
                .lock()
                .map(|buf| buf.to_string())
                .unwrap_or_default();

            // Send entire line to PTY + newline
            if !captured.is_empty() {
                let mut bytes: Vec<u8> = captured.as_bytes().to_vec();
                bytes.push(b'\n');
                let _ = agent.write_to_pty(&bytes);
            } else {
                let _ = agent.write_to_pty(b"\n");
            }

            // Record in history
            let captured_trimmed = captured.trim().to_string();
            record_terminal_command(app, idx, &captured_trimmed);

            if let Ok(mut input) = app.terminal_agents[idx].input_buffer.lock() {
                input.clear();
            }
            app.terminal_agents[idx].warp_cursor = 0;
            app.terminal_agents[idx].history_index = None;
            app.terminal_agents[idx].warp_passthrough = false;
        }
        KeyCode::Tab => {
            let input_text = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|b| b.trim().to_string())
                .unwrap_or_default();
            let is_cd = input_text.is_empty()
                || input_text == "cd"
                || input_text.starts_with("cd ")
                || input_text.starts_with("cd\t");
            if is_cd {
                return open_terminal_suggestion_picker(app, idx);
            }
            // Non-cd: send current input + Tab to PTY for native autocomplete.
            let text = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|b| b.clone())
                .unwrap_or_default();
            let _ = app.terminal_agents[idx].write_to_pty(text.as_bytes());
            let _ = app.terminal_agents[idx].write_to_pty(b"\t");
            sync_terminal_warp_buffer_from_pty(app, idx, 90);
            return Ok(());
        }
        KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
            let cursor = app.terminal_agents[idx].warp_cursor;
            if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                let pos = cursor.min(buf.len());
                buf.insert(pos, c);
            }
            app.terminal_agents[idx].warp_cursor = cursor + c.len_utf8();
            app.terminal_agents[idx].history_index = None;
        }
        KeyCode::Backspace => {
            let cursor = app.terminal_agents[idx].warp_cursor;
            if cursor > 0 {
                let new_cursor = app.terminal_agents[idx]
                    .input_buffer
                    .lock()
                    .map(|mut buf| {
                        let clamped = cursor.min(buf.len());
                        let prev = buf[..clamped]
                            .char_indices()
                            .last()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        buf.remove(prev);
                        prev
                    })
                    .unwrap_or(0);
                app.terminal_agents[idx].warp_cursor = new_cursor;
            }
        }
        KeyCode::Delete => {
            let cursor = app.terminal_agents[idx].warp_cursor;
            if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                if cursor < buf.len() {
                    buf.remove(cursor);
                }
            }
        }
        KeyCode::Left => {
            let cursor = app.terminal_agents[idx].warp_cursor;
            if cursor > 0 {
                let new_pos = app.terminal_agents[idx]
                    .input_buffer
                    .lock()
                    .map(|buf| {
                        buf[..cursor]
                            .char_indices()
                            .last()
                            .map(|(i, _)| i)
                            .unwrap_or(0)
                    })
                    .unwrap_or(0);
                app.terminal_agents[idx].warp_cursor = new_pos;
            }
        }
        KeyCode::Right => {
            let cursor = app.terminal_agents[idx].warp_cursor;
            let new_pos = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|buf| {
                    if cursor < buf.len() {
                        buf[cursor..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| cursor + i)
                            .unwrap_or(buf.len())
                    } else {
                        cursor
                    }
                })
                .unwrap_or(cursor);
            app.terminal_agents[idx].warp_cursor = new_pos;
        }
        KeyCode::Home => {
            app.terminal_agents[idx].warp_cursor = 0;
        }
        KeyCode::End => {
            let len = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|buf| buf.len())
                .unwrap_or(0);
            app.terminal_agents[idx].warp_cursor = len;
        }
        KeyCode::Up => {
            let already_browsing = app.terminal_agents[idx].history_index.is_some();
            let input_empty = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|b| b.trim().is_empty())
                .unwrap_or(true);
            if already_browsing || input_empty {
                // Browse session history
                let session_name = app.terminal_agents[idx].name.clone();
                let hist = app.terminal_histories.get(&session_name);
                let hist_len = hist.map(|h| h.commands.len()).unwrap_or(0);
                if hist_len > 0 {
                    let new_idx = match app.terminal_agents[idx].history_index {
                        None => hist_len - 1,
                        Some(i) => i.saturating_sub(1),
                    };
                    app.terminal_agents[idx].history_index = Some(new_idx);
                    if let Some(entry) = app
                        .terminal_histories
                        .get(&session_name)
                        .and_then(|h| h.commands.get(new_idx))
                    {
                        let cmd = entry.cmd.clone();
                        if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                            buf.clear();
                            buf.push_str(&cmd);
                        }
                        app.terminal_agents[idx].warp_cursor = cmd.len();
                    }
                }
            } else {
                // Scroll up through terminal scrollback
                let max = app.terminal_agents[idx].max_scroll();
                app.terminal_agents[idx].scroll_offset =
                    (app.terminal_agents[idx].scroll_offset + 3).min(max);
            }
        }
        KeyCode::Down => {
            let already_browsing = app.terminal_agents[idx].history_index.is_some();
            let input_empty = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|b| b.trim().is_empty())
                .unwrap_or(true);
            if already_browsing || (input_empty && app.terminal_agents[idx].history_index.is_some())
            {
                // Browse session history forward
                let session_name = app.terminal_agents[idx].name.clone();
                let hist_len = app
                    .terminal_histories
                    .get(&session_name)
                    .map(|h| h.commands.len())
                    .unwrap_or(0);
                let cur = app.terminal_agents[idx].history_index.unwrap_or(0);
                if cur + 1 < hist_len {
                    app.terminal_agents[idx].history_index = Some(cur + 1);
                    if let Some(entry) = app
                        .terminal_histories
                        .get(&session_name)
                        .and_then(|h| h.commands.get(cur + 1))
                    {
                        let cmd = entry.cmd.clone();
                        if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                            buf.clear();
                            buf.push_str(&cmd);
                        }
                        app.terminal_agents[idx].warp_cursor = cmd.len();
                    }
                } else {
                    // Past the end — clear input and reset history browsing
                    app.terminal_agents[idx].history_index = None;
                    if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                        buf.clear();
                    }
                    app.terminal_agents[idx].warp_cursor = 0;
                }
            } else {
                // Scroll down (towards live view)
                app.terminal_agents[idx].scroll_offset =
                    app.terminal_agents[idx].scroll_offset.saturating_sub(3);
            }
        }
        KeyCode::PageUp => {
            let max = app.terminal_agents[idx].max_scroll();
            app.terminal_agents[idx].scroll_offset =
                (app.terminal_agents[idx].scroll_offset + 15).min(max);
        }
        KeyCode::PageDown => {
            app.terminal_agents[idx].scroll_offset =
                app.terminal_agents[idx].scroll_offset.saturating_sub(15);
        }
        // Ctrl+F — search in scrollback
        KeyCode::Char('f') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.terminal_search = Some(TerminalSearch::new(idx));
        }
        // Ctrl+C — send SIGINT to PTY and clear input
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = app.terminal_agents[idx].write_to_pty(&[0x03]); // ETX
            if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                buf.clear();
            }
            app.terminal_agents[idx].warp_cursor = 0;
            app.terminal_agents[idx].history_index = None;
            app.terminal_agents[idx].warp_passthrough = false;
        }
        // Ctrl+D — send EOF
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = app.terminal_agents[idx].write_to_pty(&[0x04]); // EOT
            app.terminal_agents[idx].warp_passthrough = false;
        }
        // Ctrl+L — clear screen
        KeyCode::Char('l') if modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = app.terminal_agents[idx].write_to_pty(&[0x0c]); // FF
        }
        // Ctrl+U — clear input before cursor
        KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                let cursor = app.terminal_agents[idx].warp_cursor.min(buf.len());
                buf.drain(..cursor);
            }
            app.terminal_agents[idx].warp_cursor = 0;
        }
        // Ctrl+K — clear input after cursor
        KeyCode::Char('k') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                let cursor = app.terminal_agents[idx].warp_cursor.min(buf.len());
                buf.truncate(cursor);
            }
        }
        // Ctrl+A — go to start
        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
            app.terminal_agents[idx].warp_cursor = 0;
        }
        // Ctrl+E — go to end
        KeyCode::Char('e') if modifiers.contains(KeyModifiers::CONTROL) => {
            let len = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|buf| buf.len())
                .unwrap_or(0);
            app.terminal_agents[idx].warp_cursor = len;
        }
        _ => {}
    }
    Ok(())
}

/// Record a terminal command to the session history.
pub fn record_terminal_command(app: &mut App, idx: usize, captured: &str) {
    if captured.is_empty() {
        return;
    }
    let trimmed = captured.trim();

    // Handle cd commands: update working dir but don't record to history
    if trimmed == "cd" || trimmed.starts_with("cd ") || trimmed.starts_with("cd\t") {
        let target = if trimmed == "cd" {
            // Plain "cd" without args usually goes to home directory
            dirs::home_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string())
        } else {
            let after_cd = &trimmed[2..].trim();
            after_cd.to_string()
        };
        let current_dir = app.terminal_agents[idx].working_dir.clone();
        if let Some(abs_path) = resolve_cd_path(&current_dir, &target) {
            app.terminal_agents[idx].update_working_dir(&abs_path.to_string_lossy());
        }
        return;
    }

    let session_name = app.terminal_agents[idx].name.clone();
    let cwd = app.terminal_agents[idx].working_dir.clone();
    // Per-session history
    let hist = app
        .terminal_histories
        .entry(session_name.clone())
        .or_default();
    hist.record(captured, &cwd);
    crate::tui::terminal_history::save_history(&app.data_dir, &session_name, hist);
    // Global catalog (idempotent, excludes cd)
    crate::tui::terminal_history::record_global_catalog(&app.data_dir, captured, &cwd);
}

/// Open the suggestion picker for a terminal agent.
pub fn open_terminal_suggestion_picker(app: &mut App, idx: usize) -> Result<()> {
    let input_text = app.terminal_agents[idx]
        .input_buffer
        .lock()
        .map(|buf| buf.to_string())
        .unwrap_or_default();
    let cwd = app.terminal_agents[idx].working_dir.clone();

    // Detect "cd" prefix: "cd", "cd ", "cd foo"
    let is_cd =
        input_text == "cd" || input_text.starts_with("cd ") || input_text.starts_with("cd\t");

    if is_cd {
        let partial = if input_text.len() > 2 {
            input_text[3..].trim()
        } else {
            ""
        };
        // cd picker uses global history for known directories
        let global = crate::tui::terminal_history::load_all_histories(&app.data_dir);
        app.suggestion_picker = Some(crate::tui::terminal_history::SuggestionPicker::for_cd(
            partial, &cwd, &global,
        ));
    } else {
        // Command history uses session-only history (per-session counts)
        // Tab: global command catalog (all terminals contribute)
        app.suggestion_picker = Some(crate::tui::terminal_history::from_global_catalog(
            &input_text,
            &app.data_dir,
            &cwd,
        ));
    }
    Ok(())
}

// ── Dialog: new agent creation ──────────────────────────────────────
