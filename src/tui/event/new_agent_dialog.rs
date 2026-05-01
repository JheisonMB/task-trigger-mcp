use anyhow::Result;
use ratatui::crossterm::event::KeyCode;

use crate::tui::app::types::App;

// ── Dialog: new agent creation ──────────────────────────────────────
//
// Flow: ↑↓ switch fields, ←→ choose CLI/type/mode, ↑↓ in dir browser,
//       Space enter directory, Enter launch, Esc cancel.

pub fn handle_dialog_key(app: &mut App, code: KeyCode) -> Result<()> {
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

        // CLI picker intercepts ALL keys when open
        if dialog.cli_picker_open {
            match code {
                KeyCode::Down => {
                    let len = dialog.available_clis.len();
                    if len > 0 {
                        dialog.cli_picker_idx = (dialog.cli_picker_idx + 1) % len;
                        dialog.cli_index = dialog.cli_picker_idx;
                        dialog.refresh_model_suggestions();
                        if dialog.selected_yolo_flag().is_none() {
                            dialog.yolo_mode = false;
                        }
                    }
                }
                KeyCode::Up => {
                    let len = dialog.available_clis.len();
                    if len > 0 {
                        dialog.cli_picker_idx =
                            dialog.cli_picker_idx.checked_sub(1).unwrap_or(len - 1);
                        dialog.cli_index = dialog.cli_picker_idx;
                        dialog.refresh_model_suggestions();
                        if dialog.selected_yolo_flag().is_none() {
                            dialog.yolo_mode = false;
                        }
                    }
                }
                KeyCode::Enter => {
                    dialog.cli_index = dialog.cli_picker_idx;
                    dialog.cli_picker_open = false;
                    dialog.refresh_model_suggestions();
                    if dialog.selected_yolo_flag().is_none() {
                        dialog.yolo_mode = false;
                    }
                }
                KeyCode::Esc => {
                    dialog.cli_picker_open = false;
                }
                KeyCode::Char(c) => {
                    // Jump to first CLI starting with the typed letter
                    if let Some(idx) = dialog
                        .available_clis
                        .iter()
                        .position(|cli| cli.as_str().starts_with(c))
                    {
                        dialog.cli_picker_idx = idx;
                        dialog.cli_index = dialog.cli_picker_idx;
                        dialog.refresh_model_suggestions();
                        if dialog.selected_yolo_flag().is_none() {
                            dialog.yolo_mode = false;
                        }
                    }
                }
                _ => {}
            }
            return Ok(());
        }
    }

    match code {
        KeyCode::Esc => app.close_new_agent_dialog(),
        KeyCode::Enter => {
            // If in Resume mode with session picker and no session selected yet,
            // open the picker regardless of which field is focused.
            let should_pick = app.new_agent_dialog.as_ref().is_some_and(|d| {
                let is_interactive =
                    matches!(d.task_type, crate::tui::app::NewTaskType::Interactive);
                is_interactive
                    && matches!(d.task_mode, crate::tui::app::NewTaskMode::Resume)
                    && d.has_session_picker()
                    && d.selected_session.is_none()
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

            let is_interactive =
                matches!(dialog.task_type, crate::tui::app::NewTaskType::Interactive);
            let is_terminal = matches!(dialog.task_type, crate::tui::app::NewTaskType::Terminal);
            let is_background =
                matches!(dialog.task_type, crate::tui::app::NewTaskType::Background);

            // Field layout:
            //   Interactive: 0=type 1=mode 2=CLI 3=dir 4=yolo
            //   Terminal:    0=type 1=dir 2=shell
            //   Background:  0=type 1=trigger 2=CLI 3=model 4=prompt 5=cron/watch 6=dir
            let cli_field: usize = if is_interactive || is_background {
                2
            } else {
                0
            };
            let model_field: usize = 3; // background only
            let prompt_field: usize = 4; // background only
            let extra_field: usize = 5; // background only
            let dir_field: usize = if is_interactive {
                3
            } else if is_terminal {
                1
            } else {
                6
            };
            let yolo_field: usize = 4; // interactive only
            let _ = (prompt_field, extra_field);

            match dialog.field {
                // Type selector (field 0)
                0 => match code {
                    KeyCode::Left => {
                        dialog.task_type = match dialog.task_type {
                            crate::tui::app::NewTaskType::Interactive => {
                                crate::tui::app::NewTaskType::Background
                            }
                            crate::tui::app::NewTaskType::Terminal => {
                                crate::tui::app::NewTaskType::Interactive
                            }
                            crate::tui::app::NewTaskType::Background => {
                                crate::tui::app::NewTaskType::Terminal
                            }
                        };
                        dialog.field = 0;
                        dialog.refresh_dir_entries();
                    }
                    KeyCode::Right => {
                        dialog.task_type = match dialog.task_type {
                            crate::tui::app::NewTaskType::Interactive => {
                                crate::tui::app::NewTaskType::Terminal
                            }
                            crate::tui::app::NewTaskType::Terminal => {
                                crate::tui::app::NewTaskType::Background
                            }
                            crate::tui::app::NewTaskType::Background => {
                                crate::tui::app::NewTaskType::Interactive
                            }
                        };
                        dialog.field = 0;
                        dialog.refresh_dir_entries();
                    }
                    KeyCode::Down | KeyCode::Tab => dialog.field = 1,
                    _ => {}
                },
                // Mode selector (Interactive only — field 1)
                1 if is_interactive => match code {
                    KeyCode::Left => {
                        dialog.task_mode = match dialog.task_mode {
                            crate::tui::app::NewTaskMode::Interactive => {
                                crate::tui::app::NewTaskMode::Resume
                            }
                            crate::tui::app::NewTaskMode::Resume => {
                                crate::tui::app::NewTaskMode::Interactive
                            }
                        };
                        dialog.selected_session = None;
                    }
                    KeyCode::Right => {
                        dialog.task_mode = match dialog.task_mode {
                            crate::tui::app::NewTaskMode::Interactive => {
                                crate::tui::app::NewTaskMode::Resume
                            }
                            crate::tui::app::NewTaskMode::Resume => {
                                crate::tui::app::NewTaskMode::Interactive
                            }
                        };
                        dialog.selected_session = None;
                    }
                    KeyCode::Delete | KeyCode::Backspace
                        if matches!(dialog.task_mode, crate::tui::app::NewTaskMode::Resume) =>
                    {
                        dialog.clear_selected_session();
                    }
                    KeyCode::Down | KeyCode::Tab => dialog.field = cli_field,
                    KeyCode::Up | KeyCode::BackTab => dialog.field = 0,
                    _ => {}
                },
                // Trigger selector (Background only — field 1)
                1 if is_background => match code {
                    KeyCode::Left | KeyCode::Right => {
                        dialog.background_trigger = match dialog.background_trigger {
                            crate::tui::app::BackgroundTrigger::Cron => {
                                crate::tui::app::BackgroundTrigger::Watch
                            }
                            crate::tui::app::BackgroundTrigger::Watch => {
                                crate::tui::app::BackgroundTrigger::Cron
                            }
                        };
                        dialog.refresh_dir_entries();
                    }
                    KeyCode::Down | KeyCode::Tab => dialog.field = cli_field,
                    KeyCode::Up | KeyCode::BackTab => dialog.field = 0,
                    _ => {}
                },
                // CLI field (field 2 for interactive/background) — Space opens picker
                n if n == cli_field && !is_terminal => match code {
                    KeyCode::Char(' ') => {
                        dialog.cli_picker_open = true;
                        dialog.cli_picker_idx = dialog.cli_index;
                    }
                    KeyCode::Left => {
                        let count = dialog.available_clis.len();
                        if count > 0 {
                            dialog.cli_index = (dialog.cli_index + count - 1) % count;
                            dialog.refresh_model_suggestions();
                            if dialog.selected_yolo_flag().is_none() {
                                dialog.yolo_mode = false;
                            }
                        }
                    }
                    KeyCode::Right => {
                        let count = dialog.available_clis.len();
                        if count > 0 {
                            dialog.cli_index = (dialog.cli_index + 1) % count;
                            dialog.refresh_model_suggestions();
                            if dialog.selected_yolo_flag().is_none() {
                                dialog.yolo_mode = false;
                            }
                        }
                    }
                    KeyCode::Down => {
                        if is_interactive {
                            dialog.field = yolo_field;
                        } else {
                            dialog.field = model_field;
                        }
                    }
                    KeyCode::Up => {
                        dialog.field = if is_interactive || is_background {
                            1
                        } else {
                            0
                        };
                    }
                    _ => {}
                },
                // Model field (Background only — field 3) — Space opens picker
                n if n == model_field && is_background => match code {
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
                        dialog.field = prompt_field;
                    }
                    _ => {}
                },
                // Prompt (Background only — field 4)
                4 if is_background => match code {
                    KeyCode::Char(c) => dialog.prompt.push(c),
                    KeyCode::Backspace => {
                        dialog.prompt.pop();
                    }
                    KeyCode::Up => dialog.field = model_field,
                    KeyCode::Down => dialog.field = extra_field,
                    _ => {}
                },
                // Cron expr (Background+Cron — field 5)
                5 if is_background
                    && matches!(
                        dialog.background_trigger,
                        crate::tui::app::BackgroundTrigger::Cron
                    ) =>
                {
                    match code {
                        KeyCode::Char(c) => dialog.cron_expr.push(c),
                        KeyCode::Backspace => {
                            dialog.cron_expr.pop();
                        }
                        KeyCode::Up => dialog.field = prompt_field,
                        KeyCode::Down => dialog.field = dir_field,
                        _ => {}
                    }
                }
                // Directory browser — ↑↓ navigate  → enter dir  ← go up  Space alias for →
                // For Background+Watch, dir_field == 6 but extra_field == 5 handles the path browser
                n if n == dir_field
                    || (n == extra_field
                        && is_background
                        && dialog.background_trigger
                            == crate::tui::app::BackgroundTrigger::Watch) =>
                {
                    let is_watch_dir = n == extra_field
                        && is_background
                        && dialog.background_trigger == crate::tui::app::BackgroundTrigger::Watch;
                    match code {
                        KeyCode::Up => {
                            if dialog.dir_selected > 0 {
                                dialog.dir_selected -= 1;
                            } else {
                                // At top of list: move focus to previous field
                                dialog.field = if is_watch_dir {
                                    prompt_field
                                } else if is_interactive {
                                    cli_field
                                } else if is_terminal {
                                    0
                                } else {
                                    extra_field
                                };
                            }
                        }
                        KeyCode::Down => {
                            let filtered_len = dialog.filtered_dir_entries().len();
                            if filtered_len > 0 && dialog.dir_selected + 1 < filtered_len {
                                dialog.dir_selected += 1;
                            } else {
                                // At bottom of list: move focus to next field
                                dialog.field = if is_interactive {
                                    yolo_field
                                } else if is_terminal {
                                    2 // shell field
                                } else {
                                    // Background: dir is the last field, stay
                                    dialog.field
                                };
                            }
                        }
                        KeyCode::BackTab => {
                            dialog.field = if is_watch_dir {
                                prompt_field
                            } else if is_interactive {
                                cli_field
                            } else if is_terminal {
                                0
                            } else {
                                extra_field
                            };
                        }
                        KeyCode::Tab => {
                            dialog.field = if is_interactive {
                                yolo_field
                            } else if is_terminal {
                                2 // shell field
                            } else {
                                // Background: dir is the last field, stay
                                dialog.field
                            };
                        }
                        KeyCode::Right => {
                            dialog.navigate_to_selected();
                        }
                        KeyCode::Left => {
                            dialog.go_up();
                        }
                        KeyCode::Backspace if !dialog.dir_filter.is_empty() => {
                            dialog.dir_filter.pop();
                            dialog.dir_selected = 0;
                        }
                        KeyCode::Char(c) if c != ' ' => {
                            dialog.dir_filter.push(c);
                            dialog.dir_selected = 0;
                        }
                        KeyCode::Char(' ') => {
                            dialog.navigate_to_selected();
                        }
                        _ => {}
                    }
                }
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
                // Yolo toggle (interactive only — field 4)
                n if n == yolo_field && is_interactive => match code {
                    KeyCode::Char(' ') if dialog.selected_yolo_flag().is_some() => {
                        dialog.yolo_mode = !dialog.yolo_mode;
                    }
                    KeyCode::Char(' ') => {}
                    KeyCode::Up | KeyCode::BackTab => {
                        dialog.field = cli_field;
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
