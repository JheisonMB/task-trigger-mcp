//! Dialog overlays — new agent, quit confirmation, color legend, context transfer.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::{centered_rect, truncate_str};
use super::{ACCENT, DIM, STATUS_DISABLED, STATUS_FAIL, STATUS_OK, STATUS_RUNNING};
use crate::tui::app::App;
use crate::tui::context_transfer::ContextTransferStep;

pub(super) fn draw_new_agent_dialog(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.new_agent_dialog else {
        return;
    };

    let accent = dialog.selected_accent_color();

    let picker_rows = if dialog.model_picker_open && !dialog.model_suggestions.is_empty() {
        let visible = dialog.model_suggestions.len().min(5);
        let overflow_line = if dialog.model_suggestions.len() > 5 {
            1
        } else {
            0
        };
        visible + overflow_line
    } else {
        0
    };

    // Dir browser: label row + up to 4 entry rows
    let dir_rows: u16 = if dialog.dir_entries.is_empty() {
        0
    } else {
        1 + dialog.dir_entries.len().min(4) as u16
    };

    // Base heights: fields + 2 borders (no browser rows).
    // Interactive:    11 content rows → base 13
    // Scheduled/Watcher: 13 content rows (extra Prompt + Cron/Path) → base 15
    let base_height: u16 = match dialog.task_type {
        crate::tui::app::NewTaskType::Interactive => 13 + dir_rows,
        crate::tui::app::NewTaskType::Scheduled | crate::tui::app::NewTaskType::Watcher => {
            15 + dir_rows
        }
    };
    let height = base_height + picker_rows as u16;
    let area = centered_rect(65, height, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" New Agent ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let type_names = ["Interactive", "Scheduled", "Watcher"];
    let type_idx = match dialog.task_type {
        crate::tui::app::NewTaskType::Interactive => 0,
        crate::tui::app::NewTaskType::Scheduled => 1,
        crate::tui::app::NewTaskType::Watcher => 2,
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

    let mode_names = ["New", "Resume"];
    let mode_idx = match dialog.task_mode {
        crate::tui::app::NewTaskMode::Interactive => 0,
        crate::tui::app::NewTaskMode::Resume => 1,
    };
    let mode_field = 1;

    let is_interactive = matches!(dialog.task_type, crate::tui::app::NewTaskType::Interactive);
    let cli_field = if is_interactive { 2 } else { 1 };

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Type:  ", Style::default().fg(DIM)),
            Span::styled(format!(" ◀ {} ▶ ", type_names[type_idx]), focus_style(0)),
        ]),
        Line::from(""),
    ];

    if is_interactive {
        let mut session_line = vec![
            Span::styled("  Session:  ", Style::default().fg(DIM)),
            Span::styled(
                format!(" ◀ {} ▶ ", mode_names[mode_idx]),
                focus_style(mode_field),
            ),
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

    lines.push(Line::from(vec![
        Span::styled("  CLI:   ", Style::default().fg(DIM)),
        if is_focused(cli_field) {
            Span::styled(format!(" ◀ {cli_name} ▶ "), focus_style(cli_field))
        } else {
            Span::styled(
                format!(" ◀ {cli_name} ▶ "),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            )
        },
    ]));
    lines.push(Line::from(""));

    let model_field = if is_interactive { 3 } else { 2 };
    let prompt_field = 3usize; // non-interactive only (field 3)
    let extra_field = 4usize; // non-interactive only (field 4)
    let dir_field = if is_interactive { 4 } else { 5 };

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
    if is_focused(model_field) && dialog.model_picker_open && !dialog.model_suggestions.is_empty() {
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

    // Session picker dropdown — shown when session_picker_open
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
                let short_label = if label.len() > 36 {
                    format!("{}…", &label[..36])
                } else {
                    label.clone()
                };
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

    lines.push(Line::from(""));

    // Prompt + extra fields for non-interactive background_agents (before Dir)
    if matches!(
        dialog.task_type,
        crate::tui::app::NewTaskType::Scheduled | crate::tui::app::NewTaskType::Watcher
    ) {
        lines.push(Line::from(vec![
            Span::styled("  Prompt:", Style::default().fg(DIM)),
            Span::styled(
                if dialog.prompt.is_empty() {
                    " enter background_agent prompt...".to_string()
                } else {
                    format!(" {}▏", dialog.prompt)
                },
                focus_style(prompt_field),
            ),
        ]));
        lines.push(Line::from(""));

        if dialog.task_type == crate::tui::app::NewTaskType::Scheduled {
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

    // Only show working directory when not creating a Watcher. Watchers use
    // the 'Path' field to select files or directories to watch, which is
    // displayed above as 'Path'. Hiding Dir avoids confusion.
    if dialog.task_type != crate::tui::app::NewTaskType::Watcher {
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
        let is_watcher = dialog.task_type == crate::tui::app::NewTaskType::Watcher;
        let browser_label = if is_watcher {
            "  Browse  (↑↓ navigate, Space to select):"
        } else {
            "  Directories  (↑↓ navigate, Space to enter):"
        };
        // Browser label uses the selected CLI's accent color for emphasis
        let browser_field_idx = if is_watcher { extra_field } else { dir_field };
        lines.push(Line::from(Span::styled(
            browser_label,
            if is_focused(browser_field_idx) {
                Style::default().fg(accent)
            } else {
                Style::default().fg(DIM)
            },
        )));

        let visible_rows = 4;
        let scroll = dialog.dir_selected.saturating_sub(visible_rows - 1);

        for (i, entry) in dialog.dir_entries.iter().enumerate().skip(scroll) {
            if i >= scroll + visible_rows {
                break;
            }

            let is_selected = i == dialog.dir_selected;
            // Always highlight the selected entry so the user always sees the cursor.
            let entry_style = if is_selected {
                // Use the CLI-specific accent color for selection background so the
                // browser matches the agent's emphasis color.
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
        lines.push(Line::from(""));
    }

    let help_text = match dialog.task_type {
        crate::tui::app::NewTaskType::Interactive => {
            "  ↑↓: fields · ←→: CLI/mode · Space: navigate dirs · Enter: launch · Esc: cancel"
        }
        crate::tui::app::NewTaskType::Scheduled => {
            "  ↑↓: fields · ←→: type/CLI · Space: navigate dirs · Enter: create · Esc: cancel"
        }
        crate::tui::app::NewTaskType::Watcher => {
            "  ↑↓: fields · ←→: type/CLI · Space: navigate dirs · Enter: create · Esc: cancel"
        }
    };

    lines.push(Line::from(Span::styled(
        help_text,
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn draw_quit_confirm(frame: &mut Frame) {
    let area = centered_rect(40, 3, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Quit? ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let msg = Paragraph::new("Press y/Enter to quit, any key to cancel")
        .style(Style::default().fg(ACCENT))
        .alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(msg, inner);
}

pub(super) fn draw_legend(frame: &mut Frame) {
    let area = centered_rect(32, 10, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Color Legend ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines = vec![
        Line::from(vec![
            Span::styled("▌ ", Style::default().fg(STATUS_RUNNING)),
            Span::styled(
                "RUNNING  ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Agent is executing", Style::default().fg(DIM)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("▌ ", Style::default().fg(STATUS_OK)),
            Span::styled(
                "OK/IDLE  ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Agent ready / last run OK", Style::default().fg(DIM)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("▌ ", Style::default().fg(STATUS_FAIL)),
            Span::styled(
                "FAILED   ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Last run failed / error exit", Style::default().fg(DIM)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("▌ ", Style::default().fg(STATUS_DISABLED)),
            Span::styled(
                "DISABLED ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Agent is paused", Style::default().fg(DIM)),
        ]),
    ];

    frame.render_widget(Paragraph::new(lines), inner);
}

// ── Context Transfer modal ───────────────────────────────────────

pub(super) fn draw_context_transfer_modal(frame: &mut Frame, app: &App) {
    let Some(modal) = &app.context_transfer_modal else {
        return;
    };

    match modal.step {
        ContextTransferStep::Preview => draw_ctx_preview(frame, app),
        ContextTransferStep::AgentPicker => draw_ctx_picker(frame, app),
    }
}

fn draw_ctx_preview(frame: &mut Frame, app: &App) {
    let Some(modal) = &app.context_transfer_modal else {
        return;
    };

    let preview_lines: Vec<&str> = modal.payload_preview.lines().collect();
    let visible_preview = preview_lines.len().min(8) as u16;
    let height = 12 + visible_preview;
    let area = centered_rect(70, height, frame.area());
    frame.render_widget(Clear, area);

    let src_id = app
        .interactive_agents
        .get(modal.source_agent_idx)
        .map(|a| a.id.as_str())
        .unwrap_or("?");

    let block = Block::default()
        .title(format!(" Context Transfer — from: {src_id} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let focus_style = |active: bool| {
        if active {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        }
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Prompts:   ", Style::default().fg(DIM)),
            Span::styled(
                format!(" ◀ {} ▶ ", modal.n_prompts),
                focus_style(modal.preview_field == 0),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Scrollback:", Style::default().fg(DIM)),
            Span::styled(
                format!(" ◀ {} lines ▶ ", modal.scrollback_lines),
                focus_style(modal.preview_field == 1),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Preview:", Style::default().fg(DIM))),
    ];

    for line in preview_lines.iter().take(8) {
        lines.push(Line::from(Span::styled(
            format!("  {}", truncate_str(line, 60)),
            Style::default().fg(Color::Rgb(170, 200, 170)),
        )));
    }
    if preview_lines.len() > 8 {
        lines.push(Line::from(Span::styled(
            format!("  … {} more lines", preview_lines.len() - 8),
            Style::default().fg(DIM),
        )));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓: field · ←→: adjust · Enter: pick destination · Esc: cancel",
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_ctx_picker(frame: &mut Frame, app: &App) {
    let Some(modal) = &app.context_transfer_modal else {
        return;
    };

    let agents = &app.interactive_agents;
    let visible = agents.len().min(8) as u16;
    let height = 6 + visible.max(1);
    let area = centered_rect(60, height, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Select Destination Agent ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![Line::from("")];

    if agents.is_empty() {
        lines.push(Line::from(Span::styled(
            "  No interactive agents running.",
            Style::default().fg(DIM),
        )));
    } else {
        for (i, agent) in agents.iter().enumerate() {
            let is_sel = i == modal.picker_selected;
            let is_src = i == modal.source_agent_idx;
            let label = if is_src {
                format!(
                    "  {} {}  (source)",
                    if is_sel { "›" } else { " " },
                    agent.id
                )
            } else {
                format!("  {} {}", if is_sel { "›" } else { " " }, agent.id)
            };
            let style = if is_sel {
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else if is_src {
                Style::default().fg(DIM)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::from(Span::styled(label, style)));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓: navigate · Enter: transfer · Esc: back",
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

// ── suppress unused import warning until all variants are referenced ──
#[allow(unused_imports)]
use super::{BG_SELECTED, ERROR_COLOR, INTERACTIVE_COLOR};
