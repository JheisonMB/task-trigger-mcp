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

pub fn draw(frame: &mut Frame, app: &App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let [sidebar, panel] =
        Layout::horizontal([Constraint::Length(26), Constraint::Min(0)]).areas(body);

    draw_header(frame, header, app);
    draw_sidebar(frame, sidebar, app);
    draw_log_panel(frame, panel, app);
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

    let version = if app.daemon_version.is_empty() {
        String::new()
    } else {
        format!(" v{}", app.daemon_version)
    };

    let interactive_count = app.interactive_agents.len();
    let interactive_span = if interactive_count > 0 {
        Span::styled(
            format!("  {interactive_count} agent(s)"),
            Style::default().fg(INTERACTIVE_COLOR),
        )
    } else {
        Span::raw("")
    };

    let line = Line::from(vec![
        Span::styled(
            " 🌿 canopy",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        status,
        Span::styled(version, Style::default().fg(DIM)),
        interactive_span,
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

fn draw_sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let bg_agents: Vec<(usize, &AgentEntry)> = app
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| !matches!(a, AgentEntry::Interactive(_)))
        .collect();
    let ix_agents: Vec<(usize, &AgentEntry)> = app
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, AgentEntry::Interactive(_)))
        .collect();

    let has_bg = !bg_agents.is_empty();
    let has_ix = !ix_agents.is_empty();

    if !has_bg && !has_ix {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(DIM))
            .title(Span::styled(
                " Agents ",
                Style::default().fg(DIM).add_modifier(Modifier::BOLD),
            ));
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
        let bg_needed = bg_agents.len() as u16 * card_h + 2;
        let ix_needed = ix_agents.len() as u16 * card_h + 2;
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
                format!(" Background ({}) ", bg_agents.len()),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(bg_area);
        frame.render_widget(block, bg_area);
        draw_agent_list(frame, inner, &bg_agents, app, sidebar_focused, ACCENT);
    }

    if let Some(ix_area) = ix_area {
        let border_color = if sidebar_focused {
            INTERACTIVE_COLOR
        } else {
            DIM
        };
        let block = Block::default()
            .title(Span::styled(
                format!(" Interactive ({}) ", ix_agents.len()),
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
            &ix_agents,
            app,
            sidebar_focused,
            INTERACTIVE_COLOR,
        );
    }
}

fn draw_agent_list(
    frame: &mut Frame,
    area: Rect,
    agents: &[(usize, &AgentEntry)],
    app: &App,
    show_selection: bool,
    accent: Color,
) {
    let card_h = 3u16;
    let mut y = area.y;
    for (i, agent) in agents {
        if y + card_h > area.y + area.height {
            break;
        }
        let card_area = Rect::new(area.x, y, area.width, card_h);
        let selected = show_selection && *i == app.selected;
        draw_sidebar_card(frame, card_area, agent, app, selected, accent);
        y += card_h;
    }
}

fn draw_sidebar_card(
    frame: &mut Frame,
    area: Rect,
    agent: &AgentEntry,
    app: &App,
    selected: bool,
    accent: Color,
) {
    let (icon, id, info) = match agent {
        AgentEntry::Task(t) => {
            let has_active = app.active_runs.contains_key(&t.id);
            let icon = status_icon(t.enabled, has_active, t.last_run_ok);
            let info = format!("cron · {}", t.cli);
            (icon, t.id.as_str(), info)
        }
        AgentEntry::Watcher(w) => {
            let has_active = app.active_runs.contains_key(&w.id);
            let icon = if !w.enabled {
                "⚫"
            } else if has_active {
                "🟢"
            } else {
                "👁"
            };
            let info = format!("watch · {}", w.cli);
            (icon, w.id.as_str(), info)
        }
        AgentEntry::Interactive(idx) => {
            let a = &app.interactive_agents[*idx];
            let icon = match a.status {
                AgentStatus::Running => "🟢",
                AgentStatus::Exited(0) => "✅",
                AgentStatus::Exited(_) => "🔴",
            };
            let info = format!("{} · {}", a.cli, truncate_path(&a.working_dir));
            (icon, a.id.as_str(), info)
        }
    };

    let bg = if selected { BG_SELECTED } else { Color::Reset };
    let w = area.width as usize;

    if area.height >= 1 {
        let line = Line::from(vec![
            Span::raw(format!(" {icon} ")),
            Span::styled(
                truncate_str(id, w.saturating_sub(4)),
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(if selected { accent } else { Color::White }),
            ),
        ]);
        let r = Rect::new(area.x, area.y, area.width, 1);
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), r);
    }
    if area.height >= 2 {
        let line = Line::from(Span::styled(
            format!("    {}", truncate_str(&info, w.saturating_sub(4))),
            Style::default().fg(DIM),
        ));
        let r = Rect::new(area.x, area.y + 1, area.width, 1);
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), r);
    }
}

fn draw_log_panel(frame: &mut Frame, area: Rect, app: &App) {
    let border_color = match app.focus {
        Focus::Agent => INTERACTIVE_COLOR,
        Focus::Preview => ACCENT,
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
        Focus::Home => "  ↑↓ select agent  n new agent  q quit  Esc confirm quit",
        Focus::Preview => {
            "  ↑↓ nav  Enter focus  D delete  r rerun  e/d toggle  n new  Esc home  q quit"
        }
        Focus::NewAgentDialog => {
            "  ←→ select CLI  ↓ browse dirs  Space enter dir  Enter launch  Esc cancel"
        }
        Focus::Agent => {
            if matches!(app.selected_agent(), Some(AgentEntry::Interactive(_))) {
                "  EscEsc back  Shift+↑↓ scroll  PgUp/PgDn — all input goes to agent"
            } else {
                "  ↑↓/jk scroll log  Esc back  q quit"
            }
        }
    };

    let line = Line::from(Span::styled(hints, Style::default().fg(DIM)));
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_new_agent_dialog(frame: &mut Frame, app: &App) {
    let Some(dialog) = &app.new_agent_dialog else {
        return;
    };

    let area = centered_rect(60, 18, frame.area());
    // No Clear — let the background (automaton/banner) show through

    let block = Block::default()
        .title(" New Agent ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(INTERACTIVE_COLOR));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let cli_style = if dialog.field == 0 {
        Style::default()
            .fg(Color::Black)
            .bg(INTERACTIVE_COLOR)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let dir_style = if dialog.field == 1 {
        Style::default()
            .fg(Color::Black)
            .bg(INTERACTIVE_COLOR)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let cli_name = dialog.selected_cli().as_str();

    // Build lines for the dialog
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  CLI:  ", Style::default().fg(DIM)),
            Span::styled(format!(" ◀ {cli_name} ▶ "), cli_style),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Dir:  ", Style::default().fg(DIM)),
            Span::styled(&dialog.working_dir, dir_style),
        ]),
        Line::from(""),
    ];

    // Add directory browser list
    if !dialog.dir_entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Directories (↑↓ navigate, Space to enter):",
            Style::default().fg(DIM),
        )));

        let visible_rows = 6;
        let scroll = dialog.dir_selected.saturating_sub(visible_rows - 1);

        for (i, entry) in dialog.dir_entries.iter().enumerate().skip(scroll) {
            if i >= scroll + visible_rows {
                break;
            }

            let is_selected = i == dialog.dir_selected;
            let entry_style = if is_selected && dialog.field == 1 {
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

            let icon = if entry == ".." { "📁" } else { "📂" };

            lines.push(Line::from(Span::styled(
                format!("    {} {}", icon, entry),
                entry_style,
            )));
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  ←→: CLI · ↓: dirs · Space: enter dir · Enter: launch · Esc: cancel",
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

fn status_icon(enabled: bool, running: bool, last_ok: Option<bool>) -> &'static str {
    if !enabled {
        return "⚫";
    }
    if running {
        return "🟢";
    }
    match last_ok {
        Some(true) => "🔵",
        Some(false) => "🔴",
        None => "🔵",
    }
}

#[allow(dead_code)]
fn run_result_icon(last_ok: Option<bool>) -> &'static str {
    match last_ok {
        Some(true) => "✅",
        Some(false) => "❌",
        None => "",
    }
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

fn truncate_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if let Some(rest) = path.strip_prefix(home_str.as_ref()) {
            return format!("~{rest}");
        }
    }
    path.to_string()
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
    let status = if !task.enabled {
        "⚫ Disabled"
    } else if has_active {
        "🟢 Running"
    } else if task.last_run_ok == Some(true) {
        "🔵 OK"
    } else if task.last_run_ok == Some(false) {
        "🔴 Failed"
    } else {
        "🔵 Never run"
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Status:  ", Style::default().fg(DIM)),
            Span::styled(status, Style::default().fg(ACCENT)),
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
    let lines = vec![
        Line::from(vec![
            Span::styled("Status:  ", Style::default().fg(DIM)),
            Span::styled(
                if watcher.enabled {
                    "🟢 Active"
                } else {
                    "⚫ Disabled"
                },
                Style::default().fg(ACCENT),
            ),
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
}
