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
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
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
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Press {
                        handle_key(app, key.code, key.modifiers)?;
                    }
                }
                Event::Mouse(mouse) => {
                    handle_mouse(app, mouse.kind, mouse.modifiers)?;
                }
                Event::Resize(_, _) => {
                    // Resize is handled by refresh() on next tick
                }
                _ => {}
            }
        }

        app.refresh()?;
    }

    app.cleanup();
    Ok(())
}

fn handle_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
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

    match app.focus {
        Focus::Home => handle_home_key(app, code, modifiers),
        Focus::Preview => handle_preview_key(app, code, modifiers),
        Focus::NewAgentDialog => handle_dialog_key(app, code),
        Focus::Agent => handle_agent_key(app, code, modifiers),
        Focus::ContextTransfer => handle_context_transfer_key(app, code),
    }
}

// ── Mouse: scroll wheel + Shift+Click to copy selection ─────────────

fn handle_mouse(app: &mut App, kind: MouseEventKind, modifiers: KeyModifiers) -> Result<()> {
    // Shift+Left release — terminal has already placed the selection in the
    // clipboard; just surface the "Copied" indicator as visual feedback.
    if matches!(kind, MouseEventKind::Up(MouseButton::Left))
        && modifiers.contains(KeyModifiers::SHIFT)
    {
        app.show_copied = true;
        app.copied_at = std::time::Instant::now();
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
                        agent.scroll_offset = (agent.scroll_offset + 5).min(max);
                    } else {
                        agent.scroll_offset = agent.scroll_offset.saturating_sub(5);
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
                        agent.scroll_offset = (agent.scroll_offset + 3).min(max);
                    } else {
                        agent.scroll_offset = agent.scroll_offset.saturating_sub(3);
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
                if dir > 0 && dialog.dir_selected > 0 {
                    dialog.dir_selected -= 1;
                } else if dir < 0 && dialog.dir_selected + 1 < dialog.dir_entries.len() {
                    dialog.dir_selected += 1;
                }
            }
        }
        Focus::ContextTransfer => {}
    }
    Ok(())
}

// ── Home: screensaver — arrows enter Preview ────────────────────────

fn handle_home_key(app: &mut App, code: KeyCode, _modifiers: KeyModifiers) -> Result<()> {
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
        KeyCode::F(1) => {
            app.show_legend = true;
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

fn handle_preview_key(app: &mut App, code: KeyCode, _modifiers: KeyModifiers) -> Result<()> {
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
        KeyCode::Char('q') => app.running = false,
        KeyCode::F(1) => {
            app.show_legend = true;
        }
        _ => {}
    }
    Ok(())
}

// ── Focus: PTY interaction or log scroll ────────────────────────────

fn handle_agent_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // Background agents: simple log-scrolling, single Esc → Preview
    if !matches!(app.selected_agent(), Some(AgentEntry::Interactive(_))) {
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

    // Ctrl+T: open context transfer modal
    if code == KeyCode::Char('t') && modifiers.contains(KeyModifiers::CONTROL) {
        app.open_context_transfer_modal();
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

    // F1 = toggle legend (intercept before PTY)
    if code == KeyCode::F(1) {
        app.show_legend = !app.show_legend;
        return Ok(());
    }

    // Tab = cycle to next interactive agent (focus mode)
    if code == KeyCode::Tab {
        app.next_interactive();
        return Ok(());
    }

    let Some(AgentEntry::Interactive(idx)) = app.selected_agent() else {
        app.focus = Focus::Home;
        return Ok(());
    };
    let idx = *idx;

    // Bounds check — agent may have been removed between ticks
    if idx >= app.interactive_agents.len() {
        app.focus = Focus::Preview;
        return Ok(());
    }

    // Shift+Up/Down = always scroll (even when not already scrolled)
    if modifiers.contains(KeyModifiers::SHIFT) {
        match code {
            KeyCode::Up => {
                let max = app.interactive_agents[idx].max_scroll();
                app.interactive_agents[idx].scroll_offset =
                    (app.interactive_agents[idx].scroll_offset + 3).min(max);
                return Ok(());
            }
            KeyCode::Down => {
                app.interactive_agents[idx].scroll_offset =
                    app.interactive_agents[idx].scroll_offset.saturating_sub(3);
                return Ok(());
            }
            _ => {}
        }
    }

    // Up/Down = scroll PTY history when scrolled up, otherwise pass to PTY.
    // PageUp/PageDown always scroll regardless of position.
    let max_scroll = app.interactive_agents[idx].max_scroll();
    let scrolled = app.interactive_agents[idx].scroll_offset > 0;
    match code {
        KeyCode::Up if scrolled => {
            app.interactive_agents[idx].scroll_offset =
                (app.interactive_agents[idx].scroll_offset + 3).min(max_scroll);
            return Ok(());
        }
        KeyCode::Down if scrolled => {
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

    // Record the prompt when the user presses Enter
    if code == KeyCode::Enter {
        if let Ok(input) = app.interactive_agents[idx].input_buffer.lock() {
            let captured = input.trim().to_string();
            if !captured.is_empty() {
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

    let bytes = key_to_bytes(code, modifiers);
    if !bytes.is_empty() {
        let _ = app.interactive_agents[idx].write_to_pty(&bytes);
    }

    Ok(())
}

// ── Dialog: new agent creation ──────────────────────────────────────
//
// Flow: ↑↓ switch fields, ←→ choose CLI/type/mode, ↑↓ in dir browser,
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
        _ => {
            let Some(dialog) = &mut app.new_agent_dialog else {
                return Ok(());
            };

            let is_interactive = matches!(dialog.task_type, super::app::NewTaskType::Interactive);
            let cli_field: usize = if is_interactive { 2 } else { 1 };
            let model_field: usize = if is_interactive { 3 } else { 2 };
            // Non-interactive only fields (prompt=3, extra=4 are before dir)
            let prompt_field: usize = 3;
            let extra_field: usize = 4;
            let dir_field: usize = if is_interactive {
                4
            } else if dialog.task_type == super::app::NewTaskType::Watcher {
                // Watcher reuses the extra_field (4) as the browser field so there is
                // no separate Dir field for Watchers.
                4
            } else {
                5
            };
            let _ = (prompt_field, extra_field); // used in non-interactive branches below

            match dialog.field {
                // BackgroundAgent type selector
                0 => match code {
                    KeyCode::Left => {
                        dialog.task_type = match dialog.task_type {
                            super::app::NewTaskType::Interactive => {
                                super::app::NewTaskType::Watcher
                            }
                            super::app::NewTaskType::Scheduled => {
                                super::app::NewTaskType::Interactive
                            }
                            super::app::NewTaskType::Watcher => super::app::NewTaskType::Scheduled,
                        };
                        dialog.refresh_dir_entries();
                    }
                    KeyCode::Right => {
                        dialog.task_type = match dialog.task_type {
                            super::app::NewTaskType::Interactive => {
                                super::app::NewTaskType::Scheduled
                            }
                            super::app::NewTaskType::Scheduled => super::app::NewTaskType::Watcher,
                            super::app::NewTaskType::Watcher => {
                                super::app::NewTaskType::Interactive
                            }
                        };
                        dialog.refresh_dir_entries();
                    }
                    KeyCode::Down | KeyCode::Tab => dialog.field = 1,
                    _ => {}
                },
                // Mode selector (Interactive only)
                1 if is_interactive => match code {
                    KeyCode::Left => {
                        dialog.task_mode = super::app::NewTaskMode::Interactive;
                    }
                    KeyCode::Right => {
                        dialog.task_mode = super::app::NewTaskMode::Resume;
                    }
                    KeyCode::Down | KeyCode::Tab => dialog.field = cli_field,
                    KeyCode::Up | KeyCode::BackTab => dialog.field = 0,
                    _ => {}
                },
                // CLI selector
                n if n == cli_field => match code {
                    KeyCode::Left => {
                        dialog.prev_cli();
                        dialog.refresh_model_suggestions();
                    }
                    KeyCode::Right => {
                        dialog.next_cli();
                        dialog.refresh_model_suggestions();
                    }
                    KeyCode::Down => dialog.field = model_field,
                    KeyCode::Up => {
                        dialog.field = if is_interactive { 1 } else { 0 };
                    }
                    _ => {}
                },
                // Model field — Space opens picker, ↑↓ navigate suggestions or fields
                n if n == model_field => match code {
                    KeyCode::Char(' ') => {
                        dialog.model_picker_open = true;
                        dialog.model_suggestion_idx = 0;
                        dialog.refresh_model_suggestions();
                    }
                    KeyCode::Char(c) => {
                        dialog.model.push(c);
                        dialog.model_picker_open = true;
                        dialog.model_suggestion_idx = 0;
                        dialog.refresh_model_suggestions();
                    }
                    KeyCode::Backspace => {
                        dialog.model.pop();
                        dialog.model_picker_open = !dialog.model.is_empty();
                        dialog.model_suggestion_idx = 0;
                        dialog.refresh_model_suggestions();
                    }
                    KeyCode::Down if dialog.model_picker_open => {
                        let len = dialog.model_suggestions.len();
                        if len > 0 {
                            dialog.model_suggestion_idx = (dialog.model_suggestion_idx + 1) % len;
                        }
                    }
                    KeyCode::Up if dialog.model_picker_open => {
                        let len = dialog.model_suggestions.len();
                        if len > 0 {
                            dialog.model_suggestion_idx = dialog
                                .model_suggestion_idx
                                .checked_sub(1)
                                .unwrap_or(len - 1);
                        }
                    }
                    KeyCode::Right if dialog.model_picker_open => {
                        dialog.accept_model_suggestion();
                    }
                    KeyCode::Enter if dialog.model_picker_open => {
                        dialog.accept_model_suggestion();
                        dialog.model_picker_open = false;
                    }
                    KeyCode::Esc | KeyCode::Left if dialog.model_picker_open => {
                        dialog.model_picker_open = false;
                    }
                    KeyCode::Up => {
                        dialog.model_picker_open = false;
                        dialog.field = cli_field;
                    }
                    KeyCode::Down => {
                        dialog.model_picker_open = false;
                        dialog.field = if is_interactive { dir_field } else { 3 };
                        // prompt or dir
                    }
                    _ => {}
                },
                // Prompt (scheduled/watcher only — field 3)
                3 if !is_interactive => match code {
                    KeyCode::Char(c) => dialog.prompt.push(c),
                    KeyCode::Backspace => {
                        dialog.prompt.pop();
                    }
                    KeyCode::Up => dialog.field = model_field,
                    KeyCode::Down => dialog.field = 4, // extra_field
                    _ => {}
                },
                // Cron expr or watch path (field 4, non-interactive only)
                4 if !is_interactive => match dialog.task_type {
                    super::app::NewTaskType::Scheduled => match code {
                        KeyCode::Char(c) => dialog.cron_expr.push(c),
                        KeyCode::Backspace => {
                            dialog.cron_expr.pop();
                        }
                        KeyCode::Up => dialog.field = 3, // prompt
                        KeyCode::Down => dialog.field = dir_field,
                        _ => {}
                    },
                    super::app::NewTaskType::Watcher => match code {
                        KeyCode::Char(c) => dialog.watch_path.push(c),
                        KeyCode::Backspace => {
                            dialog.watch_path.pop();
                        }
                        KeyCode::Up => dialog.field = 3, // prompt
                        KeyCode::Down => dialog.field = dir_field,
                        _ => {}
                    },
                    _ => {}
                },
                // Directory browser — ↑↓ navigate entries, ↑ at top exits up
                n if n == dir_field => match code {
                    KeyCode::Up => {
                        if dialog.dir_selected > 0 {
                            dialog.dir_selected -= 1;
                        } else if is_interactive {
                            dialog.field = model_field;
                        } else {
                            dialog.field = 4; // extra_field
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
                    _ => {}
                },
                _ => {}
            }
        }
    }
    Ok(())
}

// ── Context Transfer modal ───────────────────────────────────────
//
// Step 1 (Preview):  ↑↓ switch between n_prompts/scrollback fields,
//                    ←→ adjust values, Enter → Step 2, Esc → cancel.
// Step 2 (Picker):   ↑↓ navigate agents, Enter → execute, Esc → back.

fn handle_context_transfer_key(app: &mut App, code: KeyCode) -> Result<()> {
    use super::context_transfer::ContextTransferStep;

    let Some(modal) = app.context_transfer_modal.as_ref() else {
        app.focus = super::app::Focus::Agent;
        return Ok(());
    };

    match modal.step {
        ContextTransferStep::Preview => match code {
            KeyCode::Esc => {
                app.close_context_transfer_modal();
            }
            KeyCode::Enter => {
                app.context_transfer_to_picker();
            }
            KeyCode::Up | KeyCode::Down => {
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    modal.preview_field = 1 - modal.preview_field;
                }
            }
            KeyCode::Right | KeyCode::Char('+') => {
                let max = app.context_transfer_config.max_scrollback_lines;
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    modal.increment_field(max);
                }
                let src_idx = app
                    .context_transfer_modal
                    .as_ref()
                    .map(|m| m.source_agent_idx);
                if let Some(idx) = src_idx {
                    if idx < app.interactive_agents.len() {
                        // Reborrow safely via index
                        let (n_prompts, scrollback_lines) = app
                            .context_transfer_modal
                            .as_ref()
                            .map(|m| (m.n_prompts, m.scrollback_lines))
                            .unwrap();
                        let preview = super::context_transfer::build_context_payload(
                            &app.interactive_agents[idx],
                            n_prompts,
                            scrollback_lines,
                        );
                        if let Some(modal) = app.context_transfer_modal.as_mut() {
                            modal.payload_preview = preview;
                        }
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('-') => {
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    modal.decrement_field();
                }
                let src_idx = app
                    .context_transfer_modal
                    .as_ref()
                    .map(|m| m.source_agent_idx);
                if let Some(idx) = src_idx {
                    if idx < app.interactive_agents.len() {
                        let (n_prompts, scrollback_lines) = app
                            .context_transfer_modal
                            .as_ref()
                            .map(|m| (m.n_prompts, m.scrollback_lines))
                            .unwrap();
                        let preview = super::context_transfer::build_context_payload(
                            &app.interactive_agents[idx],
                            n_prompts,
                            scrollback_lines,
                        );
                        if let Some(modal) = app.context_transfer_modal.as_mut() {
                            modal.payload_preview = preview;
                        }
                    }
                }
            }
            _ => {}
        },
        ContextTransferStep::AgentPicker => match code {
            KeyCode::Esc => {
                // Go back to preview step
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    modal.step = ContextTransferStep::Preview;
                }
            }
            KeyCode::Up => {
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    if modal.picker_selected > 0 {
                        modal.picker_selected -= 1;
                    }
                }
            }
            KeyCode::Down => {
                let picker_len = app.picker_interactive_entries().len();
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    if modal.picker_selected + 1 < picker_len {
                        modal.picker_selected += 1;
                    }
                }
            }
            KeyCode::Enter => {
                let dest_idx = app
                    .context_transfer_modal
                    .as_ref()
                    .map(|m| m.picker_selected)
                    .unwrap_or(0);
                app.execute_context_transfer(dest_idx);
            }
            _ => {}
        },
    }
    Ok(())
}
