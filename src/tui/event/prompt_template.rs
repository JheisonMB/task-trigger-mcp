use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::path::PathBuf;

use crate::tui::app::types::{AgentEntry, App, Focus};

pub fn handle_prompt_template_key(
    app: &mut App,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> Result<()> {
    // Approximate instruction field width from terminal width
    // Must match the render calculation in dialogs.rs:
    //   dialog_width = (term_width * 65/100).max(40)
    //   inner_width  = dialog_width - 2 (borders)
    //   field_width  = inner_width - 2 (padding)
    let field_width = ((app.term_width as usize * 65 / 100).max(40))
        .saturating_sub(4)
        .max(10);
    let db = app.db.clone();

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
                KeyCode::Up if *selected > 0 => {
                    dialog.picker_mode = SectionPickerMode::AddSection {
                        selected: selected - 1,
                    };
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
                            } else if *name == "project_context" {
                                let entries =
                                    crate::tui::app::dialog::SimplePromptDialog::collect_projects_for_picker(&app.db)?;
                                dialog.picker_mode = SectionPickerMode::ProjectPicker {
                                    selected: 0,
                                    entries,
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
                KeyCode::Enter
                    if !input_copy.is_empty() && !dialog.enabled_sections.contains(&input_copy) =>
                {
                    dialog.add_section(&input_copy);
                    dialog.picker_mode = SectionPickerMode::None;
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
                KeyCode::Up if *selected > 0 => {
                    dialog.picker_mode = SectionPickerMode::RemoveSection {
                        selected: selected - 1,
                    };
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
                KeyCode::Up if selected > 0 => {
                    if let SectionPickerMode::SkillsPicker {
                        selected: ref mut s,
                        ..
                    } = dialog.picker_mode
                    {
                        *s = selected - 1;
                    }
                }
                KeyCode::Down if selected + 1 < count => {
                    if let SectionPickerMode::SkillsPicker {
                        selected: ref mut s,
                        ..
                    } = dialog.picker_mode
                    {
                        *s = selected + 1;
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
        SectionPickerMode::ProjectPicker { selected, entries } => {
            let selected = *selected;
            let count = entries.len();
            match code {
                KeyCode::Esc => {
                    dialog.picker_mode = SectionPickerMode::None;
                }
                KeyCode::Up if selected > 0 => {
                    if let SectionPickerMode::ProjectPicker {
                        selected: ref mut s,
                        ..
                    } = dialog.picker_mode
                    {
                        *s = selected - 1;
                    }
                }
                KeyCode::Down if selected + 1 < count => {
                    if let SectionPickerMode::ProjectPicker {
                        selected: ref mut s,
                        ..
                    } = dialog.picker_mode
                    {
                        *s = selected + 1;
                    }
                }
                KeyCode::Enter | KeyCode::Tab => {
                    if let SectionPickerMode::ProjectPicker { entries, selected } =
                        std::mem::replace(&mut dialog.picker_mode, SectionPickerMode::None)
                    {
                        if let Some(project) = entries.get(selected) {
                            dialog
                                .add_section_with_content("project_context", project.path.clone());
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

    // Expand collapsed paste so the user can see/edit real content.
    dialog.expand_collapsed_paste(&section_name);

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
            if let Ok(prompt) = dialog.build_prompt_with_resolved_resources(&db, &workdir) {
                if let Some(AgentEntry::Interactive(idx)) = app.selected_agent() {
                    let idx = *idx;
                    if idx < app.interactive_agents.len() {
                        let pasted = format!("\x1b[200~{}\x1b[201~", prompt);
                        let _ = app.interactive_agents[idx].write_to_pty(pasted.as_bytes());
                        let _ = app.interactive_agents[idx].write_to_pty(b"\r");
                    }
                }
                let workdir = app.current_workdir();
                app.prompt_builder_sessions.remove(&workdir);
                app.discard_simple_prompt_dialog();
            }
        }
        KeyCode::Enter => {
            let is_instruction =
                section_name == "instruction" || section_name.starts_with("instruction_");
            if is_instruction && modifiers.is_empty() {
                dialog.insert_newline_at_cursor(&section_name, field_width);
            } else if !is_instruction && modifiers.is_empty() {
                // Enter in non-instruction fields also sends
                if let Ok(prompt) = dialog.build_prompt_with_resolved_resources(&db, &workdir) {
                    if let Some(AgentEntry::Interactive(idx)) = app.selected_agent() {
                        let idx = *idx;
                        if idx < app.interactive_agents.len() {
                            let pasted = format!("\x1b[200~{}\x1b[201~", prompt);
                            let _ = app.interactive_agents[idx].write_to_pty(pasted.as_bytes());
                            let _ = app.interactive_agents[idx].write_to_pty(b"\r");
                        }
                    }
                    let workdir = app.current_workdir();
                    app.prompt_builder_sessions.remove(&workdir);
                    app.discard_simple_prompt_dialog();
                }
            }
        }
        KeyCode::Tab if dialog.focused_section + 1 < dialog.enabled_sections.len() => {
            dialog.focused_section += 1;
        }
        KeyCode::Tab => {}
        KeyCode::BackTab if dialog.focused_section > 0 => {
            dialog.focused_section -= 1;
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
        KeyCode::Up if dialog.focused_section > 0 => {
            dialog.focused_section -= 1;
        }
        KeyCode::Down if dialog.focused_section + 1 < dialog.enabled_sections.len() => {
            dialog.focused_section += 1;
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
