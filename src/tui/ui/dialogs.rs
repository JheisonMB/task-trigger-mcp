//! Dialog overlays — new agent, quit confirmation, color legend, context transfer.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use super::{centered_rect, truncate_str};
use super::{
    ACCENT, DIM, STATUS_DISABLED, STATUS_FAIL, STATUS_OK, STATUS_RUNNING, STATUS_WAIT_OFF,
    STATUS_WAIT_ON,
};
use crate::tui::app::dialog::SimplePromptDialog;
use crate::tui::app::{AgentEntry, App};
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

    let is_edit = dialog.is_edit_mode();

    // In edit mode: height is the same as the task type's base (no session row)
    // Base heights: fields + 2 borders (no browser rows).
    // Interactive:    11 content rows → base 13
    // Scheduled/Watcher: 13 content rows (extra Prompt + Cron/Path) → base 15
    let base_height: u16 = match dialog.task_type {
        crate::tui::app::NewTaskType::Interactive => 17 + dir_rows,
        crate::tui::app::NewTaskType::Scheduled | crate::tui::app::NewTaskType::Watcher => {
            15 + dir_rows
        }
    };
    let height = base_height + picker_rows as u16;
    let area = centered_rect(65, height, frame.area());
    frame.render_widget(Clear, area);

    let title = if is_edit {
        match dialog.task_type {
            crate::tui::app::NewTaskType::Scheduled => " Edit Task ",
            crate::tui::app::NewTaskType::Watcher => " Edit Watcher ",
            crate::tui::app::NewTaskType::Interactive => " Edit Agent ",
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
    let name_field = 2usize; // interactive only
    let cli_field = if is_interactive { 3 } else { 1 };

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Type:  ", Style::default().fg(DIM)),
            if is_edit {
                // Locked in edit mode — show type without arrow affordance
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

    // Name row — only for interactive, hidden in edit mode
    if is_interactive && !is_edit {
        lines.push(Line::from(vec![
            Span::styled("  Name:  ", Style::default().fg(DIM)),
            Span::styled(
                if dialog.agent_name.is_empty() {
                    " (optional — random if empty)".to_string()
                } else {
                    format!(" {}▏", dialog.agent_name)
                },
                focus_style(name_field),
            ),
        ]));
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

    let model_field = if is_interactive { 4 } else { 2 };
    let prompt_field = 3usize; // non-interactive only (field 3)
    let extra_field = 4usize; // non-interactive only (field 4)
    let dir_field = if is_interactive {
        5
    } else if dialog.task_type == crate::tui::app::NewTaskType::Watcher {
        4
    } else {
        5
    };

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

    // Yolo mode toggle — only for interactive agents, shown before the dir browser
    if is_interactive {
        let has_yolo = dialog.selected_yolo_flag().is_some();
        let checkbox = if dialog.yolo_mode { "◉" } else { "○" };
        let yolo_field = 6usize;
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
            "  Directories  (↑↓ navigate  → enter  ← go up):"
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
            "  ↑↓: fields · ←→: CLI/mode  (in dirs: → enter  ← up) · Enter: launch · Esc: cancel"
        }
        crate::tui::app::NewTaskType::Scheduled => {
            "  ↑↓: fields · ←→: type/CLI  (in dirs: → enter  ← up) · Enter: create · Esc: cancel"
        }
        crate::tui::app::NewTaskType::Watcher => {
            "  ↑↓: fields · ←→: type/CLI · Space: select · Enter: create · Esc: cancel"
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

pub(super) fn draw_legend(frame: &mut Frame, app: &App) {
    let area = centered_rect(32, 12, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Color Legend ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let blink_cycle = (app.animation_tick / 10) % 2;
    let wait_color = if blink_cycle == 0 {
        STATUS_WAIT_ON
    } else {
        STATUS_WAIT_OFF
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("▌ ", Style::default().fg(STATUS_RUNNING)),
            Span::styled(
                "RUNNING   ",
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
                "OK/IDLE   ",
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
                "FAILED    ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Last run failed / error exit", Style::default().fg(DIM)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("▌ ", Style::default().fg(wait_color)),
            Span::styled(
                "ATTENTION ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Waiting for user input", Style::default().fg(DIM)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("▌ ", Style::default().fg(STATUS_DISABLED)),
            Span::styled(
                "DISABLED  ",
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
    let height = 10 + visible_preview;
    let area = centered_rect(70, height, frame.area());
    frame.render_widget(Clear, area);

    let (src_id, accent) = app
        .interactive_agents
        .get(modal.source_agent_idx)
        .map(|a| (a.name.as_str(), a.accent_color))
        .unwrap_or(("?", ACCENT));

    let block = Block::default()
        .title(format!(" Context Transfer — from: {src_id} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let active_style = Style::default()
        .fg(Color::Black)
        .bg(accent)
        .add_modifier(Modifier::BOLD);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  From prompt: ", Style::default().fg(DIM)),
            Span::styled(format!(" ◀ {} ▶ ", modal.n_prompts), active_style),
            Span::styled(
                "  (most recent N prompts + responses)",
                Style::default().fg(DIM),
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
        "  ←→: adjust prompts · Enter: pick destination · Esc: cancel",
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_ctx_picker(frame: &mut Frame, app: &App) {
    let Some(modal) = &app.context_transfer_modal else {
        return;
    };

    let agents = &app.interactive_agents;
    let card_h = 3u16;
    let visible_cards = agents.len().min(5) as u16;
    let list_h = if agents.is_empty() {
        1
    } else {
        visible_cards * card_h + visible_cards.saturating_sub(1)
    };
    let height = 4 + list_h + 2;
    let area = centered_rect(66, height, frame.area());
    frame.render_widget(Clear, area);

    let src_accent = app
        .interactive_agents
        .get(modal.source_agent_idx)
        .map(|a| a.accent_color)
        .unwrap_or(ACCENT);

    let block = Block::default()
        .title(" Select Destination Agent ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(src_accent))
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
            if i > 0 {
                lines.push(Line::from(""));
            }
            let is_sel = i == modal.picker_selected;
            let is_src = i == modal.source_agent_idx;

            let bar_color = if is_src { DIM } else { agent.accent_color };
            let accent = agent.accent_color;
            let id_color = if is_sel {
                Color::Black
            } else if is_src {
                DIM
            } else {
                Color::White
            };
            let bg = if is_sel {
                accent
            } else {
                Color::Rgb(15, 25, 15)
            };

            let cursor = if is_sel { "›" } else { " " };
            let src_tag = if is_src { "  (source)" } else { "" };

            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {} ", cursor),
                    Style::default().fg(bar_color).bg(bg),
                ),
                Span::styled(
                    format!("{}{}", agent.name, src_tag),
                    Style::default()
                        .fg(id_color)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));

            lines.push(Line::from(vec![
                Span::styled("    ", Style::default().bg(bg)),
                Span::styled(
                    format!("pty · {}", agent.cli.as_str()),
                    Style::default().fg(DIM).bg(bg),
                ),
            ]));

            let dir = truncate_path(&agent.working_dir, inner.width.saturating_sub(6) as usize);
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default().bg(bg)),
                Span::styled(dir, Style::default().fg(Color::Cyan).bg(bg)),
            ]));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ↑↓ navigate · Enter transfer · Esc back",
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn truncate_path(path: &str, max_chars: usize) -> String {
    if path.len() <= max_chars {
        return path.to_string();
    }
    let trimmed = &path[path.len() - max_chars.saturating_sub(1)..];
    let start = trimmed.find('/').map(|p| p + 1).unwrap_or(0);
    format!("…/{}", &trimmed[start..])
}

#[allow(unused_imports)]
use super::{BG_SELECTED, ERROR_COLOR, INTERACTIVE_COLOR};

// Old function removed - using simple prompt dialog instead

fn draw_section_picker_modal(
    frame: &mut Frame,
    app: &App,
    accent: Color,
    mode: &crate::tui::app::dialog::SectionPickerMode,
) {
    use crate::tui::app::dialog::SectionPickerMode;

    let Some(dialog) = &app.simple_prompt_dialog else {
        return;
    };

    match mode {
        SectionPickerMode::AddSection { selected } => {
            let addable = dialog.get_addable_sections();
            let height = (addable.len() as u16 + 4).min(15);
            let area = centered_rect(50, height, frame.area());
            frame.render_widget(Clear, area);

            let title = " Add Section ";
            let block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent))
                .style(Style::default().bg(Color::Rgb(15, 25, 15)));

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let mut y_pos = inner.y;
            for (i, (_, label)) in addable.iter().enumerate() {
                let is_selected = i == *selected;
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let line = Line::from(vec![Span::styled(format!("  {} ", label), style)]);
                let line_area = ratatui::layout::Rect {
                    x: inner.x,
                    y: y_pos,
                    width: inner.width,
                    height: 1,
                };
                frame.render_widget(Paragraph::new(line), line_area);
                y_pos += 1;
            }

            let hint = Line::from(vec![
                Span::styled("↑↓ ", Style::default().fg(DIM)),
                Span::styled("select  ", Style::default().fg(Color::White)),
                Span::styled("c ", Style::default().fg(DIM)),
                Span::styled("custom  ", Style::default().fg(Color::White)),
                Span::styled("Enter ", Style::default().fg(DIM)),
                Span::styled("add  ", Style::default().fg(Color::White)),
                Span::styled("Esc ", Style::default().fg(DIM)),
                Span::styled("cancel", Style::default().fg(Color::White)),
            ]);
            let hint_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(hint), hint_area);
        }
        SectionPickerMode::AddCustom { input } => {
            let area = centered_rect(50, 6, frame.area());
            frame.render_widget(Clear, area);

            let title = " Custom Section ";
            let block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent))
                .style(Style::default().bg(Color::Rgb(15, 25, 15)));

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let label_line = Line::from(vec![Span::styled("Name: ", Style::default().fg(accent))]);
            let label_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(label_line), label_area);

            let mut display = input.clone();
            display.push('│');
            let input_line = Line::from(vec![Span::styled(
                display,
                Style::default().fg(ACCENT).bg(Color::Rgb(20, 35, 20)),
            )]);
            let input_area = ratatui::layout::Rect {
                x: inner.x + 1,
                y: inner.y + 1,
                width: inner.width - 2,
                height: 1,
            };
            frame.render_widget(Paragraph::new(input_line), input_area);

            let hint = Line::from(vec![
                Span::styled("Enter ", Style::default().fg(DIM)),
                Span::styled("add  ", Style::default().fg(Color::White)),
                Span::styled("Esc ", Style::default().fg(DIM)),
                Span::styled("cancel", Style::default().fg(Color::White)),
            ]);
            let hint_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + 3,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(hint), hint_area);
        }
        SectionPickerMode::RemoveSection { selected } => {
            let removable = dialog.get_removable_sections();
            let height = (removable.len() as u16 + 4).min(15);
            let area = centered_rect(50, height, frame.area());
            frame.render_widget(Clear, area);

            let title = " Remove Section ";
            let block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent))
                .style(Style::default().bg(Color::Rgb(15, 25, 15)));

            let inner = block.inner(area);
            frame.render_widget(block, area);

            let mut y_pos = inner.y;
            for (i, (_, display_label)) in removable.iter().enumerate() {
                let is_selected = i == *selected;

                let style = if is_selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(ERROR_COLOR)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let line = Line::from(vec![Span::styled(format!("  {} ", display_label), style)]);
                let line_area = ratatui::layout::Rect {
                    x: inner.x,
                    y: y_pos,
                    width: inner.width,
                    height: 1,
                };
                frame.render_widget(Paragraph::new(line), line_area);
                y_pos += 1;
            }

            let hint = Line::from(vec![
                Span::styled("↑↓ ", Style::default().fg(DIM)),
                Span::styled("select  ", Style::default().fg(Color::White)),
                Span::styled("Enter ", Style::default().fg(DIM)),
                Span::styled("remove  ", Style::default().fg(Color::White)),
                Span::styled("Esc ", Style::default().fg(DIM)),
                Span::styled("cancel", Style::default().fg(Color::White)),
            ]);
            let hint_area = ratatui::layout::Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            };
            frame.render_widget(Paragraph::new(hint), hint_area);
        }
        SectionPickerMode::None => {}
    }
}

/// Generate a top border line with title dynamically based on width
fn generate_top_border(title: &str, width: u16, style: Style) -> Line<'static> {
    let title_with_spaces = format!(" {} ", title);
    let available_width = width.saturating_sub(title_with_spaces.len() as u16 + 2);
    let left_dashes = available_width / 2;
    let right_dashes = available_width - left_dashes;

    let border = format!(
        "┌{}{}{}┐",
        "─".repeat(left_dashes as usize),
        title_with_spaces,
        "─".repeat(right_dashes as usize)
    );
    Line::from(vec![Span::styled(border, style)])
}

/// Generate a bottom border line dynamically based on width
fn generate_bottom_border(width: u16, style: Style) -> Line<'static> {
    let border = format!("└{}┘", "─".repeat((width - 2) as usize));
    Line::from(vec![Span::styled(border, style)])
}

/// Inline `@`-file picker — a compact dropdown below the dialog box.
fn draw_at_picker_dropdown(
    frame: &mut Frame,
    dialog_area: ratatui::layout::Rect,
    accent: Color,
    dialog: &SimplePromptDialog,
) {
    let Some(picker) = &dialog.at_picker else {
        return;
    };

    const MAX_VISIBLE: usize = 8;
    let n = picker.entries.len().clamp(1, MAX_VISIBLE);
    let drop_h = n as u16 + 2; // entries + top/bottom border

    // Try to place the dropdown right below the dialog; flip above if no room.
    let screen_h = frame.area().height;
    let drop_y = if dialog_area.y + dialog_area.height + drop_h <= screen_h {
        dialog_area.y + dialog_area.height
    } else if dialog_area.y >= drop_h {
        dialog_area.y - drop_h
    } else {
        return; // no room at all
    };

    let drop_area = ratatui::layout::Rect {
        x: dialog_area.x,
        y: drop_y,
        width: dialog_area.width,
        height: drop_h,
    };
    frame.render_widget(Clear, drop_area);

    let title = format!(" {} ", picker.title());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(Color::Rgb(10, 20, 10)));
    let inner = block.inner(drop_area);
    frame.render_widget(block, drop_area);

    if picker.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "  no matches",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }

    let scroll = if picker.selected >= MAX_VISIBLE {
        picker.selected - MAX_VISIBLE + 1
    } else {
        0
    };

    let items: Vec<ListItem> = picker
        .entries
        .iter()
        .skip(scroll)
        .take(MAX_VISIBLE)
        .enumerate()
        .map(|(i, entry)| {
            let abs_idx = i + scroll;
            let icon = if entry.is_dir { "📁 " } else { "   " };
            let label = format!("{}{}", icon, entry.name);
            let style = if abs_idx == picker.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Span::styled(label, style))
        })
        .collect();

    frame.render_widget(List::new(items), inner);
}

pub(super) fn draw_simple_prompt_dialog(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.simple_prompt_dialog else {
        return;
    };

    // Get agent accent color
    let accent = app
        .selected_agent()
        .and_then(|a| match a {
            AgentEntry::Interactive(idx) => {
                app.interactive_agents.get(*idx).map(|ia| ia.accent_color)
            }
            _ => None,
        })
        .unwrap_or(ACCENT);

    // Use 65% of terminal width (responsive, not edge-to-edge)
    let percent_x = 65u16;
    let dialog_width = (frame.area().width * percent_x / 100).max(40);
    let inner_width = dialog_width.saturating_sub(2);
    let field_width = inner_width.saturating_sub(2).max(10) as usize;

    let is_instruction_focused = dialog.focused_section == 0;

    // Instruction height: expanded (1-5) when focused, collapsed (1) otherwise
    let instruction_content = dialog
        .sections
        .get("instruction")
        .map(|s| s.as_str())
        .unwrap_or("");
    let instruction_display_height = if is_instruction_focused {
        let vis = crate::tui::app::dialog::SimplePromptDialog::visual_line_count(
            instruction_content,
            field_width,
        );
        (vis as u16).clamp(1, 5)
    } else {
        1u16
    };

    // Optional sections: expanded when focused, collapsed (1) otherwise
    let mut optional_section_height: u16 = 0;
    for (i, section_name) in dialog.enabled_sections.iter().enumerate() {
        if section_name == "instruction" {
            continue;
        }
        let h = if dialog.focused_section == i {
            let content = dialog
                .sections
                .get(section_name)
                .map(|s| s.as_str())
                .unwrap_or("");
            let vis = crate::tui::app::dialog::SimplePromptDialog::visual_line_count(
                content,
                field_width,
            );
            let max_h =
                crate::tui::app::dialog::SimplePromptDialog::max_visible_lines(section_name);
            (vis as u16).clamp(1, max_h as u16)
        } else {
            1u16
        };
        optional_section_height += 1 + h + 1 + 1;
    }

    let total_height =
        2 + 1 + 1 + 1 + instruction_display_height + 1 + 1 + optional_section_height + 1;

    // Cap dialog height — leave at least 4 rows margin, minimum 10 rows.
    let max_dialog_h = frame.area().height.saturating_sub(4).max(10);
    let height = total_height.min(max_dialog_h);

    // Pre-compute render height for each section (label + content + border + gap = h + 3).
    // Used for auto-scroll calculation.
    let section_heights: Vec<u16> = dialog
        .enabled_sections
        .iter()
        .enumerate()
        .map(|(i, section_name)| {
            let is_focused = dialog.focused_section == i;
            let content_h = if i == 0 {
                // instruction
                instruction_display_height
            } else if is_focused {
                let content = dialog
                    .sections
                    .get(section_name)
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let vis = crate::tui::app::dialog::SimplePromptDialog::visual_line_count(
                    content,
                    field_width,
                );
                let max_h =
                    crate::tui::app::dialog::SimplePromptDialog::max_visible_lines(section_name);
                (vis as u16).clamp(1, max_h as u16)
            } else {
                1u16
            };
            content_h + 3 // label(1) + content + bottom_border(1) + gap(1)
        })
        .collect();

    let area = centered_rect(percent_x, height, frame.area());
    frame.render_widget(Clear, area);

    let title = " Prompt Builder ";
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Draw hint line
    let instructions = Line::from(vec![
        Span::styled("↑↓ ", Style::default().fg(DIM)),
        Span::styled("fields  ", Style::default().fg(Color::White)),
        Span::styled("⇧↑↓←→ ", Style::default().fg(DIM)),
        Span::styled("cursor  ", Style::default().fg(Color::White)),
        Span::styled("@ ", Style::default().fg(DIM)),
        Span::styled("file  ", Style::default().fg(Color::White)),
        Span::styled("Ctrl+A ", Style::default().fg(DIM)),
        Span::styled("add  ", Style::default().fg(Color::White)),
        Span::styled("Ctrl+X ", Style::default().fg(DIM)),
        Span::styled("remove  ", Style::default().fg(Color::White)),
        Span::styled("Ctrl+S ", Style::default().fg(DIM)),
        Span::styled("send  ", Style::default().fg(Color::White)),
        Span::styled("Esc  ", Style::default().fg(DIM)),
        Span::styled("cancel", Style::default().fg(Color::White)),
    ]);

    let instructions_area = ratatui::layout::Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(instructions), instructions_area);

    // ── Scroll computation ─────────────────────────────────────────────────
    // sections_available_h = inner height minus hint(1) + blank(1).
    let sections_top = inner.y + 2;
    let sections_available_h = inner.height.saturating_sub(2);

    // Work backwards from focused_section to find the first section that fits.
    let start_idx = {
        let focused = dialog.focused_section;
        let focused_h = section_heights.get(focused).copied().unwrap_or(4);
        let mut remaining = sections_available_h.saturating_sub(focused_h);
        let mut start = focused;
        while start > 0 {
            let prev_h = section_heights.get(start - 1).copied().unwrap_or(4);
            if prev_h > remaining {
                break;
            }
            remaining -= prev_h;
            start -= 1;
        }
        start
    };

    // Scroll indicators
    let inner_bottom = inner.y + inner.height;
    if start_idx > 0 {
        let arrow = Span::styled(" ▲ ", Style::default().fg(accent));
        let a = ratatui::layout::Rect { x: inner.x, y: sections_top, width: inner.width, height: 1 };
        frame.render_widget(Paragraph::new(Line::from(arrow)).alignment(ratatui::layout::Alignment::Right), a);
    }

    let mut y_pos = sections_top;

    // ── Draw Instruction field ──────────────────────────────────────────────
    if start_idx == 0 {
        let label_style = if is_instruction_focused {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(accent)
        };

        let label_line = generate_top_border("Instruction", inner.width, label_style);
        let label_area = ratatui::layout::Rect {
            x: inner.x,
            y: y_pos,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(label_line), label_area);
        y_pos += 1;

        let instruction_bg = if is_instruction_focused {
            Color::Rgb(40, 40, 40)
        } else {
            Color::Rgb(30, 30, 30)
        };

        let instruction_content = dialog
            .sections
            .get("instruction")
            .map(|s| s.as_str())
            .unwrap_or("");

        let (instruction_render_text, instr_scroll) = if is_instruction_focused {
            let cursor_idx = dialog
                .cursor("instruction")
                .min(instruction_content.chars().count());
            let before: String = instruction_content.chars().take(cursor_idx).collect();
            let after: String = instruction_content.chars().skip(cursor_idx).collect();
            (
                format!("{}│{}", before, after),
                dialog.scroll("instruction") as u16,
            )
        } else {
            let first_line = instruction_content
                .lines()
                .next()
                .unwrap_or(instruction_content);
            let text = if first_line.chars().count() > field_width {
                format!(
                    "{}…",
                    first_line
                        .chars()
                        .take(field_width.saturating_sub(1))
                        .collect::<String>()
                )
            } else {
                first_line.to_string()
            };
            (text, 0u16)
        };

        let content_style = Style::default().fg(Color::White).bg(instruction_bg);
        let instruction_paragraph = Paragraph::new(instruction_render_text)
            .style(content_style)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((instr_scroll, 0));

        let content_area = ratatui::layout::Rect {
            x: inner.x + 1,
            y: y_pos,
            width: inner.width.saturating_sub(2),
            height: instruction_display_height,
        };
        frame.render_widget(instruction_paragraph, content_area);
        y_pos += instruction_display_height;

        let bottom_border = generate_bottom_border(inner.width, label_style);
        let border_area = ratatui::layout::Rect {
            x: inner.x,
            y: y_pos,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(bottom_border), border_area);
        y_pos += 2;
    }

    // ── Draw optional sections (starting from start_idx, skipping instruction) ──
    for (i, section_name) in dialog.enabled_sections.iter().enumerate() {
        if section_name == "instruction" {
            continue;
        }
        // Skip sections before start_idx
        if i < start_idx {
            continue;
        }
        // Stop if we've run out of vertical space (leave 1 row for ▼ indicator)
        if y_pos + 3 >= inner_bottom {
            break;
        }

        let is_focused = dialog.focused_section == i;

        let section_type = {
            let known = [
                "output_format",
                "instruction",
                "context",
                "resources",
                "examples",
                "constraints",
            ];
            known
                .iter()
                .find(|k| section_name.starts_with(*k))
                .copied()
                .unwrap_or(section_name.as_str())
        };

        let label = crate::tui::app::dialog::SimplePromptDialog::get_available_sections()
            .into_iter()
            .find(|(name, _)| *name == section_type)
            .map(|(_, label)| label)
            .unwrap_or(section_type);

        let suffix = section_name.strip_prefix(section_type).unwrap_or("");
        let display_label = if suffix.is_empty() {
            label.to_string()
        } else {
            format!("{} {}", label, suffix.trim_start_matches('_'))
        };

        let label_style = if is_focused {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(accent)
        };

        let label_line = generate_top_border(&display_label, inner.width, label_style);
        let label_area = ratatui::layout::Rect {
            x: inner.x,
            y: y_pos,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(label_line), label_area);
        y_pos += 1;

        let section_bg = if is_focused {
            Color::Rgb(40, 40, 40)
        } else {
            Color::Rgb(30, 30, 30)
        };

        let content_raw = dialog
            .sections
            .get(section_name)
            .map(|s| s.as_str())
            .unwrap_or("");

        let (render_text, content_height, scroll_offset) = if is_focused {
            let cursor_idx = dialog.cursor(section_name).min(content_raw.chars().count());
            let before: String = content_raw.chars().take(cursor_idx).collect();
            let after: String = content_raw.chars().skip(cursor_idx).collect();
            let text = format!("{}│{}", before, after);
            let max_h =
                crate::tui::app::dialog::SimplePromptDialog::max_visible_lines(section_name);
            let vis = crate::tui::app::dialog::SimplePromptDialog::visual_line_count(
                content_raw,
                field_width,
            );
            // Clamp content height to available space
            let max_avail = inner_bottom.saturating_sub(y_pos).saturating_sub(2);
            (
                text,
                (vis as u16).clamp(1, max_h as u16).min(max_avail),
                dialog.scroll(section_name) as u16,
            )
        } else {
            let first_line = content_raw.lines().next().unwrap_or(content_raw);
            let text = if first_line.chars().count() > field_width {
                format!(
                    "{}…",
                    first_line
                        .chars()
                        .take(field_width.saturating_sub(1))
                        .collect::<String>()
                )
            } else {
                first_line.to_string()
            };
            (text, 1u16, 0u16)
        };

        let styled_content = dialog.get_file_reference_with_styling(&render_text, accent);
        let mut spans = Vec::new();
        for (text, color) in styled_content {
            let span_style = if let Some(c) = color {
                Style::default().fg(c).bg(section_bg)
            } else {
                Style::default().fg(Color::White).bg(section_bg)
            };
            spans.push(Span::styled(text, span_style));
        }

        let content_paragraph = Paragraph::new(Line::from(spans))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .scroll((scroll_offset, 0));

        let content_area = ratatui::layout::Rect {
            x: inner.x + 1,
            y: y_pos,
            width: inner.width.saturating_sub(2),
            height: content_height,
        };
        frame.render_widget(content_paragraph, content_area);
        y_pos += content_height;

        let bottom_border = generate_bottom_border(inner.width, label_style);
        let border_area = ratatui::layout::Rect {
            x: inner.x,
            y: y_pos,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(bottom_border), border_area);
        y_pos += 2;
    }

    // ▼ indicator when there are more sections below
    let last_visible_section = {
        let mut last = start_idx;
        let mut yy = sections_top;
        if start_idx == 0 {
            yy += section_heights.first().copied().unwrap_or(0);
        }
        for (i, _) in dialog.enabled_sections.iter().enumerate() {
            if i == 0 || i < start_idx { continue; }
            let sh = section_heights.get(i).copied().unwrap_or(4);
            if yy + sh + 3 >= inner_bottom { break; }
            yy += sh;
            last = i;
        }
        last
    };
    if last_visible_section < dialog.enabled_sections.len().saturating_sub(1) {
        let arrow = Span::styled(" ▼ ", Style::default().fg(accent));
        let a = ratatui::layout::Rect {
            x: inner.x,
            y: inner_bottom.saturating_sub(1),
            width: inner.width,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Line::from(arrow)).alignment(ratatui::layout::Alignment::Right), a);
    }

    // Draw @ file picker dropdown if active
    if dialog.at_picker.is_some() {
        draw_at_picker_dropdown(frame, area, accent, dialog);
    }

    // Draw picker modal if open
    draw_section_picker_modal(frame, app, accent, &dialog.picker_mode);
}
