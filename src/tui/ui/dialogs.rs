//! Dialog overlays — new agent, quit confirmation, color legend, context transfer.

use ratatui::layout::{Constraint, Layout, Rect};
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

    // Dir browser: label row + filter row + up to 10 entry rows + status line
    let filtered_entries = dialog.filtered_dir_entries();
    let dir_rows: u16 = if filtered_entries.is_empty() && dialog.dir_entries.is_empty() {
        0
    } else {
        3 + filtered_entries.len().min(10) as u16
    };

    let is_edit = dialog.is_edit_mode();

    let is_interactive = matches!(dialog.task_type, crate::tui::app::NewTaskType::Interactive);
    let is_terminal = matches!(dialog.task_type, crate::tui::app::NewTaskType::Terminal);

    // Base heights: Interactive=15, Scheduled/Watcher=13, Terminal=10
    let base_height: u16 = match dialog.task_type {
        crate::tui::app::NewTaskType::Interactive => 15 + dir_rows,
        crate::tui::app::NewTaskType::Scheduled | crate::tui::app::NewTaskType::Watcher => {
            13 + dir_rows
        }
        crate::tui::app::NewTaskType::Terminal => 10 + dir_rows,
    };
    let height = base_height + picker_rows as u16;
    let area = centered_rect(65, height, frame.area());
    frame.render_widget(Clear, area);

    let title = if is_edit {
        match dialog.task_type {
            crate::tui::app::NewTaskType::Scheduled => " Edit Task ",
            crate::tui::app::NewTaskType::Watcher => " Edit Watcher ",
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

    let type_names = ["Interactive", "Scheduled", "Watcher", "Terminal"];
    let type_idx = match dialog.task_type {
        crate::tui::app::NewTaskType::Interactive => 0,
        crate::tui::app::NewTaskType::Scheduled => 1,
        crate::tui::app::NewTaskType::Watcher => 2,
        crate::tui::app::NewTaskType::Terminal => 3,
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

    // After removing the Name field, field indices for Interactive shift down by 1:
    // Interactive: 0=type, 1=mode, 2=CLI, 3=model, 4=dir, 5=yolo
    // Scheduled/Watcher: 0=type, 1=CLI, 2=model, 3=prompt, 4=cron/watch, 5=dir
    // Terminal: 0=type, 1=dir, 2=shell
    let cli_field = if is_interactive { 2 } else { 1 };

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

    // For Terminal type, show only Dir + Shell fields (no CLI, no model, etc.)
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

        // Directory browser for terminal (field 1 = dir)
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

            // Status line with scroll indicators
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

    // Interactive: 0=type 1=mode 2=CLI 3=model 4=dir 5=yolo
    // Scheduled/Watcher: 0=type 1=CLI 2=model 3=prompt 4=cron/watch 5=dir
    let model_field = if is_interactive { 3 } else { 2 };
    let prompt_field = 3usize;
    let extra_field = 4usize;
    let dir_field = if is_interactive || dialog.task_type == crate::tui::app::NewTaskType::Watcher {
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
        let yolo_field = 5usize;
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
        let filtered = dialog.filtered_dir_entries();
        let is_watcher = dialog.task_type == crate::tui::app::NewTaskType::Watcher;
        let browser_field_idx = if is_watcher { extra_field } else { dir_field };

        // Filter input line
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

        // Status line with scroll indicators
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
        crate::tui::app::NewTaskType::Terminal => {
            "  ↑↓: fields  (in dirs: → enter  ← up) · Enter: launch · Esc: cancel"
        }
    };

    lines.push(Line::from(Span::styled(
        help_text,
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Draw the split picker overlay for pairing two sessions.
pub(super) fn draw_split_picker(frame: &mut Frame, app: &App) {
    if !app.split_picker_open {
        return;
    }

    let sessions = &app.split_picker_sessions;
    let current_name = match app.selected_agent() {
        Some(crate::tui::app::AgentEntry::Interactive(idx)) => {
            app.interactive_agents[*idx].name.clone()
        }
        Some(crate::tui::app::AgentEntry::Terminal(idx)) => app.terminal_agents[*idx].name.clone(),
        _ => String::new(),
    };

    let visible = sessions.len().min(6) as u16;
    let height = 9 + visible;
    let area = centered_rect(60, height, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Split con... ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Green))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Current: ", Style::default().fg(DIM)),
            Span::styled(
                &current_name,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled("  Selecciona:", Style::default().fg(DIM))),
    ];

    for (i, (name, type_label)) in sessions.iter().enumerate() {
        if name == &current_name {
            continue;
        }
        let selected = i == app.split_picker_idx;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let prefix = if selected { "  > " } else { "    " };
        lines.push(Line::from(vec![
            Span::styled(format!("{}{}", prefix, name), style),
            Span::styled(format!("  [{}]", type_label), Style::default().fg(DIM)),
        ]));
    }

    lines.push(Line::from(""));

    let orient_label = match app.split_picker_orientation {
        crate::domain::models::SplitOrientation::Horizontal => "● Horizontal  ○ Vertical",
        crate::domain::models::SplitOrientation::Vertical => "○ Horizontal  ● Vertical",
    };
    lines.push(Line::from(vec![
        Span::styled("  Orientación:  ", Style::default().fg(DIM)),
        Span::styled(orient_label, Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(Span::styled(
        "                Tab para alternar",
        Style::default().fg(DIM),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Esc cancelar               Enter crear",
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
    let area = centered_rect(42, 22, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Shortcuts & Legend ")
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

    let key_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(DIM);

    let lines = vec![
        Line::from(Span::styled(
            "Status colors",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("▌ ", Style::default().fg(STATUS_RUNNING)),
            Span::styled(
                "RUNNING   ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Agent is executing", desc_style),
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
            Span::styled("Agent ready / last run OK", desc_style),
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
            Span::styled("Last run failed / error exit", desc_style),
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
            Span::styled("Waiting for user input", desc_style),
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
            Span::styled("Agent is paused", desc_style),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Shortcuts",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Ctrl+S  ", key_style),
            Span::styled("split with another session", desc_style),
        ]),
        Line::from(vec![
            Span::styled("F4      ", key_style),
            Span::styled("dissolve/end", desc_style),
        ]),
        Line::from(vec![
            Span::styled("Shift+F4", key_style),
            Span::styled("end", desc_style),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+←→ ", key_style),
            Span::styled("switch split panel", desc_style),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+T  ", key_style),
            Span::styled("context transfer to agent", desc_style),
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

    let (src_id, accent) = if modal.source_is_terminal {
        app.terminal_agents
            .get(modal.source_agent_idx)
            .map(|a| (a.name.as_str(), a.accent_color))
            .unwrap_or(("?", ACCENT))
    } else {
        app.interactive_agents
            .get(modal.source_agent_idx)
            .map(|a| (a.name.as_str(), a.accent_color))
            .unwrap_or(("?", ACCENT))
    };

    let src_type = if modal.source_is_terminal {
        "terminal"
    } else {
        "agent"
    };
    let n_label = if modal.source_is_terminal {
        "pages (×50 lines)"
    } else {
        "prompts"
    };

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
            Span::styled(format!("  From {src_type}: "), Style::default().fg(DIM)),
            Span::styled(format!(" ◀ {} ▶ ", modal.n_prompts), active_style),
            Span::styled(
                format!("  (most recent {n_label})"),
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

            for (y_pos, (i, (_, label))) in (inner.y..).zip(addable.iter().enumerate()) {
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

            for (y_pos, (i, (_, display_label))) in (inner.y..).zip(removable.iter().enumerate()) {
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
        SectionPickerMode::SkillsPicker {
            selected, entries, ..
        } => {
            let height = (entries.len() as u16 + 5).min(16);
            let area = centered_rect(55, height, frame.area());
            frame.render_widget(Clear, area);

            let block = Block::default()
                .title(" Tools — Pick a Skill ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(accent))
                .style(Style::default().bg(Color::Rgb(10, 20, 30)));

            let inner = block.inner(area);
            frame.render_widget(block, area);

            if entries.is_empty() {
                let msg = Line::from(vec![Span::styled(
                    "  No skills found",
                    Style::default().fg(Color::DarkGray),
                )]);
                frame.render_widget(
                    Paragraph::new(msg),
                    ratatui::layout::Rect {
                        x: inner.x,
                        y: inner.y,
                        width: inner.width,
                        height: 1,
                    },
                );
            } else {
                for (y_pos, (i, (_label, raw_name, prefix))) in
                    (inner.y..).zip(entries.iter().enumerate())
                {
                    if y_pos >= inner.y + inner.height.saturating_sub(1) {
                        break;
                    }
                    let is_selected = i == *selected;
                    let style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    // Picker shows [prefix]:name for clarity (skill vs global)
                    let display = format!("  [{prefix}]:{raw_name} ");
                    let line = Line::from(vec![Span::styled(display, style)]);
                    frame.render_widget(
                        Paragraph::new(line),
                        ratatui::layout::Rect {
                            x: inner.x,
                            y: y_pos,
                            width: inner.width,
                            height: 1,
                        },
                    );
                }
            }

            let hint = Line::from(vec![
                Span::styled("↑↓ ", Style::default().fg(DIM)),
                Span::styled("select  ", Style::default().fg(Color::White)),
                Span::styled("Enter ", Style::default().fg(DIM)),
                Span::styled("add  ", Style::default().fg(Color::White)),
                Span::styled("Esc ", Style::default().fg(DIM)),
                Span::styled("cancel", Style::default().fg(Color::White)),
            ]);
            frame.render_widget(
                Paragraph::new(hint),
                ratatui::layout::Rect {
                    x: inner.x,
                    y: inner.y + inner.height.saturating_sub(1),
                    width: inner.width,
                    height: 1,
                },
            );
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
    anchor_area: ratatui::layout::Rect,
    accent: Color,
    dialog: &SimplePromptDialog,
) {
    let Some(picker) = &dialog.at_picker else {
        return;
    };

    const MAX_VISIBLE: usize = 8;
    let screen_h = frame.area().height;
    let available_below = screen_h.saturating_sub(anchor_area.y + anchor_area.height);
    let available_above = anchor_area.y.saturating_sub(dialog_area.y);

    let prefer_below = available_below >= 3 || available_below >= available_above;
    let available_space = if prefer_below {
        available_below
    } else {
        available_above
    };
    let visible_items = picker
        .entries
        .len()
        .clamp(1, MAX_VISIBLE)
        .min(available_space.saturating_sub(2) as usize);
    if visible_items == 0 {
        return;
    }
    let drop_h = visible_items as u16 + 2;
    let drop_y = if prefer_below {
        anchor_area.y + anchor_area.height
    } else {
        anchor_area.y.saturating_sub(drop_h)
    };

    let drop_area = ratatui::layout::Rect {
        x: anchor_area.x.saturating_sub(1).max(dialog_area.x),
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
        .take(visible_items)
        .enumerate()
        .map(|(i, entry)| {
            let abs_idx = i + scroll;
            let icon = if entry.is_dir { "📁 " } else { "   " };

            // Show relative path when in recursive search mode (query active)
            let label = if picker.query.is_empty() {
                // Flat mode: just show filename
                format!("{}{}", icon, entry.name)
            } else {
                // Recursive mode: show relative path to distinguish files with same name
                let relative_path = entry
                    .path
                    .strip_prefix(&picker.workdir)
                    .unwrap_or(&entry.path);
                let display_path = if relative_path.to_string_lossy() == entry.name {
                    // Same directory as workdir
                    entry.name.clone()
                } else {
                    // Show relative path
                    relative_path.to_string_lossy().to_string()
                };
                format!("{}{}", icon, display_path)
            };

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
    let mut picker_anchor_area: Option<ratatui::layout::Rect> = None;

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
        let a = ratatui::layout::Rect {
            x: inner.x,
            y: sections_top,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(arrow)).alignment(ratatui::layout::Alignment::Right),
            a,
        );
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
        if is_instruction_focused {
            picker_anchor_area = Some(content_area);
        }
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
                "tools",
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
        let is_tools = section_type == "tools";
        let display_label = if is_tools && suffix.is_empty() {
            "Tools".to_string()
        } else if is_tools {
            format!("Tools {}", suffix.trim_start_matches('_'))
        } else if suffix.is_empty() {
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

        let (render_text, content_height, scroll_offset) = if is_tools {
            // Tools section: read-only, always 1 line — shows skill label or placeholder
            let display = if content_raw.trim().is_empty() {
                "  (empty — Ctrl+A to pick a skill)".to_string()
            } else {
                content_raw.trim().to_string()
            };
            (display, 1u16, 0u16)
        } else if is_focused {
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
        if is_focused {
            picker_anchor_area = Some(content_area);
        }
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
            if i == 0 || i < start_idx {
                continue;
            }
            let sh = section_heights.get(i).copied().unwrap_or(4);
            if yy + sh + 3 >= inner_bottom {
                break;
            }
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
        frame.render_widget(
            Paragraph::new(Line::from(arrow)).alignment(ratatui::layout::Alignment::Right),
            a,
        );
    }

    // Draw @ file picker dropdown if active
    if dialog.at_picker.is_some() {
        let anchor = picker_anchor_area.unwrap_or(inner);
        draw_at_picker_dropdown(frame, area, anchor, accent, dialog);
    }

    // Draw picker modal if open
    draw_section_picker_modal(frame, app, accent, &dialog.picker_mode);
}

// ── Suggestion picker (terminal Tab autocomplete) ───────────────

pub(super) fn draw_suggestion_picker(
    frame: &mut Frame,
    app: &App,
    panel_area: ratatui::layout::Rect,
) {
    let Some(picker) = &app.suggestion_picker else {
        return;
    };

    // Determine the actual area to draw the picker in (respecting split view)
    let picker_area = if let Some(ref split_id) = app.active_split_id {
        if let Some(group) = app.split_groups.iter().find(|g| g.id == *split_id) {
            let orientation = group.orientation;
            let areas = match orientation {
                crate::domain::models::SplitOrientation::Horizontal => {
                    Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .areas(panel_area)
                }
                crate::domain::models::SplitOrientation::Vertical => {
                    Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .areas(panel_area)
                }
            };
            let [area_a, area_b]: [Rect; 2] = areas;
            let raw = if app.split_right_focused {
                area_b
            } else {
                area_a
            };
            // Account for the split panel border (1px each side)
            Rect::new(
                raw.x.saturating_add(1),
                raw.y.saturating_add(1),
                raw.width.saturating_sub(2),
                raw.height.saturating_sub(2),
            )
        } else {
            panel_area
        }
    } else {
        panel_area
    };

    if picker.items.is_empty() {
        // Show "no matches" indicator
        let w = 30u16.min(picker_area.width.saturating_sub(2));
        let area = ratatui::layout::Rect::new(
            picker_area.x + 1,
            picker_area.y + picker_area.height.saturating_sub(3),
            w,
            2,
        );
        frame.render_widget(Clear, area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .style(Style::default().bg(Color::Rgb(15, 15, 25)));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let msg = Paragraph::new("  No matches").style(Style::default().fg(DIM));
        frame.render_widget(msg, inner);
        return;
    }

    let visible = picker.visible_count().min(10) as u16;
    let w = 60u16.min(picker_area.width.saturating_sub(2));
    let h = visible + 2; // items + border

    let total = picker.items.len();
    let title = match picker.mode {
        crate::tui::terminal_history::PickerMode::CommandHistory => {
            if total > picker.visible_count() {
                format!(" History [{}/{}] ", picker.selected + 1, total)
            } else {
                " History ".to_string()
            }
        }
        crate::tui::terminal_history::PickerMode::CdDirectory => {
            format!(" {} ←→ ", picker.input)
        }
    };

    // Anchor above the warp input box (3 rows from bottom)
    let area = ratatui::layout::Rect::new(
        picker_area.x + 1,
        picker_area.y + picker_area.height.saturating_sub(h + 4),
        w,
        h,
    );
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let scroll_offset = picker.scroll_offset;
    let items: Vec<ListItem> = picker
        .visible_items()
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let abs_index = scroll_offset + i;
            let selected = abs_index == picker.selected;
            let style = if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let prefix = if selected { "> " } else { "  " };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(&item.label, style),
            ]))
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, inner);
}
