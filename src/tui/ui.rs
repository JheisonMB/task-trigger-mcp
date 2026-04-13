//! UI rendering — sidebar with agent cards, log panel, header, footer, and dialogs.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

use super::agent::AgentStatus;
use super::app::{relative_time, AgentEntry, App, Focus};
use super::brians_brain::CellState;

const ACCENT: Color = Color::Rgb(76, 175, 80);
const DIM: Color = Color::Rgb(150, 150, 170);
const ERROR_COLOR: Color = Color::Rgb(229, 57, 53);
const BG_SELECTED: Color = Color::Rgb(20, 40, 20);
const INTERACTIVE_COLOR: Color = Color::Rgb(102, 187, 106);
const STATUS_DISABLED: Color = Color::Rgb(120, 120, 120);
const STATUS_RUNNING: Color = Color::Rgb(76, 175, 80);
const STATUS_OK: Color = Color::Rgb(66, 165, 245);
const STATUS_FAIL: Color = Color::Rgb(229, 57, 53);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    if app.sidebar_visible {
        let [sidebar, panel] =
            Layout::horizontal([Constraint::Length(26), Constraint::Min(0)]).areas(body);
        draw_header(frame, header, app);
        draw_sidebar(frame, sidebar, app);
        draw_log_panel(frame, panel, app);
    } else {
        draw_header_full(frame, header, app);
        draw_log_panel(frame, body, app);
    }
    draw_footer(frame, footer, app);

    if app.new_agent_dialog.is_some() {
        draw_new_agent_dialog(frame, app);
    }

    if app.quit_confirm {
        draw_quit_confirm(frame);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let status = if app.daemon_running {
        Span::styled(
            format!(" RUNNING (PID: {}) ", app.daemon_pid.unwrap_or(0)),
            Style::default().fg(Color::Black).bg(ACCENT),
        )
    } else {
        Span::styled(
            " STOPPED ",
            Style::default().fg(Color::Black).bg(ERROR_COLOR),
        )
    };

    let line = Line::from(vec![
        Span::styled(
            " agent-canopy",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        status,
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

/// Full-width header (sidebar hidden): name left, daemon status right.
fn draw_header_full(frame: &mut Frame, area: Rect, app: &App) {
    let status_text = if app.daemon_running {
        format!(" RUNNING (PID: {}) ", app.daemon_pid.unwrap_or(0))
    } else {
        " STOPPED ".to_string()
    };
    let status_w = status_text.chars().count() as u16;

    let left = Paragraph::new(Line::from(Span::styled(
        " agent-canopy",
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(left, area);

    if area.width > status_w {
        let status = Paragraph::new(Line::from(Span::styled(
            status_text,
            Style::default().fg(Color::Black).bg(if app.daemon_running {
                ACCENT
            } else {
                ERROR_COLOR
            }),
        )));
        let status_area = Rect::new(area.x + area.width - status_w, area.y, status_w, 1);
        frame.render_widget(status, status_area);
    }
}

fn draw_sidebar(frame: &mut Frame, area: Rect, app: &mut App) {
    // Clear click map from previous frame
    app.sidebar_click_map.clear();

    let bg_indices: Vec<usize> = app
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| !matches!(a, AgentEntry::Interactive(_)))
        .map(|(i, _)| i)
        .collect();
    let ix_indices: Vec<usize> = app
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, AgentEntry::Interactive(_)))
        .map(|(i, _)| i)
        .collect();

    let has_bg = !bg_indices.is_empty();
    let has_ix = !ix_indices.is_empty();

    if !has_bg && !has_ix {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let msg = Paragraph::new("  No agents registered").style(Style::default().fg(DIM));
        frame.render_widget(msg, inner);
        return;
    }

    // Sidebar is highlighted only when focus is Home
    let sidebar_focused = app.focus == Focus::Home;
    let card_h = 3u16;

    // Calculate proportional split
    let (bg_area, ix_area) = if has_bg && has_ix {
        let bg_needed = bg_indices.len() as u16 * card_h + 2;
        let ix_needed = ix_indices.len() as u16 * card_h + 2;
        let total = bg_needed + ix_needed;
        if total <= area.height {
            let [top, bottom] =
                Layout::vertical([Constraint::Length(bg_needed), Constraint::Min(ix_needed)])
                    .areas(area);
            (Some(top), Some(bottom))
        } else {
            let [top, bottom] =
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(area);
            (Some(top), Some(bottom))
        }
    } else if has_bg {
        (Some(area), None)
    } else {
        (None, Some(area))
    };

    if let Some(bg_area) = bg_area {
        let border_color = if sidebar_focused { ACCENT } else { DIM };
        let block = Block::default()
            .title(Span::styled(
                format!(" Background ({}) ", bg_indices.len()),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(bg_area);
        frame.render_widget(block, bg_area);
        draw_agent_list(frame, inner, &bg_indices, app, sidebar_focused, ACCENT);
    }

    if let Some(ix_area) = ix_area {
        let border_color = if sidebar_focused {
            INTERACTIVE_COLOR
        } else {
            DIM
        };
        let block = Block::default()
            .title(Span::styled(
                format!(" Interactive ({}) ", ix_indices.len()),
                Style::default()
                    .fg(INTERACTIVE_COLOR)
                    .add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(ix_area);
        frame.render_widget(block, ix_area);
        draw_agent_list(
            frame,
            inner,
            &ix_indices,
            app,
            sidebar_focused,
            INTERACTIVE_COLOR,
        );
    }
}

fn draw_agent_list(
    frame: &mut Frame,
    area: Rect,
    indices: &[usize],
    app: &mut App,
    _sidebar_focused: bool,
    accent: Color,
) {
    let card_h = 3u16;
    let row_h = 4u16; // 3 lines + 1 spacer
    let mut y = area.y;
    for (i, &idx) in indices.iter().enumerate() {
        if y + card_h > area.y + area.height {
            break;
        }
        let card_area = Rect::new(area.x, y, area.width, card_h);
        let agent = &app.agents[idx];
        let selected = idx == app.selected;
        draw_sidebar_card(frame, card_area, agent, app, selected, accent);
        app.sidebar_click_map.push((idx, y, y + card_h));
        // Add spacer between cards (but not after the last visible one)
        if i < indices.len() - 1 {
            y += row_h;
        } else {
            y += card_h;
        }
    }
}

fn draw_sidebar_card(
    frame: &mut Frame,
    area: Rect,
    agent: &AgentEntry,
    app: &App,
    selected: bool,
    _accent: Color,
) {
    let w = area.width as usize;
    let bg = if selected { BG_SELECTED } else { Color::Reset };

    // Resolve accent color per-agent type
    let accent = match agent {
        AgentEntry::Interactive(idx) => app.interactive_agents[*idx].accent_color,
        _ => ACCENT,
    };

    // Determine status info for accent bar color
    let (status_color, agent_type, type_detail) = match agent {
        AgentEntry::Task(t) => {
            let has_active = app.active_runs.contains_key(&t.id);
            let color = if !t.enabled {
                STATUS_DISABLED
            } else if has_active {
                STATUS_RUNNING
            } else if t.last_run_ok == Some(true) {
                STATUS_OK
            } else if t.last_run_ok == Some(false) {
                STATUS_FAIL
            } else {
                STATUS_OK
            };
            (color, "cron", t.cli.as_str())
        }
        AgentEntry::Watcher(w) => {
            let has_active = app.active_runs.contains_key(&w.id);
            let color = if !w.enabled {
                STATUS_DISABLED
            } else if has_active {
                STATUS_RUNNING
            } else {
                STATUS_OK
            };
            (color, "watch", w.cli.as_str())
        }
        AgentEntry::Interactive(idx) => {
            let a = &app.interactive_agents[*idx];
            let color = match &a.status {
                AgentStatus::Running => STATUS_RUNNING,
                AgentStatus::Exited(0) => STATUS_OK,
                AgentStatus::Exited(_) => STATUS_FAIL,
            };
            (color, "pty", a.cli.as_str())
        }
    };

    // Line 1: ▌ + id
    if area.height >= 1 {
        let accent_bar = Span::styled("▌", Style::default().fg(status_color));
        let id_text = Span::styled(
            agent.id(app),
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(if selected { accent } else { Color::White }),
        );
        let line = Line::from(vec![accent_bar, Span::raw(" "), id_text]);
        let r = Rect::new(area.x, area.y, area.width, 1);
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), r);
    }

    // Line 2: ▌ + type + detail
    if area.height >= 2 {
        let accent_bar = Span::styled("▌", Style::default().fg(status_color));
        let line = Line::from(vec![
            accent_bar,
            Span::raw(" "),
            Span::styled(
                format!("{} · {}", agent_type, truncate_str(type_detail, w.saturating_sub(6))),
                Style::default().fg(DIM),
            ),
        ]);
        let r = Rect::new(area.x, area.y + 1, area.width, 1);
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), r);
    }

    // Line 3: ▌ + working dir (last 2 segments)
    if area.height >= 3 {
        let accent_bar = Span::styled("▌", Style::default().fg(status_color));
        let work_dir = match agent {
            AgentEntry::Task(t) => t.working_dir.as_deref(),
            AgentEntry::Watcher(w) => Some(w.path.as_str()),
            AgentEntry::Interactive(idx) => Some(app.interactive_agents[*idx].working_dir.as_str()),
        };
        let dir_text = work_dir
            .filter(|d| !d.is_empty())
            .map(last_two_segments)
            .unwrap_or_else(|| "/".to_string());
        let dir_span = Span::styled(dir_text, Style::default().fg(DIM));
        let line = Line::from(vec![accent_bar, Span::raw(" "), dir_span]);
        let r = Rect::new(area.x, area.y + 2, area.width, 1);
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), r);
    }
}

fn draw_log_panel(frame: &mut Frame, area: Rect, app: &App) {
    let border_color = match app.focus {
        Focus::Agent | Focus::Preview => app
            .selected_agent()
            .map(|a| match a {
                AgentEntry::Interactive(idx) => app.interactive_agents[*idx].accent_color,
                _ => ACCENT,
            })
            .unwrap_or(DIM),
        _ => DIM,
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    match app.focus {
        // ── Home: banner/brain grid (pre-activation shows banner as cells) ──
        Focus::Home => {
            if let Some(ref brain) = app.brain {
                draw_brians_brain(frame, inner, brain);
                return;
            }
            draw_canopy_banner_preview(frame, inner);
            return;
        }

        // ── Preview: config/details or read-only PTY ──
        Focus::Preview => {
            match app.selected_agent() {
                Some(AgentEntry::Task(t)) => {
                    draw_task_details(frame, inner, t, app);
                    return;
                }
                Some(AgentEntry::Watcher(w)) => {
                    draw_watcher_details(frame, inner, w);
                    return;
                }
                Some(AgentEntry::Interactive(idx)) => {
                    let agent = &app.interactive_agents[*idx];
                    if let Some(snap) = agent.screen_snapshot() {
                        render_vt_screen(frame, inner, &snap);
                        return;
                    }
                }
                _ => {}
            }
            // Fallback: log
        }

        // ── Focus: interactive PTY (with cursor) or scrollable log ──
        Focus::Agent => {
            if let Some(AgentEntry::Interactive(idx)) = app.selected_agent() {
                let agent = &app.interactive_agents[*idx];
                if let Some(snap) = agent.screen_snapshot() {
                    render_vt_screen(frame, inner, &snap);
                    if !snap.scrolled {
                        let cx = inner.x + snap.cursor_col.min(inner.width.saturating_sub(1));
                        let cy = inner.y + snap.cursor_row.min(inner.height.saturating_sub(1));
                        frame.set_cursor_position((cx, cy));
                    }
                    // Scroll indicator when scrolled into history
                    if snap.scrolled {
                        let scroll_msg = " ▒ SCROLLED ▒ ";
                        let scroll_w = scroll_msg.len() as u16;
                        let sx = inner.x + inner.width.saturating_sub(scroll_w + 1);
                        let sy = inner.y;
                        let bar = Paragraph::new(scroll_msg)
                            .style(Style::default().fg(Color::Yellow).bg(Color::Black));
                        let scroll_area = ratatui::layout::Rect::new(sx, sy, scroll_w, 1);
                        frame.render_widget(bar, scroll_area);
                    }
                    return;
                }
            }
            // background agents fall through to log rendering below
        }
        Focus::NewAgentDialog => {}
    }

    // ── Log / text content ──
    let title = app.selected_id();
    let title_suffix = match app.focus {
        Focus::Agent => " (Esc → back)",
        Focus::Preview => " (Enter → focus)",
        _ => "",
    };
    let title_block = Block::default()
        .title(format!(" {title}{title_suffix} "))
        .borders(Borders::NONE);
    frame.render_widget(title_block, area);

    let line_count = app.log_content.lines().count() as u16;
    let max_scroll = line_count.saturating_sub(inner.height);
    let scroll = app.log_scroll.min(max_scroll);

    let paragraph = Paragraph::new(app.log_content.as_str())
        .style(Style::default().fg(Color::White))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inner);

    if line_count > inner.height {
        let mut scrollbar_state =
            ScrollbarState::new(line_count as usize).position(scroll as usize);
        frame.render_stateful_widget(
            Scrollbar::default().orientation(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar_state,
        );
    }
}

/// Render a vt100 screen snapshot directly into the ratatui buffer.
fn render_vt_screen(frame: &mut Frame, area: Rect, snap: &super::agent::ScreenSnapshot) {
    let buf = frame.buffer_mut();

    for (row_idx, row) in snap.cells.iter().enumerate() {
        if row_idx as u16 >= area.height {
            break;
        }
        let y = area.y + row_idx as u16;

        for (col_idx, cell) in row.iter().enumerate() {
            if col_idx as u16 >= area.width {
                break;
            }
            let x = area.x + col_idx as u16;

            let Some(c) = cell else {
                continue;
            };

            let ch = if c.ch.is_empty() { " " } else { &c.ch };

            let (fg, bg) = if c.inverse {
                (c.bg, c.fg)
            } else {
                (c.fg, c.bg)
            };

            let mut style = Style::default().fg(fg).bg(bg);
            if c.bold {
                style = style.add_modifier(Modifier::BOLD);
            }
            if c.underline {
                style = style.add_modifier(Modifier::UNDERLINED);
            }

            let buf_cell = &mut buf[(x, y)];
            buf_cell.set_symbol(ch);
            buf_cell.set_style(style);
        }
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints = match app.focus {
        Focus::Home => "  ↑↓ select agent  n new  q quit  Esc confirm quit  Tab sidebar",
        Focus::Preview => {
            "  ↑↓ nav  Enter focus  D delete  r rerun  e/d toggle  n new  Esc home  q quit"
        }
        Focus::NewAgentDialog => {
            "  ←→ select CLI  ↓ browse dirs  Space enter dir  Enter launch  Esc cancel"
        }
        Focus::Agent => {
            if matches!(app.selected_agent(), Some(AgentEntry::Interactive(_))) {
                "  EscEsc back  Shift+↑↓ scroll  PgUp/PgDn  Tab sidebar"
            } else {
                "  ↑↓/jk scroll log  Esc back  q quit"
            }
        }
    };

    let version = if app.daemon_version.is_empty() {
        String::new()
    } else {
        format!(" v{} ", app.daemon_version)
    };
    let version_w = version.len() as u16;

    // Hints on left, version on right
    let hints_span = Span::styled(hints, Style::default().fg(DIM));
    let version_span = Span::styled(
        &version,
        Style::default().fg(DIM).add_modifier(Modifier::BOLD),
    );

    let hints_p = Paragraph::new(Line::from(hints_span));
    frame.render_widget(hints_p, area);

    if version_w > 0 && area.width > version_w {
        let ver_area = Rect::new(area.x + area.width - version_w, area.y, version_w, 1);
        let ver_p = Paragraph::new(Line::from(version_span));
        frame.render_widget(ver_p, ver_area);
    }
}

fn draw_new_agent_dialog(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.new_agent_dialog else {
        return;
    };

    let accent = dialog.selected_accent_color();

    let height = match dialog.task_type {
        super::app::NewTaskType::Interactive => 16,
        super::app::NewTaskType::Scheduled => 16,
        super::app::NewTaskType::Watcher => 14,
    };
    let area = centered_rect(65, height, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" New Task ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(accent))
        .style(Style::default().bg(Color::Rgb(15, 25, 15)));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let type_names = ["Interactive", "Scheduled", "Watcher"];
    let type_idx = match dialog.task_type {
        super::app::NewTaskType::Interactive => 0,
        super::app::NewTaskType::Scheduled => 1,
        super::app::NewTaskType::Watcher => 2,
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

    // Build lines for the dialog
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Type:  ", Style::default().fg(DIM)),
            Span::styled(format!(" ◀ {} ▶ ", type_names[type_idx]), focus_style(0)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  CLI:   ", Style::default().fg(DIM)),
            if is_focused(1) {
                Span::styled(format!(" ◀ {cli_name} ▶ "), focus_style(1))
            } else {
                Span::styled(
                    format!(" ◀ {cli_name} ▶ "),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                )
            },
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Dir:   ", Style::default().fg(DIM)),
            Span::styled(truncate_str(&dialog.working_dir, 50), focus_style(2)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Model: ", Style::default().fg(DIM)),
            Span::styled(
                if dialog.model.is_empty() {
                    "(optional, e.g. gpt-4.1)".to_string()
                } else {
                    dialog.model.clone()
                },
                focus_style(3),
            ),
        ]),
        Line::from(""),
    ];

    // Type-specific fields
    if matches!(
        dialog.task_type,
        super::app::NewTaskType::Scheduled | super::app::NewTaskType::Watcher
    ) {
        lines.push(Line::from(vec![
            Span::styled("  Prompt:", Style::default().fg(DIM)),
            Span::styled(
                if dialog.prompt.is_empty() {
                    "enter task prompt...".to_string()
                } else {
                    dialog.prompt.clone()
                },
                focus_style(4),
            ),
        ]));
        lines.push(Line::from(""));

        if dialog.task_type == super::app::NewTaskType::Scheduled {
            lines.push(Line::from(vec![
                Span::styled("  Cron:  ", Style::default().fg(DIM)),
                Span::styled(dialog.cron_expr.clone(), focus_style(5)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("  Path:  ", Style::default().fg(DIM)),
                Span::styled(truncate_str(&dialog.watch_path, 50), focus_style(5)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Directory browser (for interactive mode)
    if dialog.task_type == super::app::NewTaskType::Interactive && !dialog.dir_entries.is_empty() {
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
            let entry_style = if is_selected && is_focused(2) {
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
        super::app::NewTaskType::Interactive => {
            "  Tab: next field · ←→: CLI · ↑↓: dirs · Space: enter dir · Enter: launch · Esc: cancel"
        }
        super::app::NewTaskType::Scheduled => {
            "  Tab: next field · ←→: type/CLI · chars: input · Enter: create · Esc: cancel"
        }
        super::app::NewTaskType::Watcher => {
            "  Tab: next field · ←→: type/CLI · chars: input · Enter: create · Esc: cancel"
        }
    };

    lines.push(Line::from(Span::styled(
        help_text,
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Create a centered rect of given percentage width and fixed height.
fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let [_, center, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .areas(area);

    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(center);

    center
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 1 {
        format!("{}…", &s[..max - 1])
    } else {
        String::new()
    }
}

/// Extract the last two path segments, e.g. `/a/b/c/d` → `c/d`.
fn last_two_segments(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return "/".to_string();
    }
    if parts.len() <= 2 {
        return trimmed.to_string();
    }
    format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
}

fn draw_quit_confirm(frame: &mut Frame) {
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

fn draw_canopy_banner_preview(frame: &mut Frame, area: Rect) {
    const BANNER: &str = r#"
  ██████   ██████   ████████    ██████  ████████  █████ ████
 ███░░███ ░░░░░███ ░░███░░███  ███░░███░░███░░███░░███ ░███
░███ ░░░   ███████  ░███ ░███ ░███ ░███ ░███ ░███ ░███ ░███
░███  ███ ███░░███  ░███ ░███ ░███ ░███ ░███ ░███ ░███ ░███
░░██████ ░░████████ ████ █████░░██████  ░███████  ░░███████
 ░░░░░░   ░░░░░░░░ ░░░░ ░░░░░  ░░░░░░   ░███░░░    ░░░░░███
                                        ░███       ███ ░███
                                        █████     ░░██████
                                       ░░░░░       ░░░░░░
"#;

    let lines: Vec<Line> = BANNER
        .lines()
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
        })
        .collect();

    let total_banner = lines.len() as u16;
    let top_pad = if area.height > total_banner {
        (area.height - total_banner) / 2
    } else {
        0
    };

    let banner_area = Rect::new(
        area.x,
        area.y + top_pad,
        area.width,
        total_banner.min(area.height),
    );

    let banner = Paragraph::new(lines).alignment(ratatui::layout::Alignment::Center);
    frame.render_widget(banner, banner_area);
}

fn draw_task_details(frame: &mut Frame, area: Rect, task: &crate::domain::models::Task, app: &App) {
    let has_active = app.active_runs.contains_key(&task.id);
    let (status_text, status_color) = if !task.enabled {
        ("DISABLED", STATUS_DISABLED)
    } else if has_active {
        ("RUNNING", STATUS_RUNNING)
    } else if task.last_run_ok == Some(true) {
        ("OK", STATUS_OK)
    } else if task.last_run_ok == Some(false) {
        ("FAILED", STATUS_FAIL)
    } else {
        ("IDLE", STATUS_OK)
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Status:  ", Style::default().fg(DIM)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Prompt:  ", Style::default().fg(DIM)),
            Span::raw(&task.prompt),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Cron:    ", Style::default().fg(DIM)),
            Span::styled(&task.schedule_expr, Style::default().fg(INTERACTIVE_COLOR)),
        ]),
        Line::from(vec![
            Span::styled("CLI:     ", Style::default().fg(DIM)),
            Span::raw(task.cli.as_str()),
        ]),
    ];

    if let Some(ref model) = task.model {
        lines.push(Line::from(vec![
            Span::styled("Model:   ", Style::default().fg(DIM)),
            Span::raw(model),
        ]));
    }

    if let Some(ref dir) = task.working_dir {
        lines.push(Line::from(vec![
            Span::styled("Dir:     ", Style::default().fg(DIM)),
            Span::raw(dir),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("Timeout: ", Style::default().fg(DIM)),
        Span::raw(format!("{} min", task.timeout_minutes)),
    ]));

    if let Some(ref exp) = task.expires_at {
        lines.push(Line::from(vec![
            Span::styled("Expires: ", Style::default().fg(DIM)),
            Span::raw(relative_time(exp)),
        ]));
    }

    if let Some(ref lr) = task.last_run_at {
        lines.push(Line::from(vec![
            Span::styled("Last run:", Style::default().fg(DIM)),
            Span::raw(relative_time(lr)),
        ]));
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_watcher_details(frame: &mut Frame, area: Rect, watcher: &crate::domain::models::Watcher) {
    let (status_text, status_color) = if watcher.enabled {
        ("ACTIVE", STATUS_RUNNING)
    } else {
        ("DISABLED", STATUS_DISABLED)
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("Status:  ", Style::default().fg(DIM)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Prompt:  ", Style::default().fg(DIM)),
            Span::raw(&watcher.prompt),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Path:    ", Style::default().fg(DIM)),
            Span::raw(&watcher.path),
        ]),
        Line::from(vec![
            Span::styled("Events:  ", Style::default().fg(DIM)),
            Span::raw(
                watcher
                    .events
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ]),
        Line::from(vec![
            Span::styled("CLI:     ", Style::default().fg(DIM)),
            Span::raw(watcher.cli.as_str()),
        ]),
        Line::from(vec![
            Span::styled("Triggers:", Style::default().fg(DIM)),
            Span::raw(watcher.trigger_count.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Debounce:", Style::default().fg(DIM)),
            Span::raw(format!("{}s", watcher.debounce_seconds)),
        ]),
        Line::from(vec![
            Span::styled("Recursive:", Style::default().fg(DIM)),
            Span::raw(if watcher.recursive { "yes" } else { "no" }),
        ]),
        Line::from(vec![
            Span::styled("Timeout: ", Style::default().fg(DIM)),
            Span::raw(format!("{} min", watcher.timeout_minutes)),
        ]),
    ];

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_brians_brain(frame: &mut Frame, area: Rect, brain: &super::brians_brain::BriansBrain) {
    let buf = frame.buffer_mut();
    for (r, row) in brain.grid.iter().enumerate() {
        if r as u16 >= area.height {
            break;
        }
        for (c, cell) in row.iter().enumerate() {
            if c as u16 >= area.width {
                break;
            }
            let x = area.x + c as u16;
            let y = area.y + r as u16;
            let (ch, color) = match cell {
                CellState::On => ("█", ACCENT),
                CellState::Dying => ("░", Color::Rgb(100, 130, 100)),
                CellState::Off => (" ", Color::Reset),
            };
            let buf_cell = &mut buf[(x, y)];
            buf_cell.set_symbol(ch);
            buf_cell.set_style(Style::default().fg(color));
        }
    }
    // Overlay the banner progressively during pre-activation (unfold from center row).
    if !brain.active {
        let accent_dim = Color::Rgb(80, 140, 80);
        for br in brain.visible_overlay() {
            if br.row as u16 >= area.height {
                continue;
            }
            for &(c, is_shade) in &br.cells {
                if c as u16 >= area.width {
                    continue;
                }
                let x = area.x + c as u16;
                let y = area.y + br.row as u16;
                let buf_cell = &mut buf[(x, y)];
                buf_cell.set_symbol(if is_shade { "░" } else { "█" });
                buf_cell.set_style(Style::default().fg(if is_shade { accent_dim } else { ACCENT }));
            }
        }
    }
}
