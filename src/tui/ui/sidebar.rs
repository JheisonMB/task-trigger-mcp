//! Sidebar rendering — agent cards split into Background and Interactive groups.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::{last_two_segments, truncate_str, ACCENT, BG_SELECTED, DIM, INTERACTIVE_COLOR};
use super::{STATUS_DISABLED, STATUS_FAIL, STATUS_OK, STATUS_RUNNING};
use crate::tui::agent::AgentStatus;
use crate::tui::app::{AgentEntry, App};
use ratatui::style::Color;

pub(super) fn draw_sidebar(frame: &mut Frame, area: Rect, app: &mut App) {
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

    let border_color = DIM;
    let row_h = 4u16; // 3 lines + 1 spacer

    let (bg_area, ix_area) = if has_bg && has_ix {
        let bg_needed = bg_indices.len() as u16 * row_h + 2;
        let ix_needed = ix_indices.len() as u16 * row_h + 2;
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
        let block = Block::default()
            .title(Span::styled(
                format!(" background ({}) ", bg_indices.len()),
                Style::default().fg(DIM),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(bg_area);
        frame.render_widget(block, bg_area);
        draw_agent_list(frame, inner, &bg_indices, app, ACCENT);
    }

    if let Some(ix_area) = ix_area {
        let block = Block::default()
            .title(Span::styled(
                format!(" interactive ({}) ", ix_indices.len()),
                Style::default().fg(DIM),
            ))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(ix_area);
        frame.render_widget(block, ix_area);
        draw_agent_list(frame, inner, &ix_indices, app, INTERACTIVE_COLOR);
    }
}

fn draw_agent_list(frame: &mut Frame, area: Rect, indices: &[usize], app: &mut App, accent: Color) {
    let card_h = 3u16;
    let row_h = 4u16;
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

    let accent = match agent {
        AgentEntry::Interactive(idx) => app.interactive_agents[*idx].accent_color,
        _ => ACCENT,
    };

    let (mut status_color, agent_type, type_detail) = match agent {
        AgentEntry::BackgroundAgent(t) => {
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

    // Detect if waiting for input and apply pulsing animation
    let is_waiting = if let AgentEntry::Interactive(idx) = agent {
        app.interactive_agents[*idx].is_waiting_for_input()
    } else {
        false
    };

    // Pulse animation: cycle every 20 ticks (at ~50ms tick = 1 second)
    let pulse_cycle = (app.animation_tick / 5) % 4;
    if is_waiting && pulse_cycle < 2 {
        // Brighten the color during animation
        status_color = match status_color {
            Color::Rgb(r, g, b) => Color::Rgb(
                r.saturating_add(60),
                g.saturating_add(60),
                b.saturating_add(60),
            ),
            Color::Yellow => Color::Rgb(255, 255, 100),
            Color::Cyan => Color::Rgb(100, 255, 255),
            Color::White => Color::Rgb(255, 255, 255),
            c => c,
        };
    }

    // Line 1: accent bar + id
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

    // Line 2: accent bar + type · detail
    if area.height >= 2 {
        let accent_bar = Span::styled("▌", Style::default().fg(status_color));
        let line = Line::from(vec![
            accent_bar,
            Span::raw(" "),
            Span::styled(
                format!(
                    "{} · {}",
                    agent_type,
                    truncate_str(type_detail, w.saturating_sub(6))
                ),
                Style::default().fg(DIM),
            ),
        ]);
        let r = Rect::new(area.x, area.y + 1, area.width, 1);
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), r);
    }

    // Line 3: accent bar + working dir
    if area.height >= 3 {
        let accent_bar = Span::styled("▌", Style::default().fg(status_color));
        let work_dir = match agent {
            AgentEntry::BackgroundAgent(t) => t.working_dir.as_deref(),
            AgentEntry::Watcher(w) => Some(w.path.as_str()),
            AgentEntry::Interactive(idx) => Some(app.interactive_agents[*idx].working_dir.as_str()),
        };
        let dir_text = work_dir
            .filter(|d| !d.is_empty())
            .map(last_two_segments)
            .unwrap_or_else(|| "/".to_string());

        // Add waiting indicator if applicable
        let display_text = if is_waiting {
            format!("{} ⏳", dir_text)
        } else {
            dir_text
        };

        let dir_span = Span::styled(display_text, Style::default().fg(DIM));
        let line = Line::from(vec![accent_bar, Span::raw(" "), dir_span]);
        let r = Rect::new(area.x, area.y + 2, area.width, 1);
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), r);
    }
}
