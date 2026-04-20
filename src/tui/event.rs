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
use std::path::PathBuf;
use std::time::Duration;

use super::agent::key_to_bytes;
use super::app::{AgentEntry, App, Focus, TerminalSearch};
use super::ui;

type Terminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>;

/// Main event loop: draw → poll events → refresh data.
pub fn run_event_loop(terminal: &mut Terminal, app: &mut App) -> Result<()> {
    while app.running {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Tick speed adapts to what needs frequent repaints
        let tick = match app.focus {
            Focus::Agent
            | Focus::NewAgentDialog
            | Focus::ContextTransfer
            | Focus::PromptTemplateDialog => Duration::from_millis(50),
            Focus::Preview
                if matches!(
                    app.selected_agent(),
                    Some(AgentEntry::Interactive(_)) | Some(AgentEntry::Terminal(_))
                ) =>
            {
                Duration::from_millis(100)
            }
            Focus::Home if app.brain.as_ref().is_some_and(|b| !b.active) => {
                Duration::from_millis(50)
            }
            Focus::Home if app.brain.as_ref().is_some_and(|b| b.active) => {
                Duration::from_millis(110)
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
                Event::Paste(text) => {
                    handle_paste(app, &text);
                }
                _ => {}
            }
        }

        app.refresh()?;
    }

    app.cleanup();
    Ok(())
}

// ── Prompt Template Dialog ──────────────────────────────────────

fn handle_prompt_template_key(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // Approximate instruction field width from terminal width
    // dialog is ~65% of terminal, minus borders/padding (~6 chars)
    let field_width = (app.term_width as usize * 65 / 100)
        .saturating_sub(6)
        .max(20);

    // Resolve workdir for @ file picker (needs agent borrow before dialog borrow).
    let workdir: PathBuf = app
        .selected_agent()
        .and_then(|a| match a {
            AgentEntry::Interactive(idx) => app
                .interactive_agents
                .get(*idx)
                .map(|ia| PathBuf::from(&ia.working_dir)),
            _ => None,
        })
        .unwrap_or_else(|| app.data_dir.parent().unwrap_or(&app.data_dir).to_path_buf());

    let Some(dialog) = &mut app.simple_prompt_dialog else {
        app.focus = Focus::Agent;
        return Ok(());
    };

    use crate::tui::app::dialog::SectionPickerMode;

    // If picker is open, handle picker navigation
    match &dialog.picker_mode {
        SectionPickerMode::AddSection { selected } => {
            match code {
                KeyCode::Esc => {
                    dialog.picker_mode = SectionPickerMode::None;
                }
                KeyCode::Up => {
                    if *selected > 0 {
                        dialog.picker_mode = SectionPickerMode::AddSection {
                            selected: selected - 1,
                        };
                    }
                }
                KeyCode::Down => {
                    let addable = dialog.get_addable_sections();
                    if *selected + 1 < addable.len() {
                        dialog.picker_mode = SectionPickerMode::AddSection {
                            selected: selected + 1,
                        };
                    }
                }
                KeyCode::Enter => {
                    let addable = dialog.get_addable_sections();
                    if *selected < addable.len() {
                        if let Some((name, _)) = addable.get(*selected) {
                            if *name == "tools" {
                                // Chain directly to SkillsPicker — no extra Ctrl+A needed
                                use crate::tui::app::dialog::SimplePromptDialog;
                                let entries =
                                    SimplePromptDialog::collect_skills_for_picker(&workdir);
                                dialog.picker_mode = SectionPickerMode::SkillsPicker {
                                    selected: 0,
                                    entries,
                                    replace_id: None,
                                };
                            } else {
                                dialog.add_section(name);
                                dialog.picker_mode = SectionPickerMode::None;
                            }
                        }
                    }
                }
                KeyCode::Char('c') => {
                    dialog.picker_mode = SectionPickerMode::AddCustom {
                        input: String::new(),
                    };
                }
                _ => {}
            }
            return Ok(());
        }
        SectionPickerMode::AddCustom { input } => {
            let input_copy = input.clone();
            match code {
                KeyCode::Esc => {
                    dialog.picker_mode = SectionPickerMode::None;
                }
                KeyCode::Enter => {
                    if !input_copy.is_empty() && !dialog.enabled_sections.contains(&input_copy) {
                        dialog.add_section(&input_copy);
                        dialog.picker_mode = SectionPickerMode::None;
                    }
                }
                KeyCode::Char(c) => {
                    dialog.picker_mode = SectionPickerMode::AddCustom {
                        input: format!("{}{}", input_copy, c),
                    };
                }
                KeyCode::Backspace => {
                    dialog.picker_mode = SectionPickerMode::AddCustom {
                        input: input_copy
                            .chars()
                            .take(input_copy.len().saturating_sub(1))
                            .collect(),
                    };
                }
                _ => {}
            }
            return Ok(());
        }
        SectionPickerMode::RemoveSection { selected } => {
            match code {
                KeyCode::Esc => {
                    dialog.picker_mode = SectionPickerMode::None;
                }
                KeyCode::Up => {
                    if *selected > 0 {
                        dialog.picker_mode = SectionPickerMode::RemoveSection {
                            selected: selected - 1,
                        };
                    }
                }
                KeyCode::Down => {
                    let removable = dialog.get_removable_sections();
                    if *selected + 1 < removable.len() {
                        dialog.picker_mode = SectionPickerMode::RemoveSection {
                            selected: selected + 1,
                        };
                    }
                }
                KeyCode::Enter => {
                    let removable = dialog.get_removable_sections();
                    if *selected < removable.len() {
                        if let Some((section_id, _)) = removable.get(*selected) {
                            dialog.remove_section(section_id);
                            dialog.picker_mode = SectionPickerMode::None;
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        SectionPickerMode::SkillsPicker {
            selected, entries, ..
        } => {
            let selected = *selected;
            let count = entries.len();
            match code {
                KeyCode::Esc => {
                    dialog.picker_mode = SectionPickerMode::None;
                }
                KeyCode::Up => {
                    if selected > 0 {
                        if let SectionPickerMode::SkillsPicker {
                            selected: ref mut s,
                            ..
                        } = dialog.picker_mode
                        {
                            *s = selected - 1;
                        }
                    }
                }
                KeyCode::Down => {
                    if selected + 1 < count {
                        if let SectionPickerMode::SkillsPicker {
                            selected: ref mut s,
                            ..
                        } = dialog.picker_mode
                        {
                            *s = selected + 1;
                        }
                    }
                }
                KeyCode::Enter | KeyCode::Tab => {
                    if let SectionPickerMode::SkillsPicker {
                        entries,
                        selected,
                        replace_id,
                    } = std::mem::replace(&mut dialog.picker_mode, SectionPickerMode::None)
                    {
                        if let Some((label, _, _)) = entries.get(selected) {
                            match replace_id {
                                Some(ref id) => dialog.set_tools_section_skill(id, label),
                                None => dialog.add_section_with_content("tools", label.clone()),
                            }
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        SectionPickerMode::None => {}
    }

    // Normal dialog mode — all sections support cursor editing
    let is_shift = modifiers.contains(KeyModifiers::SHIFT);
    let section_name = dialog.enabled_sections[dialog.focused_section].clone();

    // ── @ file-picker intercepts keys when active ─────────────────
    if dialog.at_picker.is_some() {
        match code {
            KeyCode::Esc => {
                // Close picker but keep the lone `@` in the text.
                dialog.at_picker = None;
            }
            KeyCode::Up => {
                if let Some(p) = &mut dialog.at_picker {
                    if p.selected > 0 {
                        p.selected -= 1;
                    } else {
                        p.selected = p.entries.len().saturating_sub(1);
                    }
                }
            }
            KeyCode::Down => {
                if let Some(p) = &mut dialog.at_picker {
                    if p.selected + 1 < p.entries.len() {
                        p.selected += 1;
                    } else {
                        p.selected = 0;
                    }
                }
            }
            KeyCode::Left => {
                if let Some(p) = &mut dialog.at_picker {
                    p.go_up();
                }
            }
            KeyCode::Right => {
                let is_dir = dialog
                    .at_picker
                    .as_ref()
                    .and_then(|p| p.entries.get(p.selected))
                    .map(|e| e.is_dir)
                    .unwrap_or(false);
                if is_dir {
                    if let Some(p) = &mut dialog.at_picker {
                        p.enter_dir();
                    }
                }
            }
            KeyCode::Enter | KeyCode::Tab => {
                // Enter/Tab always selects the highlighted entry — file OR directory.
                // Use → (Right arrow) to navigate inside a directory without selecting it.
                let rel = dialog
                    .at_picker
                    .as_ref()
                    .and_then(|p| p.relative_path_of_selected());
                let full = dialog
                    .at_picker
                    .as_ref()
                    .and_then(|p| p.full_path_of_selected());
                if let (Some(rel_path), Some(full_path)) = (rel, full) {
                    let full_str = full_path.to_string_lossy().to_string();
                    let orig_focus = dialog.focused_section;
                    dialog.insert_at_completion(&section_name, &rel_path, &full_str, field_width);
                    // Explicitly restore focus so the section where @ was typed stays active.
                    dialog.focused_section = orig_focus;
                }
                dialog.at_picker = None;
            }
            KeyCode::Backspace => {
                let query_empty = dialog
                    .at_picker
                    .as_ref()
                    .map(|p| p.query.is_empty())
                    .unwrap_or(true);
                if query_empty {
                    // Remove the `@` and close.
                    dialog.at_picker = None;
                    dialog.backspace_at_cursor(&section_name, field_width);
                } else {
                    if let Some(p) = &mut dialog.at_picker {
                        p.query.pop();
                        p.refresh();
                    }
                }
            }
            KeyCode::Char(c) if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                let ch = if modifiers.contains(KeyModifiers::SHIFT) {
                    c.to_uppercase().next().unwrap_or(c)
                } else {
                    c
                };
                if let Some(p) = &mut dialog.at_picker {
                    p.query.push(ch);
                    p.refresh();
                }
            }
            _ => {}
        }
        // Trigger a fresh picker on `@` even while picker is active? No — just close old one.
        // Ensure the workdir-owned AtPicker is available (no extra work needed here).
        let _ = workdir; // consumed above if needed
        return Ok(());
    }

    match code {
        KeyCode::Esc => {
            app.close_simple_prompt_dialog();
        }
        // Ctrl+S → send prompt (reliable across all terminals)
        KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
            if let Ok(prompt) = dialog.build_prompt() {
                if let Some(AgentEntry::Interactive(idx)) = app.selected_agent() {
                    let idx = *idx;
                    if idx < app.interactive_agents.len() {
                        let pasted = format!("\x1b[200~{}\x1b[201~", prompt);
                        let _ = app.interactive_agents[idx].write_to_pty(pasted.as_bytes());
                        let _ = app.interactive_agents[idx].write_to_pty(b"\r");
                    }
                }
                app.close_simple_prompt_dialog();
            }
        }
        KeyCode::Enter => {
            let is_instruction =
                section_name == "instruction" || section_name.starts_with("instruction_");
            if is_instruction && modifiers.is_empty() {
                dialog.insert_newline_at_cursor(&section_name, field_width);
            } else if !is_instruction && modifiers.is_empty() {
                // Enter in non-instruction fields also sends
                if let Ok(prompt) = dialog.build_prompt() {
                    if let Some(AgentEntry::Interactive(idx)) = app.selected_agent() {
                        let idx = *idx;
                        if idx < app.interactive_agents.len() {
                            let pasted = format!("\x1b[200~{}\x1b[201~", prompt);
                            let _ = app.interactive_agents[idx].write_to_pty(pasted.as_bytes());
                            let _ = app.interactive_agents[idx].write_to_pty(b"\r");
                        }
                    }
                    app.close_simple_prompt_dialog();
                }
            }
        }
        KeyCode::Tab => {
            if dialog.focused_section + 1 < dialog.enabled_sections.len() {
                dialog.focused_section += 1;
            }
        }
        KeyCode::BackTab => {
            if dialog.focused_section > 0 {
                dialog.focused_section -= 1;
            }
        }
        // Shift+Arrow → move cursor within the focused section
        KeyCode::Left if is_shift => {
            dialog.move_cursor_left(&section_name, field_width);
        }
        KeyCode::Right if is_shift => {
            dialog.move_cursor_right(&section_name, field_width);
        }
        KeyCode::Up if is_shift => {
            dialog.move_cursor_up(&section_name, field_width);
        }
        KeyCode::Down if is_shift => {
            dialog.move_cursor_down(&section_name, field_width);
        }
        // Plain arrows → navigate between sections
        KeyCode::Up => {
            if dialog.focused_section > 0 {
                dialog.focused_section -= 1;
            }
        }
        KeyCode::Down => {
            if dialog.focused_section + 1 < dialog.enabled_sections.len() {
                dialog.focused_section += 1;
            }
        }
        // Ctrl+A → if on tools section: open SkillsPicker to replace; else: open add-section picker
        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
            use crate::tui::app::dialog::SimplePromptDialog;
            if SimplePromptDialog::is_tools_section(&section_name) {
                let entries = SimplePromptDialog::collect_skills_for_picker(&workdir);
                dialog.picker_mode = SectionPickerMode::SkillsPicker {
                    selected: 0,
                    entries,
                    replace_id: Some(section_name.clone()),
                };
            } else {
                let addable = dialog.get_addable_sections();
                if !addable.is_empty() {
                    dialog.picker_mode = SectionPickerMode::AddSection { selected: 0 };
                }
            }
        }
        // Ctrl+X → open remove-section picker
        KeyCode::Char('x') if modifiers.contains(KeyModifiers::CONTROL) => {
            let removable = dialog.get_removable_sections();
            if !removable.is_empty() {
                dialog.picker_mode = SectionPickerMode::RemoveSection { selected: 0 };
            }
        }
        KeyCode::Char(c) => {
            use crate::tui::app::dialog::SimplePromptDialog;
            // Tools sections are read-only — no direct text input.
            if SimplePromptDialog::is_tools_section(&section_name) {
                return Ok(());
            }
            // Insert first so cursor advances, then check for `@` trigger.
            dialog.insert_char_at_cursor(&section_name, c, field_width);
            if c == '@' && dialog.at_picker.is_none() {
                use crate::tui::app::dialog::AtPicker;
                let trigger_pos = dialog.cursor(&section_name).saturating_sub(1);
                dialog.at_picker = Some(AtPicker::new(workdir, trigger_pos));
            }
        }
        KeyCode::Backspace => {
            use crate::tui::app::dialog::SimplePromptDialog;
            // Tools sections are read-only — backspace is a no-op.
            if SimplePromptDialog::is_tools_section(&section_name) {
                return Ok(());
            }
            dialog.backspace_at_cursor(&section_name, field_width);
        }
        _ => {}
    }

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

    // Ctrl+B: open prompt builder dialog from focus mode
    if code == KeyCode::Char('b')
        && modifiers.contains(KeyModifiers::CONTROL)
        && matches!(app.focus, Focus::Agent)
    {
        app.open_simple_prompt_dialog(None);
        return Ok(());
    }

    // Ctrl+F: search in scrollback (interactive or terminal agents)
    if code == KeyCode::Char('f')
        && modifiers.contains(KeyModifiers::CONTROL)
        && matches!(app.focus, Focus::Agent)
    {
        match app.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => {
                app.terminal_search = Some(super::app::TerminalSearch::new_interactive(*idx));
            }
            Some(AgentEntry::Terminal(idx)) => {
                app.terminal_search = Some(super::app::TerminalSearch::new(*idx));
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
        Focus::PromptTemplateDialog => {}
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
        KeyCode::F(4) => app.running = false,
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
        _ => {
            // Any unbound key resets the brain animation back to the banner
            if app.brain.as_ref().is_some_and(|b| b.active) {
                app.dismiss_brain();
            }
        }
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
            // For Group entries: Enter activates the split and enters focus
            if let Some(AgentEntry::Group(idx)) = app.selected_agent() {
                let idx = *idx;
                if let Some(group) = app.split_groups.get(idx) {
                    let id = group.id.clone();
                    app.active_split_id = Some(id);
                    app.split_right_focused = false;
                }
                app.focus = Focus::Agent;
                return Ok(());
            }
            app.log_scroll = 0;
            app.focus = Focus::Agent;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.select_next();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.select_prev();
        }
        KeyCode::Char('e') => {
            app.open_edit_dialog();
        }
        KeyCode::Char('d') => {
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

    // Ctrl+X: dissolve current split
    if code == KeyCode::Char('x') && modifiers.contains(KeyModifiers::CONTROL) {
        app.dissolve_split();
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

    // F4 = terminate current session (or all sessions in a split group)
    if code == KeyCode::F(4) {
        app.terminate_focused_session();
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
    if modifiers.contains(KeyModifiers::SHIFT) {
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
    if agent_vec == "interactive" {
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
    }

    // Terminal: track input buffer + record history on Enter
    if agent_vec == "terminal" {
        // Ctrl+W = toggle warp mode
        if code == KeyCode::Char('w') && modifiers.contains(KeyModifiers::CONTROL) {
            app.terminal_agents[idx].warp_mode = !app.terminal_agents[idx].warp_mode;
            return Ok(());
        }

        let warp = app.terminal_agents[idx].warp_mode;

        if warp {
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
            let empty = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|b| b.trim().is_empty())
                .unwrap_or(true);
            if empty {
                return open_terminal_suggestion_picker(app, idx);
            }
            // Non-empty: forward Tab to PTY for native autocomplete
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

// ── Terminal warp-mode key handling ─────────────────────────────────

/// Handle keystrokes for a terminal agent in warp mode.
/// Keys are accumulated in the input buffer and only sent to PTY on Enter.
fn handle_terminal_warp_key(
    app: &mut App,
    idx: usize,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<()> {
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
        }
        KeyCode::Tab => {
            let empty = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|b| b.trim().is_empty())
                .unwrap_or(true);
            if empty {
                return open_terminal_suggestion_picker(app, idx);
            }
            // Non-empty: send current input + Tab to PTY for native autocomplete.
            // Keep the warp buffer intact — the user continues editing from where they were.
            let text = app.terminal_agents[idx]
                .input_buffer
                .lock()
                .map(|b| b.clone())
                .unwrap_or_default();
            let _ = app.terminal_agents[idx].write_to_pty(text.as_bytes());
            let _ = app.terminal_agents[idx].write_to_pty(b"\t");
            return Ok(());
        }
        KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
            let cursor = app.terminal_agents[idx].warp_cursor;
            if let Ok(mut buf) = app.terminal_agents[idx].input_buffer.lock() {
                let pos = cursor.min(buf.len());
                buf.insert(pos, c);
            }
            app.terminal_agents[idx].warp_cursor = cursor + c.len_utf8();
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
            // Scroll up through terminal scrollback
            let max = app.terminal_agents[idx].max_scroll();
            app.terminal_agents[idx].scroll_offset =
                (app.terminal_agents[idx].scroll_offset + 3).min(max);
        }
        KeyCode::Down => {
            // Scroll down (towards live view)
            app.terminal_agents[idx].scroll_offset =
                app.terminal_agents[idx].scroll_offset.saturating_sub(3);
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
        }
        // Ctrl+D — send EOF
        KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
            let _ = app.terminal_agents[idx].write_to_pty(&[0x04]); // EOT
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
fn record_terminal_command(app: &mut App, idx: usize, captured: &str) {
    if captured.is_empty() {
        return;
    }
    let trimmed = captured.trim();
    if trimmed == "cd" || trimmed.starts_with("cd ") || trimmed.starts_with("cd\t") {
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
    super::terminal_history::save_history(&app.data_dir, &session_name, hist);
    // Global catalog (idempotent, excludes cd)
    super::terminal_history::record_global_catalog(&app.data_dir, captured, &cwd);
}

/// Open the suggestion picker for a terminal agent.
fn open_terminal_suggestion_picker(app: &mut App, idx: usize) -> Result<()> {
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
        let global = super::terminal_history::load_all_histories(&app.data_dir);
        app.suggestion_picker = Some(super::terminal_history::SuggestionPicker::for_cd(
            partial, &cwd, &global,
        ));
    } else {
        // Command history uses session-only history (per-session counts)
        // Tab: global command catalog (all terminals contribute)
        app.suggestion_picker = Some(super::terminal_history::from_global_catalog(
            &input_text,
            &app.data_dir,
            &cwd,
        ));
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

    {
        let Some(dialog) = &mut app.new_agent_dialog else {
            return Ok(());
        };

        // Session picker intercepts ALL keys when open
        if dialog.session_picker_open {
            match code {
                KeyCode::Down => {
                    let len = dialog.session_entries.len();
                    if len > 0 {
                        dialog.session_picker_idx = (dialog.session_picker_idx + 1) % len;
                    }
                }
                KeyCode::Up => {
                    let len = dialog.session_entries.len();
                    if len > 0 {
                        dialog.session_picker_idx =
                            dialog.session_picker_idx.checked_sub(1).unwrap_or(len - 1);
                    }
                }
                KeyCode::Enter => {
                    dialog.confirm_session_pick();
                }
                KeyCode::Esc | KeyCode::Backspace => {
                    dialog.session_picker_open = false;
                }
                _ => {}
            }
            return Ok(());
        }
    }

    match code {
        KeyCode::Esc => app.close_new_agent_dialog(),
        KeyCode::Enter => {
            // If on mode field in Resume mode with session picker, open picker instead of launching
            let should_pick = app.new_agent_dialog.as_ref().is_some_and(|d| {
                let is_interactive = matches!(d.task_type, super::app::NewTaskType::Interactive);
                let mode_field: usize = 1;
                is_interactive
                    && d.field == mode_field
                    && matches!(d.task_mode, super::app::NewTaskMode::Resume)
                    && d.has_session_picker()
            });
            if should_pick {
                if let Some(dialog) = &mut app.new_agent_dialog {
                    dialog.open_session_picker();
                }
            } else {
                let _ = app.launch_new_agent();
            }
        }
        _ => {
            let Some(dialog) = &mut app.new_agent_dialog else {
                return Ok(());
            };

            let is_interactive = matches!(dialog.task_type, super::app::NewTaskType::Interactive);
            let is_terminal = matches!(dialog.task_type, super::app::NewTaskType::Terminal);
            // Interactive: 0=type, 1=mode, 2=CLI, 3=model, 4=dir, 5=yolo
            // Scheduled/Watcher: 0=type, 1=CLI, 2=model, 3=prompt, 4=cron/watch, 5=dir
            // Terminal: 0=type, 1=dir, 2=shell
            let cli_field: usize = if is_interactive { 2 } else { 1 };
            let model_field: usize = if is_interactive { 3 } else { 2 };
            let prompt_field: usize = 3;
            let extra_field: usize = 4;
            let dir_field: usize = if is_interactive {
                4
            } else if is_terminal {
                1
            } else if dialog.task_type == super::app::NewTaskType::Watcher {
                4
            } else {
                5
            };
            let yolo_field: usize = 5; // interactive only
            let _ = (prompt_field, extra_field); // used in specific branches below

            match dialog.field {
                // BackgroundAgent type selector
                0 => match code {
                    KeyCode::Left => {
                        dialog.task_type = match dialog.task_type {
                            super::app::NewTaskType::Interactive => {
                                super::app::NewTaskType::Terminal
                            }
                            super::app::NewTaskType::Scheduled => {
                                super::app::NewTaskType::Interactive
                            }
                            super::app::NewTaskType::Watcher => super::app::NewTaskType::Scheduled,
                            super::app::NewTaskType::Terminal => super::app::NewTaskType::Watcher,
                        };
                        dialog.refresh_dir_entries();
                    }
                    KeyCode::Right => {
                        dialog.task_type = match dialog.task_type {
                            super::app::NewTaskType::Interactive => {
                                super::app::NewTaskType::Scheduled
                            }
                            super::app::NewTaskType::Scheduled => super::app::NewTaskType::Watcher,
                            super::app::NewTaskType::Watcher => super::app::NewTaskType::Terminal,
                            super::app::NewTaskType::Terminal => {
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
                        dialog.task_mode = match dialog.task_mode {
                            super::app::NewTaskMode::Interactive => super::app::NewTaskMode::Resume,
                            super::app::NewTaskMode::Resume => super::app::NewTaskMode::Interactive,
                        };
                        dialog.selected_session = None;
                    }
                    KeyCode::Right => {
                        dialog.task_mode = match dialog.task_mode {
                            super::app::NewTaskMode::Interactive => super::app::NewTaskMode::Resume,
                            super::app::NewTaskMode::Resume => super::app::NewTaskMode::Interactive,
                        };
                        dialog.selected_session = None;
                    }
                    KeyCode::Delete | KeyCode::Backspace
                        if matches!(dialog.task_mode, super::app::NewTaskMode::Resume) =>
                    {
                        dialog.clear_selected_session();
                    }
                    KeyCode::Down | KeyCode::Tab => dialog.field = cli_field,
                    KeyCode::Up | KeyCode::BackTab => dialog.field = 0,
                    _ => {}
                },
                // CLI selector
                n if n == cli_field && !is_terminal => match code {
                    KeyCode::Left => {
                        dialog.prev_cli();
                        dialog.refresh_model_suggestions();
                        if dialog.selected_yolo_flag().is_none() {
                            dialog.yolo_mode = false;
                        }
                    }
                    KeyCode::Right => {
                        dialog.next_cli();
                        dialog.refresh_model_suggestions();
                        if dialog.selected_yolo_flag().is_none() {
                            dialog.yolo_mode = false;
                        }
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
                        dialog.field = if is_interactive { yolo_field } else { 3 };
                        // yolo (interactive) or prompt (non-interactive)
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
                // Cron expr (field 4, Scheduled only — Watcher uses field 4 as the browser)
                4 if !is_interactive
                    && matches!(dialog.task_type, super::app::NewTaskType::Scheduled) =>
                {
                    match code {
                        KeyCode::Char(c) => dialog.cron_expr.push(c),
                        KeyCode::Backspace => {
                            dialog.cron_expr.pop();
                        }
                        KeyCode::Up => dialog.field = 3, // prompt
                        KeyCode::Down => dialog.field = dir_field,
                        _ => {}
                    }
                }
                // Directory browser — ↑↓ navigate  → enter dir  ← go up  Space alias for →
                // For Watcher, dir_field == 4 (same as extra_field), so this arm also handles
                // the watch-path browser. The Scheduled arm above is guarded to avoid shadowing.
                // For Terminal, dir_field == 1.
                n if n == dir_field => match code {
                    KeyCode::Up => {
                        if dialog.dir_selected > 0 {
                            dialog.dir_selected -= 1;
                        } else if is_interactive {
                            dialog.field = yolo_field; // yolo is above dir for interactive
                        } else if is_terminal {
                            dialog.field = 0; // type selector
                        } else if dialog.task_type == super::app::NewTaskType::Watcher {
                            dialog.field = 3; // prompt (watcher has no separate cron field)
                        } else {
                            dialog.field = 4; // extra_field (cron) for scheduled
                        }
                    }
                    KeyCode::Down => {
                        if dialog.dir_selected + 1 < dialog.dir_entries.len() {
                            dialog.dir_selected += 1;
                        } else if is_terminal {
                            dialog.field = 2; // shell field for terminal
                        }
                    }
                    KeyCode::Right | KeyCode::Char(' ') => {
                        dialog.navigate_to_selected();
                    }
                    KeyCode::Left => {
                        dialog.go_up();
                    }
                    _ => {}
                },
                // Shell picker (Terminal only — field 2): ←→ cycle shells
                2 if is_terminal => match code {
                    KeyCode::Left | KeyCode::Right => {
                        let count = dialog.available_shells.len();
                        if count > 0 {
                            dialog.shell_index = if code == KeyCode::Right {
                                (dialog.shell_index + 1) % count
                            } else {
                                (dialog.shell_index + count - 1) % count
                            };
                        }
                    }
                    KeyCode::Up | KeyCode::BackTab => dialog.field = dir_field,
                    _ => {}
                },
                // Yolo toggle (interactive only — field 5), sits between model and dir
                n if n == yolo_field && is_interactive => match code {
                    KeyCode::Char(' ') => {
                        if dialog.selected_yolo_flag().is_some() {
                            dialog.yolo_mode = !dialog.yolo_mode;
                        }
                    }
                    KeyCode::Up | KeyCode::BackTab => {
                        dialog.field = model_field;
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        dialog.field = dir_field;
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
// Step 1 (Preview):  ↑↓ / ←→ adjust n_prompts, Enter → Step 2, Esc → cancel.
// Step 2 (Picker):   ↑↓ navigate agents, Enter → execute, Esc → back.

/// Rebuild the payload_preview string from the current source agent state.
fn ctx_rebuild_preview(app: &mut App) {
    let src_info = app
        .context_transfer_modal
        .as_ref()
        .map(|m| (m.source_agent_idx, m.source_is_terminal, m.n_prompts));
    if let Some((idx, is_terminal, n_prompts)) = src_info {
        if is_terminal {
            if idx < app.terminal_agents.len() {
                let preview = super::context_transfer::build_terminal_context_payload(
                    &app.terminal_agents[idx],
                    n_prompts,
                );
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    modal.payload_preview = preview;
                }
            }
        } else if idx < app.interactive_agents.len() {
            let preview = super::context_transfer::build_context_payload(
                &app.interactive_agents[idx],
                n_prompts,
            );
            if let Some(modal) = app.context_transfer_modal.as_mut() {
                modal.payload_preview = preview;
            }
        }
    }
}

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
            KeyCode::Right | KeyCode::Up | KeyCode::Char('+') => {
                // Determine the max allowed before incrementing.
                // For interactive: cap by actual prompt history length.
                // For terminal: cap at 20 "pages" (each = 50 lines).
                let history_len = if app
                    .context_transfer_modal
                    .as_ref()
                    .is_some_and(|m| m.source_is_terminal)
                {
                    20
                } else {
                    app.context_transfer_modal
                        .as_ref()
                        .and_then(|m| app.interactive_agents.get(m.source_agent_idx))
                        .and_then(|a| a.prompt_history.lock().ok().map(|h| h.len()))
                        .unwrap_or(0)
                        .max(1)
                };
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    modal.n_prompts = (modal.n_prompts + 1).min(history_len);
                }
                ctx_rebuild_preview(app);
            }
            KeyCode::Left | KeyCode::Down | KeyCode::Char('-') => {
                if let Some(modal) = app.context_transfer_modal.as_mut() {
                    modal.decrement_field();
                }
                ctx_rebuild_preview(app);
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

/// Resolve a session name to (vec_tag, index) for PTY input routing.
fn resolve_session(app: &App, name: &str) -> (&'static str, usize) {
    if let Some(idx) = app.interactive_agents.iter().position(|a| a.name == name) {
        return ("interactive", idx);
    }
    if let Some(idx) = app.terminal_agents.iter().position(|a| a.name == name) {
        return ("terminal", idx);
    }
    ("interactive", usize::MAX)
}

// ── Suggestion picker (terminal Tab autocomplete) ───────────────────

/// Handle keys while the terminal suggestion picker is visible.
fn handle_suggestion_picker_key(app: &mut App, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Down => {
            if let Some(picker) = app.suggestion_picker.as_mut() {
                picker.move_down();
            }
        }
        KeyCode::Up => {
            if let Some(picker) = app.suggestion_picker.as_mut() {
                picker.move_up();
            }
        }
        KeyCode::Right => {
            let focused_name = app.focused_agent_name();
            let base_cwd = app
                .terminal_agents
                .iter()
                .find(|a| a.name == focused_name)
                .map(|a| a.working_dir.clone())
                .unwrap_or_default();
            if let Some(picker) = app.suggestion_picker.as_mut() {
                if picker.mode == super::terminal_history::PickerMode::CdDirectory {
                    let _ = picker.navigate_into(&base_cwd);
                }
            }
        }
        KeyCode::Left => {
            let focused_name = app.focused_agent_name();
            let base_cwd = app
                .terminal_agents
                .iter()
                .find(|a| a.name == focused_name)
                .map(|a| a.working_dir.clone())
                .unwrap_or_default();
            if let Some(picker) = app.suggestion_picker.as_mut() {
                if picker.mode == super::terminal_history::PickerMode::CdDirectory {
                    let _ = picker.navigate_parent(&base_cwd);
                }
            }
        }
        KeyCode::Enter => {
            // For cd mode, resolve full path from the browsed directory
            let resolved = app.suggestion_picker.as_ref().and_then(|p| {
                if p.mode != super::terminal_history::PickerMode::CdDirectory {
                    return p.selected_text().map(|t| (t.to_string(), false));
                }
                let selected = p.selected_text()?;
                if selected == ".." {
                    // Parent directory — use the cd_current_dir's parent
                    let parent = p.cd_current_dir.as_ref()?.parent()?;
                    return Some((parent.to_string_lossy().to_string(), true));
                }
                let cd_dir = p.cd_current_dir.as_ref()?;
                if let Some(stripped) = selected.strip_prefix("./") {
                    let full = cd_dir.join(stripped);
                    Some((full.to_string_lossy().to_string(), true))
                } else {
                    Some((selected.to_string(), true))
                }
            });
            app.suggestion_picker = None;

            if let Some((text, is_cd)) = resolved {
                insert_suggestion_into_terminal(app, &text, is_cd);
            }
        }
        KeyCode::Esc | KeyCode::Tab => {
            app.suggestion_picker = None;
        }
        _ => {}
    }
    Ok(())
}

/// Insert the selected suggestion into the terminal's input.
fn insert_suggestion_into_terminal(app: &mut App, text: &str, is_cd: bool) {
    let term_idx = find_focused_terminal(app);
    let Some(idx) = term_idx else { return };

    let full_text = if is_cd {
        format!("cd {text}")
    } else {
        text.to_string()
    };

    let Some(agent) = app.terminal_agents.get_mut(idx) else {
        return;
    };

    if agent.warp_mode {
        // Warp mode: only update the input buffer (PTY has nothing typed yet)
        if let Ok(mut buf) = agent.input_buffer.lock() {
            buf.clear();
            buf.push_str(&full_text);
        }
        agent.warp_cursor = full_text.len();
    } else {
        // Non-warp: clear PTY line with Ctrl+U then type suggestion
        let mut bytes: Vec<u8> = vec![0x15]; // Ctrl+U
        bytes.extend(full_text.as_bytes());
        let _ = agent.write_to_pty(&bytes);
        if let Ok(mut buf) = agent.input_buffer.lock() {
            buf.clear();
            buf.push_str(&full_text);
        }
    }
}

/// Find the index of the terminal agent that currently has focus.
fn find_focused_terminal(app: &App) -> Option<usize> {
    if let Some(ref split_id) = app.active_split_id {
        let name = app
            .split_groups
            .iter()
            .find(|g| g.id == *split_id)
            .map(|g| {
                if app.split_right_focused {
                    &g.session_b
                } else {
                    &g.session_a
                }
            })?;
        app.terminal_agents.iter().position(|a| &a.name == name)
    } else {
        match app.selected_agent() {
            Some(AgentEntry::Terminal(idx)) => {
                let idx = *idx;
                if idx < app.terminal_agents.len() {
                    Some(idx)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

// ── Paste handling (bracketed paste) ─────────────────────────────────

/// Handle pasted text — inserts into the active input buffer without triggering sends.
/// Newlines are replaced with spaces to prevent accidental prompt submission.
fn handle_paste(app: &mut App, text: &str) {
    // Replace newlines with spaces
    let clean = text.replace('\n', " ").replace('\r', "");

    match app.focus {
        Focus::Agent => {
            let (vec, idx) = if let Some(split_id) = &app.active_split_id {
                let id = split_id.clone();
                resolve_session(app, &id)
            } else {
                match app.selected_agent() {
                    Some(AgentEntry::Interactive(idx)) => ("interactive", *idx),
                    Some(AgentEntry::Terminal(idx)) => ("terminal", *idx),
                    _ => return,
                }
            };

            let agent = if vec == "terminal" {
                app.terminal_agents.get_mut(idx)
            } else {
                app.interactive_agents.get_mut(idx)
            };
            if let Some(agent) = agent {
                if agent.warp_mode {
                    // Warp mode: insert into input buffer at cursor
                    if let Ok(mut buf) = agent.input_buffer.lock() {
                        let pos = agent.warp_cursor.min(buf.len());
                        buf.insert_str(pos, &clean);
                        agent.warp_cursor = pos + clean.len();
                    }
                } else {
                    // Non-warp: send directly to PTY (with newlines preserved for PTY)
                    let _ = agent.write_to_pty(text.as_bytes());
                }
            }
        }
        _ => {
            // For dialogs or other contexts, simulate typing each char
            for c in clean.chars() {
                let _ = handle_key(app, KeyCode::Char(c), KeyModifiers::NONE);
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
