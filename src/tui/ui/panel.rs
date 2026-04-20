//! Right panel rendering — PTY output, brain automaton, banner, background_agent/watcher details, log.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use ratatui::Frame;

use super::{
    ACCENT, DIM, INTERACTIVE_COLOR, STATUS_DISABLED, STATUS_FAIL, STATUS_OK, STATUS_RUNNING,
};
use crate::tui::agent::ScreenSnapshot;
use crate::tui::app::{relative_time, AgentEntry, App, Focus};
use crate::tui::brians_brain::CellState;

pub(super) fn draw_log_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    let border_color = match app.focus {
        Focus::Agent | Focus::Preview => app
            .selected_agent()
            .map(|a| match a {
                AgentEntry::Interactive(idx) => app
                    .interactive_agents
                    .get(*idx)
                    .map_or(ACCENT, |a| a.accent_color),
                AgentEntry::Terminal(idx) => app
                    .terminal_agents
                    .get(*idx)
                    .map_or(ACCENT, |a| a.accent_color),
                _ => ACCENT,
            })
            .unwrap_or(DIM),
        _ => DIM,
    };

    let mode_label = match app.focus {
        Focus::Preview => " Preview ",
        Focus::Agent => " Focus ",
        _ => "",
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    if !mode_label.is_empty() {
        block = block.title(Span::styled(
            mode_label,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Store actual inner dimensions so PTY resize matches exactly
    app.last_panel_inner = (inner.width, inner.height);

    match app.focus {
        Focus::Home => {
            if let Some(ref brain) = app.brain {
                draw_brians_brain(frame, inner, brain);
                return;
            }
            draw_canopy_banner(frame, inner);
            return;
        }

        Focus::Preview => match app.selected_agent() {
            Some(AgentEntry::BackgroundAgent(t)) => {
                draw_task_details(frame, inner, t, app);
                return;
            }
            Some(AgentEntry::Watcher(w)) => {
                draw_watcher_details(frame, inner, w);
                return;
            }
            Some(AgentEntry::Interactive(idx)) => {
                if let Some(agent) = app.interactive_agents.get(*idx) {
                    if let Some(snap) = agent.screen_snapshot() {
                        render_vt_screen(frame, inner, &snap);
                        return;
                    }
                }
            }
            Some(AgentEntry::Terminal(idx)) => {
                if let Some(agent) = app.terminal_agents.get(*idx) {
                    if let Some(snap) = agent.screen_snapshot() {
                        render_vt_screen(frame, inner, &snap);
                        return;
                    }
                }
            }
            Some(AgentEntry::Group(idx)) => {
                draw_group_details(frame, inner, app, *idx);
                return;
            }
            _ => {}
        },

        Focus::Agent => match app.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => {
                let idx = *idx;
                if let Some(agent) = app.interactive_agents.get(idx) {
                    if let Some(snap) = agent.screen_snapshot() {
                        render_vt_screen(frame, inner, &snap);
                        if !snap.scrolled {
                            let cx = inner.x + snap.cursor_col.min(inner.width.saturating_sub(1));
                            let cy = inner.y + snap.cursor_row.min(inner.height.saturating_sub(1));
                            frame.set_cursor_position((cx, cy));
                        }
                        render_indicators(frame, inner, &snap, app);
                        return;
                    }
                }
            }
            Some(AgentEntry::Terminal(idx)) => {
                let idx = *idx;
                if let Some(agent) = app.terminal_agents.get(idx) {
                    if let Some(snap) = agent.screen_snapshot() {
                        render_vt_screen(frame, inner, &snap);
                        if !snap.scrolled {
                            let cx = inner.x + snap.cursor_col.min(inner.width.saturating_sub(1));
                            let cy = inner.y + snap.cursor_row.min(inner.height.saturating_sub(1));
                            frame.set_cursor_position((cx, cy));
                        }
                        render_indicators(frame, inner, &snap, app);
                        return;
                    }
                }
            }
            Some(AgentEntry::Group(idx)) => {
                draw_group_details(frame, inner, app, *idx);
                return;
            }
            _ => {}
        },

        Focus::NewAgentDialog => {
            let prev = app.new_agent_dialog.as_ref().and_then(|d| d.prev_focus);
            match prev {
                Some(Focus::Home) | None => {
                    if let Some(ref brain) = app.brain {
                        draw_brians_brain(frame, inner, brain);
                    } else {
                        draw_canopy_banner(frame, inner);
                    }
                    return;
                }
                _ => {}
            }
        }

        Focus::ContextTransfer => {
            // The context transfer modal is drawn as an overlay in ui/mod.rs.
            // Fall through to draw the underlying panel as background.
        }
        Focus::PromptTemplateDialog => {
            // The prompt template dialog is drawn as an overlay in ui/mod.rs.
            // Fall through to draw the underlying panel as background.
        }
    }

    // ── Log / text content fallback ──
    draw_log_text(frame, area, inner, app);
}

// ── Split panel ─────────────────────────────────────────────────

/// Render one half of a split view — finds the session by name and draws its PTY.
pub(super) fn draw_split_panel(
    frame: &mut Frame,
    area: Rect,
    app: &mut App,
    session_name: &str,
    focused: bool,
) {
    // Find the agent by name (could be interactive or terminal)
    let found = find_session_by_name(app, session_name);

    let accent = match &found {
        Some(SessionRef::Interactive(idx)) => app.interactive_agents[*idx].accent_color,
        Some(SessionRef::Terminal(idx)) => app.terminal_agents[*idx].accent_color,
        None => DIM,
    };

    let border_color = if focused { accent } else { DIM };
    let border_style = Style::default().fg(border_color);

    let title = if focused {
        format!(" ● {session_name} ")
    } else {
        format!("   {session_name} ")
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            title,
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Store dimensions for the focused panel so PTY resizes match
    if focused {
        app.last_panel_inner = (inner.width, inner.height);
    }

    let snap = match &found {
        Some(SessionRef::Interactive(idx)) => app.interactive_agents[*idx].screen_snapshot(),
        Some(SessionRef::Terminal(idx)) => app.terminal_agents[*idx].screen_snapshot(),
        None => None,
    };

    if let Some(snap) = snap {
        render_vt_screen(frame, inner, &snap);
        if focused && !snap.scrolled && matches!(app.focus, Focus::Agent) {
            let cx = inner.x + snap.cursor_col.min(inner.width.saturating_sub(1));
            let cy = inner.y + snap.cursor_row.min(inner.height.saturating_sub(1));
            frame.set_cursor_position((cx, cy));
        }
        render_indicators(frame, inner, &snap, app);
    } else {
        let msg = Paragraph::new(format!("  Session '{session_name}' not found"))
            .style(Style::default().fg(DIM));
        frame.render_widget(msg, inner);
    }
}

enum SessionRef {
    Interactive(usize),
    Terminal(usize),
}

fn find_session_by_name(app: &App, name: &str) -> Option<SessionRef> {
    if let Some(idx) = app.interactive_agents.iter().position(|a| a.name == name) {
        return Some(SessionRef::Interactive(idx));
    }
    if let Some(idx) = app.terminal_agents.iter().position(|a| a.name == name) {
        return Some(SessionRef::Terminal(idx));
    }
    None
}

// ── Group details panel ─────────────────────────────────────────

fn draw_group_details(frame: &mut Frame, area: Rect, app: &App, group_idx: usize) {
    let Some(group) = app.split_groups.get(group_idx) else {
        return;
    };

    let is_active = app
        .active_split_id
        .as_deref()
        .is_some_and(|id| id == group.id);

    let header_color = if is_active {
        Color::Green
    } else {
        Color::Rgb(150, 150, 200)
    };

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "  Split Group  ",
                Style::default()
                    .fg(header_color)
                    .add_modifier(Modifier::BOLD),
            ),
            if is_active {
                Span::styled("● active", Style::default().fg(Color::Green))
            } else {
                Span::styled("○ inactive", Style::default().fg(DIM))
            },
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Session A:  ", Style::default().fg(DIM)),
            Span::styled(
                &group.session_a,
                Style::default()
                    .fg(INTERACTIVE_COLOR)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("  Session B:  ", Style::default().fg(DIM)),
            Span::styled(
                &group.session_b,
                Style::default()
                    .fg(INTERACTIVE_COLOR)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Orientation: ", Style::default().fg(DIM)),
            Span::styled(
                group.orientation.as_str(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            if is_active {
                "  Ctrl+X to dissolve  ·  Ctrl+←/→ switch panel"
            } else {
                "  Enter to activate split  ·  D to dissolve"
            },
            Style::default().fg(DIM),
        )),
    ];

    // Show whether sessions still exist
    let session_a_exists = app
        .interactive_agents
        .iter()
        .any(|a| a.name == group.session_a)
        || app
            .terminal_agents
            .iter()
            .any(|a| a.name == group.session_a);
    let session_b_exists = app
        .interactive_agents
        .iter()
        .any(|a| a.name == group.session_b)
        || app
            .terminal_agents
            .iter()
            .any(|a| a.name == group.session_b);

    if !session_a_exists || !session_b_exists {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  ⚠ one or more sessions no longer exist",
            Style::default().fg(Color::Yellow),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

// ── Indicators (SCROLLED / COPIED) ──────────────────────────────

fn render_indicators(frame: &mut Frame, inner: Rect, snap: &ScreenSnapshot, _app: &App) {
    if snap.scrolled {
        let msg = " \u{2592} SCROLLED \u{2592} "; // ▒ SCROLLED ▒
        let w = msg.chars().count() as u16; // display width (char count, not bytes)
        let x = inner.x + inner.width.saturating_sub(w + 1);
        let area = Rect::new(x, inner.y, w, 1);
        let widget = Paragraph::new(msg).style(Style::default().fg(Color::Yellow).bg(Color::Black));
        frame.render_widget(widget, area);
    }
}

// ── vt100 screen rendering ──────────────────────────────────────

fn render_vt_screen(frame: &mut Frame, area: Rect, snap: &ScreenSnapshot) {
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

// ── Canopy banner ───────────────────────────────────────────────

fn draw_canopy_banner(frame: &mut Frame, area: Rect) {
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

// ── Brian's Brain automaton ─────────────────────────────────────

pub(crate) fn draw_brians_brain(
    frame: &mut Frame,
    area: Rect,
    brain: &crate::tui::brians_brain::BriansBrain,
) {
    use crate::tui::brians_brain::BannerCellKind;
    let buf = frame.buffer_mut();

    if !brain.active {
        // Pre-activation: render banner overlay with glitch effects.
        let accent_dim = Color::Rgb(80, 140, 80);
        let glitch_color = Color::Rgb(50, 220, 50);
        let (vx, vy) = brain.vibration;
        for br in brain.visible_overlay() {
            let render_row = br.row as i32 + vy as i32;
            if render_row < 0 || render_row as u16 >= area.height {
                continue;
            }
            for &(c, kind) in &br.cells {
                let render_col = c as i32 + vx as i32;
                if render_col < 0 || render_col as u16 >= area.width {
                    continue;
                }
                let x = area.x + render_col as u16;
                let y = area.y + render_row as u16;
                let buf_cell = &mut buf[(x, y)];
                match kind {
                    BannerCellKind::Block => {
                        buf_cell.set_symbol("█");
                        buf_cell.set_style(Style::default().fg(ACCENT));
                    }
                    BannerCellKind::Shade => {
                        buf_cell.set_symbol("░");
                        buf_cell.set_style(Style::default().fg(accent_dim));
                    }
                    BannerCellKind::Glitch(ch) => {
                        // Render as single-char string
                        let s: String = std::iter::once(ch).collect();
                        buf_cell.set_symbol(&s);
                        buf_cell.set_style(Style::default().fg(glitch_color));
                    }
                }
            }
        }
        return;
    }

    // Active automaton: use per-cell green from green_grid.
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
            let g = brain.green_grid[r][c];
            let (ch, color) = match cell {
                CellState::On => ("█", Color::Rgb(0, g, 0)),
                CellState::Dying => {
                    let dim_g = (g as u16 * 6 / 10) as u8;
                    ("░", Color::Rgb(dim_g / 3, dim_g, dim_g / 3))
                }
                CellState::Off => (" ", Color::Reset),
            };
            let buf_cell = &mut buf[(x, y)];
            buf_cell.set_symbol(ch);
            buf_cell.set_style(Style::default().fg(color));
        }
    }
}

// ── BackgroundAgent details (preview) ──────────────────────────────────────

fn draw_task_details(
    frame: &mut Frame,
    area: Rect,
    background_agent: &crate::domain::models::BackgroundAgent,
    app: &App,
) {
    let has_active = app.active_runs.contains_key(&background_agent.id);
    let (status_text, status_color) = if !background_agent.enabled {
        ("DISABLED", STATUS_DISABLED)
    } else if has_active {
        ("RUNNING", STATUS_RUNNING)
    } else if background_agent.last_run_ok == Some(true) {
        ("OK", STATUS_OK)
    } else if background_agent.last_run_ok == Some(false) {
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
            Span::raw(&background_agent.prompt),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Cron:    ", Style::default().fg(DIM)),
            Span::styled(
                &background_agent.schedule_expr,
                Style::default().fg(INTERACTIVE_COLOR),
            ),
        ]),
        Line::from(vec![
            Span::styled("CLI:     ", Style::default().fg(DIM)),
            Span::raw(background_agent.cli.as_str()),
        ]),
    ];

    if let Some(ref model) = background_agent.model {
        lines.push(Line::from(vec![
            Span::styled("Model:   ", Style::default().fg(DIM)),
            Span::raw(model),
        ]));
    }

    if let Some(ref dir) = background_agent.working_dir {
        lines.push(Line::from(vec![
            Span::styled("Dir:     ", Style::default().fg(DIM)),
            Span::raw(dir),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("Timeout: ", Style::default().fg(DIM)),
        Span::raw(format!("{} min", background_agent.timeout_minutes)),
    ]));

    if let Some(ref exp) = background_agent.expires_at {
        lines.push(Line::from(vec![
            Span::styled("Expires: ", Style::default().fg(DIM)),
            Span::raw(relative_time(exp)),
        ]));
    }

    if let Some(ref lr) = background_agent.last_run_at {
        lines.push(Line::from(vec![
            Span::styled("Last run:", Style::default().fg(DIM)),
            Span::raw(relative_time(lr)),
        ]));
    }

    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

// ── Watcher details (preview) ───────────────────────────────────

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

// ── Log text fallback ───────────────────────────────────────────

fn draw_log_text(frame: &mut Frame, area: Rect, inner: Rect, app: &App) {
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
