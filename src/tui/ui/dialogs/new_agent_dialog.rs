use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::{centered_rect, truncate_str, DIM};
use crate::tui::app::App;

pub fn draw_new_agent_dialog(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.new_agent_dialog else {
        return;
    };

    let accent = dialog.selected_accent_color();

    let is_interactive = matches!(dialog.task_type, crate::tui::app::NewTaskType::Interactive);
    let is_terminal = matches!(dialog.task_type, crate::tui::app::NewTaskType::Terminal);
    let is_background = matches!(dialog.task_type, crate::tui::app::NewTaskType::Background);

    let cli_picker_rows: u16 = if dialog.cli_picker_open {
        let visible = dialog.available_clis.len().min(6);
        visible as u16 + 1
    } else {
        0
    };
    let model_picker_rows: u16 = if dialog.model_picker_open && !dialog.model_suggestions.is_empty()
    {
        let visible = dialog.model_suggestions.len().min(5);
        let overflow_line = if dialog.model_suggestions.len() > 5 {
            1
        } else {
            0
        };
        (visible + overflow_line) as u16
    } else {
        0
    };

    // Dir browser: label row + filter row + up to 10 entry rows + status line
    let filtered_entries = dialog.filtered_dir_entries();
    let dir_rows: u16 = if filtered_entries.is_empty() && dialog.dir_entries.is_empty() {
        0
    } else {
        3 + filtered_entries.len().min(10) as u16
    };

    let is_edit = dialog.is_edit_mode();

    // Base heights
    let base_height: u16 = if is_interactive {
        12 + dir_rows // type, mode, cli, dir, yolo, help
    } else if is_terminal {
        10 + dir_rows // type, dir, shell, help
    } else {
        // background: type, trigger, cli, model, prompt, cron/watch, dir, help
        15 + dir_rows
    };
    let height = base_height + cli_picker_rows + model_picker_rows;
    let area = centered_rect(65, height, frame.area());
    frame.render_widget(Clear, area);

    let title = if is_edit {
        match dialog.task_type {
            crate::tui::app::NewTaskType::Background => " Edit Background ",
            crate::tui::app::NewTaskType::Interactive => " Edit Agent ",
            crate::tui::app::NewTaskType::Terminal => " Edit Terminal ",
        }
    } else {
        " New Agent "
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let type_names = ["Interactive", "Terminal", "Background"];
    let type_idx = match dialog.task_type {
        crate::tui::app::NewTaskType::Interactive => 0,
        crate::tui::app::NewTaskType::Terminal => 1,
        crate::tui::app::NewTaskType::Background => 2,
    };

    let is_focused = |field: usize| dialog.field == field;
    let focus_style = |field: usize| {
        if is_focused(field) {
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        }
    };

    let cli_binding = dialog.selected_cli();
    let cli_name = cli_binding.as_str();

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
    let extra_field: usize = 5; // background only (cron/watch)
    let dir_field: usize = if is_interactive {
        3
    } else if is_terminal {
        1
    } else {
        6
    };
    let yolo_field: usize = 4; // interactive only

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Type:  ", Style::default().fg(DIM)),
            if is_edit {
                Span::styled(
                    format!("  {}  ", type_names[type_idx]),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled(format!(" ◀ {} ▶ ", type_names[type_idx]), focus_style(0))
            },
        ]),
        Line::from(""),
    ];

    // Session/mode row — only for interactive, and hidden in edit mode
    if is_interactive && !is_edit {
        let mode_names = ["New", "Resume"];
        let mode_idx = match dialog.task_mode {
            crate::tui::app::NewTaskMode::Interactive => 0,
            crate::tui::app::NewTaskMode::Resume => 1,
        };
        let mut session_line = vec![
            Span::styled("  Session:  ", Style::default().fg(DIM)),
            Span::styled(format!(" ◀ {} ▶ ", mode_names[mode_idx]), focus_style(1)),
        ];
        if dialog.resume_unconfigured() && !dialog.has_session_picker() {
            session_line.push(Span::styled(
                "  (not configured — falls back to new)",
                Style::default().fg(Color::Yellow),
            ));
        }
        if matches!(dialog.task_mode, crate::tui::app::NewTaskMode::Resume)
            && dialog.has_session_picker()
        {
            let session_label = match &dialog.selected_session {
                Some((_, title)) => {
                    let short = if title.len() > 40 {
                        format!("{}…", &title[..40])
                    } else {
                        title.clone()
                    };
                    format!("  ↵ pick  [{short}]")
                }
                None => "  ↵ pick session  (latest)".to_string(),
            };
            session_line.push(Span::styled(
                session_label,
                Style::default().fg(Color::Cyan),
            ));
        }
        lines.push(Line::from(session_line));
        lines.push(Line::from(""));
    }

    // For Terminal type, show only Dir + Shell fields
    if is_terminal {
        let term_dir_field = 1usize;
        let term_shell_field = 2usize;

        lines.push(Line::from(vec![
            Span::styled("  Dir:   ", Style::default().fg(DIM)),
            Span::styled(
                truncate_str(&dialog.working_dir, 50),
                focus_style(term_dir_field),
            ),
        ]));
        lines.push(Line::from(""));

        // Directory browser for terminal
        if !dialog.dir_entries.is_empty() {
            let filtered = dialog.filtered_dir_entries();
            let filter_display = if dialog.dir_filter.is_empty() {
                "type to filter".to_string()
            } else {
                dialog.dir_filter.clone()
            };
            lines.push(Line::from(vec![
                Span::styled(
                    "  🔍 ",
                    if dialog.field == term_dir_field {
                        Style::default().fg(accent)
                    } else {
                        Style::default().fg(DIM)
                    },
                ),
                Span::styled(
                    filter_display,
                    if dialog.dir_filter.is_empty() {
                        Style::default().fg(DIM)
                    } else {
                        Style::default().fg(Color::White)
                    },
                ),
            ]));

            let visible_rows = 10;
            let scroll = if dialog.dir_selected >= visible_rows {
                dialog.dir_selected - visible_rows + 1
            } else {
                0
            };
            let has_above = scroll > 0;
            let has_below = !filtered.is_empty() && scroll + visible_rows < filtered.len();

            if filtered.is_empty() {
                lines.push(Line::from(Span::styled(
                    "    (no matches)",
                    Style::default().fg(DIM),
                )));
            } else {
                for (i, entry) in filtered.iter().enumerate().skip(scroll).take(visible_rows) {
                    let is_selected = i == dialog.dir_selected;
                    let entry_style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    lines.push(Line::from(Span::styled(
                        format!("    {entry}"),
                        entry_style,
                    )));
                }
            }

            let up = if has_above { "↑ " } else { "  " };
            let dn = if has_below { " ↓" } else { "  " };
            if filtered.is_empty() {
                lines.push(Line::from(Span::styled(
                    "    0 items",
                    Style::default().fg(DIM),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("    {up}{}/{}{dn}", dialog.dir_selected + 1, filtered.len()),
                    Style::default().fg(DIM),
                )));
            }
            lines.push(Line::from(""));
        }

        let selected_shell = dialog.selected_shell();
        let shell_display = if dialog.available_shells.len() > 1 {
            format!("◂ {} ▸", selected_shell)
        } else {
            selected_shell.to_string()
        };
        lines.push(Line::from(vec![
            Span::styled("  Shell: ", Style::default().fg(DIM)),
            Span::styled(
                format!(" {} ", shell_display),
                focus_style(term_shell_field),
            ),
        ]));
        lines.push(Line::from(""));

        lines.push(Line::from(Span::styled(
            "  ↑↓: fields  (in dirs: → enter  ← up) · Enter: launch · Esc: cancel",
            Style::default().fg(DIM),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    // ── Trigger row (Background only) ──
    if is_background && !is_edit {
        let trigger_names = ["Cron", "Watch"];
        let trigger_idx = match dialog.background_trigger {
            crate::tui::app::BackgroundTrigger::Cron => 0,
            crate::tui::app::BackgroundTrigger::Watch => 1,
        };
        lines.push(Line::from(vec![
            Span::styled("  Trigger:", Style::default().fg(DIM)),
            Span::styled(
                format!(" ◀ {} ▶ ", trigger_names[trigger_idx]),
                focus_style(1),
            ),
        ]));
        lines.push(Line::from(""));
    } else if is_background && is_edit {
        let trigger_label = match dialog.background_trigger {
            crate::tui::app::BackgroundTrigger::Cron => "Cron",
            crate::tui::app::BackgroundTrigger::Watch => "Watch",
        };
        lines.push(Line::from(vec![
            Span::styled("  Trigger:", Style::default().fg(DIM)),
            Span::styled(
                format!("  {}  ", trigger_label),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));
    }

    // ── CLI row (not for terminal) ──
    lines.push(Line::from(vec![
        Span::styled("  CLI:   ", Style::default().fg(DIM)),
        Span::styled(format!(" {} ", cli_name), focus_style(cli_field)),
        Span::styled("  (◂▸ cycle · Space pick)", Style::default().fg(DIM)),
    ]));

    // CLI picker dropdown
    if dialog.cli_picker_open {
        let max_visible = 6;
        let total = dialog.available_clis.len();
        let sel = dialog.cli_picker_idx;
        let scroll = if sel >= max_visible {
            sel - max_visible + 1
        } else {
            0
        };
        for (i, cli) in dialog
            .available_clis
            .iter()
            .enumerate()
            .skip(scroll)
            .take(max_visible)
        {
            let is_sel = i == sel;
            let style = if is_sel {
                Style::default()
                    .fg(Color::Black)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::from(vec![
                Span::styled(format!("    {} ", if is_sel { "›" } else { " " }), style),
                Span::styled(cli.as_str().to_string(), style),
            ]));
        }
        if total > max_visible {
            lines.push(Line::from(Span::styled(
                format!("    … {total} CLIs  ↑↓ scroll  Enter/Esc close"),
                Style::default().fg(DIM),
            )));
        }
    }

    lines.push(Line::from(""));

    // ── Model row (Background only, not Interactive) ──
    if is_background {
        lines.push(Line::from(vec![
            Span::styled("  Model: ", Style::default().fg(DIM)),
            Span::styled(
                if dialog.model.is_empty() {
                    "(optional — Space to browse)".to_string()
                } else {
                    format!("{}▏", dialog.model)
                },
                focus_style(model_field),
            ),
        ]));

        // Model suggestions dropdown
        if is_focused(model_field)
            && dialog.model_picker_open
            && !dialog.model_suggestions.is_empty()
        {
            let max_visible = 5;
            let total = dialog.model_suggestions.len();
            let sel = dialog.model_suggestion_idx;
            let scroll = if sel >= max_visible {
                sel - max_visible + 1
            } else {
                0
            };
            for (i, entry) in dialog
                .model_suggestions
                .iter()
                .enumerate()
                .skip(scroll)
                .take(max_visible)
            {
                let is_sel = i == sel;
                let style = if is_sel {
                    Style::default()
                        .fg(Color::Black)
                        .bg(accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let provider_tag = format!(" [{}]", entry.provider);
                lines.push(Line::from(vec![
                    Span::styled(format!("    {} ", if is_sel { "›" } else { " " }), style),
                    Span::styled(truncate_str(&entry.id, 38), style),
                    Span::styled(
                        provider_tag,
                        if is_sel {
                            style
                        } else {
                            Style::default().fg(DIM)
                        },
                    ),
                ]));
            }
            if total > max_visible {
                lines.push(Line::from(Span::styled(
                    format!("    … {total} models  ↑↓ scroll  → accept  Esc close"),
                    Style::default().fg(DIM),
                )));
            }
        }
        lines.push(Line::from(""));
    }

    // Session picker dropdown — shown when session_picker_open (interactive only)
    if dialog.session_picker_open {
        let max_visible = 6;
        let total = dialog.session_entries.len();
        let sel = dialog.session_picker_idx;
        let scroll = if sel >= max_visible {
            sel - max_visible + 1
        } else {
            0
        };

        if total == 0 {
            lines.push(Line::from(Span::styled(
                "    (no sessions found)",
                Style::default().fg(DIM),
            )));
        } else {
            for (i, (id, label)) in dialog
                .session_entries
                .iter()
                .enumerate()
                .skip(scroll)
                .take(max_visible)
            {
                let is_sel = i == sel;
                let style = if is_sel {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let short_label = truncate_str(label, 36);
                lines.push(Line::from(vec![
                    Span::styled(format!("    {} ", if is_sel { "›" } else { " " }), style),
                    Span::styled(truncate_str(id, 18), style),
                    Span::styled(
                        format!("  {short_label}"),
                        if is_sel {
                            style
                        } else {
                            Style::default().fg(DIM)
                        },
                    ),
                ]));
            }
            if total > max_visible {
                lines.push(Line::from(Span::styled(
                    format!("    … {total} sessions  ↑↓ scroll  Enter accept  Esc close"),
                    Style::default().fg(DIM),
                )));
            }
        }
    }

    // ── Prompt (Background only) ──
    if is_background {
        lines.push(Line::from(vec![
            Span::styled("  Prompt:", Style::default().fg(DIM)),
            Span::styled(
                if dialog.prompt.is_empty() {
                    " enter agent prompt...".to_string()
                } else {
                    format!(" {}▏", dialog.prompt)
                },
                focus_style(prompt_field),
            ),
        ]));
        lines.push(Line::from(""));

        // Cron expr or Watch path
        if dialog.background_trigger == crate::tui::app::BackgroundTrigger::Cron {
            lines.push(Line::from(vec![
                Span::styled("  Cron:  ", Style::default().fg(DIM)),
                Span::styled(
                    if dialog.cron_expr.is_empty() {
                        " * * * * *  (min hr dom mon dow)".to_string()
                    } else {
                        format!(" {}▏", dialog.cron_expr)
                    },
                    focus_style(extra_field),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("  Path:  ", Style::default().fg(DIM)),
                Span::styled(
                    truncate_str(&dialog.watch_path, 50),
                    focus_style(extra_field),
                ),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Yolo mode toggle — only for interactive agents
    if is_interactive {
        let has_yolo = dialog.selected_yolo_flag().is_some();
        let checkbox = if dialog.yolo_mode { "◉" } else { "○" };
        let checkbox_style = if dialog.field == yolo_field {
            Style::default()
                .fg(Color::Black)
                .bg(accent)
                .add_modifier(Modifier::BOLD)
        } else if dialog.yolo_mode {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let mut yolo_spans = vec![
            Span::styled("  Yolo:  ", Style::default().fg(DIM)),
            Span::styled(format!("{checkbox} Autonomous mode"), checkbox_style),
        ];
        if !has_yolo {
            yolo_spans.push(Span::styled(
                "  (not supported by this CLI)",
                Style::default().fg(DIM),
            ));
        } else if dialog.yolo_mode {
            yolo_spans.push(Span::styled(
                "  ⚠ agent acts without approval",
                Style::default().fg(Color::Yellow),
            ));
        }
        lines.push(Line::from(yolo_spans));
        lines.push(Line::from(""));
    }

    // Working directory — hide for Watch background (uses Path field)
    let hide_dir =
        is_background && dialog.background_trigger == crate::tui::app::BackgroundTrigger::Watch;
    if !hide_dir {
        lines.push(Line::from(vec![
            Span::styled("  Dir:   ", Style::default().fg(DIM)),
            Span::styled(
                truncate_str(&dialog.working_dir, 50),
                focus_style(dir_field),
            ),
        ]));
        lines.push(Line::from(""));
    }

    // Directory / file browser
    if !dialog.dir_entries.is_empty() {
        let filtered = dialog.filtered_dir_entries();
        let is_watch =
            is_background && dialog.background_trigger == crate::tui::app::BackgroundTrigger::Watch;
        let browser_field_idx = if is_watch { extra_field } else { dir_field };

        let filter_display = if dialog.dir_filter.is_empty() {
            "type to filter".to_string()
        } else {
            dialog.dir_filter.clone()
        };
        lines.push(Line::from(vec![
            Span::styled(
                "  🔍 ",
                if is_focused(browser_field_idx) {
                    Style::default().fg(accent)
                } else {
                    Style::default().fg(DIM)
                },
            ),
            Span::styled(
                filter_display,
                if dialog.dir_filter.is_empty() {
                    Style::default().fg(DIM)
                } else {
                    Style::default().fg(Color::White)
                },
            ),
        ]));

        let visible_rows = 10;
        let scroll = if dialog.dir_selected >= visible_rows {
            dialog.dir_selected - visible_rows + 1
        } else {
            0
        };
        let has_above = scroll > 0;
        let has_below = !filtered.is_empty() && scroll + visible_rows < filtered.len();

        if filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "    (no matches)",
                Style::default().fg(DIM),
            )));
        } else {
            for (i, entry) in filtered.iter().enumerate().skip(scroll).take(visible_rows) {
                let is_selected = i == dialog.dir_selected;
                let entry_style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                lines.push(Line::from(Span::styled(
                    format!("    {entry}"),
                    entry_style,
                )));
            }
        }

        let up = if has_above { "↑ " } else { "  " };
        let dn = if has_below { " ↓" } else { "  " };
        if filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                "    0 items",
                Style::default().fg(DIM),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("    {up}{}/{}{dn}", dialog.dir_selected + 1, filtered.len()),
                Style::default().fg(DIM),
            )));
        }
        lines.push(Line::from(""));
    }

    let help_text = if is_interactive {
        "  ↑↓: fields · ←→: mode  (in dirs: → enter  ← up) · Enter: launch · Esc: cancel"
    } else if is_background {
        "  ↑↓: fields · ←→: trigger  (in dirs: → enter  ← up) · Enter: create · Esc: cancel"
    } else {
        "  ↑↓: fields  (in dirs: → enter  ← up) · Enter: launch · Esc: cancel"
    };

    lines.push(Line::from(Span::styled(
        help_text,
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}
