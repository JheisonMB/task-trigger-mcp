//! Dialog overlays — new agent, quit confirmation, color legend.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use super::{centered_rect, truncate_str};
use super::{ACCENT, DIM, INTERACTIVE_COLOR, STATUS_DISABLED, STATUS_FAIL, STATUS_OK, STATUS_RUNNING};
use crate::tui::app::App;

pub(super) fn draw_new_agent_dialog(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.new_agent_dialog else {
        return;
    };

    let accent = dialog.selected_accent_color();

    let picker_rows = if dialog.model_picker_open && !dialog.model_suggestions.is_empty() {
        let visible = dialog.model_suggestions.len().min(5);
        let overflow_line = if dialog.model_suggestions.len() > 5 { 1 } else { 0 };
        visible + overflow_line
    } else {
        0
    };

    let base_height: u16 = match dialog.task_type {
        crate::tui::app::NewTaskType::Interactive => 18,
        crate::tui::app::NewTaskType::Scheduled => 16,
        crate::tui::app::NewTaskType::Watcher => 14,
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

    let cli_name = dialog.selected_cli().as_str();

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
        lines.push(Line::from(vec![
            Span::styled("  Session:  ", Style::default().fg(DIM)),
            Span::styled(
                format!(" ◀ {} ▶ ", mode_names[mode_idx]),
                focus_style(mode_field),
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

    let model_field = if is_interactive { 3 } else { 2 };
    let dir_field = if is_interactive { 4 } else { 3 };
    let prompt_field = if is_interactive { 5 } else { 4 };
    let extra_field = if is_interactive { 6 } else { 5 };

    lines.push(Line::from(vec![
        Span::styled("  Model: ", Style::default().fg(DIM)),
        Span::styled(
            if dialog.model.is_empty() {
                "(press space to select)".to_string()
            } else {
                format!("{}▏", dialog.model)
            },
            focus_style(model_field),
        ),
    ]));

    // Model suggestions dropdown
    if is_focused(model_field) && dialog.model_picker_open && !dialog.model_suggestions.is_empty()
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
                Span::styled(
                    format!("    {} ", if is_sel { "›" } else { " " }),
                    style,
                ),
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
                format!("    … {total} models (↑↓ scroll, → or Tab accept)"),
                Style::default().fg(DIM),
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("  Dir:   ", Style::default().fg(DIM)),
        Span::styled(
            truncate_str(&dialog.working_dir, 50),
            focus_style(dir_field),
        ),
    ]));
    lines.push(Line::from(""));

    if matches!(
        dialog.task_type,
        crate::tui::app::NewTaskType::Scheduled | crate::tui::app::NewTaskType::Watcher
    ) {
        lines.push(Line::from(vec![
            Span::styled("  Prompt:", Style::default().fg(DIM)),
            Span::styled(
                if dialog.prompt.is_empty() {
                    "enter task prompt...".to_string()
                } else {
                    dialog.prompt.clone()
                },
                focus_style(prompt_field),
            ),
        ]));
        lines.push(Line::from(""));

        if dialog.task_type == crate::tui::app::NewTaskType::Scheduled {
            lines.push(Line::from(vec![
                Span::styled("  Cron:  ", Style::default().fg(DIM)),
                Span::styled(dialog.cron_expr.clone(), focus_style(extra_field)),
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

    // Directory browser (all task types)
    if !dialog.dir_entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Directories (↑↓ navigate, Space to enter):",
            Style::default().fg(DIM),
        )));

        let visible_rows = 4;
        let scroll = dialog.dir_selected.saturating_sub(visible_rows - 1);

        for (i, entry) in dialog.dir_entries.iter().enumerate().skip(scroll) {
            if i >= scroll + visible_rows {
                break;
            }

            let is_selected = i == dialog.dir_selected;
            let entry_style = if is_selected && is_focused(dir_field) {
                Style::default()
                    .fg(Color::Black)
                    .bg(INTERACTIVE_COLOR)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let icon = if entry == ".." { ".." } else { ">" };
            lines.push(Line::from(Span::styled(
                format!("    {} {}", icon, entry),
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
