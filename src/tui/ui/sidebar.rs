//! Sidebar rendering — agent cards split into Background and Interactive groups.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::{last_two_segments, truncate_str, ACCENT, BG_SELECTED, DIM, INTERACTIVE_COLOR};
use super::{STATUS_DISABLED, STATUS_FAIL, STATUS_OK, STATUS_RUNNING};
use crate::tui::agent::AgentStatus;
use crate::tui::app::{AgentEntry, App, Focus};
use ratatui::style::Color;

pub(super) fn draw_sidebar(frame: &mut Frame, area: Rect, app: &mut App) {
    app.sidebar_click_map.clear();

    let bg_indices: Vec<usize> = app
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| {
            !matches!(
                a,
                AgentEntry::Interactive(_) | AgentEntry::Terminal(_) | AgentEntry::Group(_)
            )
        })
        .map(|(i, _)| i)
        .collect();
    let ix_indices: Vec<usize> = app
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, AgentEntry::Interactive(_)))
        .map(|(i, _)| i)
        .collect();
    let term_indices: Vec<usize> = app
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, AgentEntry::Terminal(_)))
        .map(|(i, _)| i)
        .collect();

    let has_bg = !bg_indices.is_empty();
    let has_ix = !ix_indices.is_empty();
    let has_term = !term_indices.is_empty();
    let has_groups = !app.split_groups.is_empty();

    // Responsive dashboard height based on how many lines the dashboard will render.
    // Base lines: cpu + mem + disk + load/procs = 4. +1 for gpu, +1 for swap if used.
    let mut dashboard_content_lines = 4u16;
    if app.system_info.gpu_info.is_some() {
        dashboard_content_lines += 1;
    }
    if app.system_info.swap_used > 0 {
        dashboard_content_lines += 1;
    }
    let dashboard_height = dashboard_content_lines + 2; // +2 for borders
    let dashboard_area = if area.height >= dashboard_height {
        Some(Rect::new(
            area.x,
            area.y + area.height - dashboard_height,
            area.width,
            dashboard_height,
        ))
    } else {
        None
    };

    let content_area = if let Some(dashboard) = dashboard_area {
        Rect::new(
            area.x,
            area.y,
            area.width,
            area.height.saturating_sub(dashboard.height),
        )
    } else {
        area
    };

    if !has_bg && !has_ix && !has_term && !has_groups {
        let brain_area = Rect::new(
            area.x,
            area.y,
            area.width,
            area.height
                .saturating_sub(dashboard_area.map(|d| d.height).unwrap_or(0)),
        );
        if brain_area.height >= 3 && brain_area.width >= 6 {
            if let Some(brain) = app.sidebar_brain.as_ref() {
                crate::tui::ui::panel::draw_brians_brain(frame, brain_area, brain);
            }
        }
        if let Some(dashboard_area) = dashboard_area {
            crate::tui::ui::system_dashboard::render_system_dashboard(
                frame,
                dashboard_area,
                &app.system_info,
                app.temperature_unit,
            );
        }
        return;
    }

    let bg_needed = if has_bg {
        bg_indices.len() as u16 * 4 + 2
    } else {
        0
    };
    let ix_needed = if has_ix {
        ix_indices.len() as u16 * 4 + 2
    } else {
        0
    };
    let term_needed = if has_term {
        term_indices.len() as u16 * 4 + 2
    } else {
        0
    };
    let grp_needed = if has_groups {
        app.split_groups.len() as u16 * 2 + 2
    } else {
        0
    };
    let total_needed = bg_needed + ix_needed + term_needed + grp_needed;

    let border_color = DIM;
    let section_count = has_bg as u16 + has_ix as u16 + has_term as u16 + has_groups as u16;
    let mut brain_area: Option<Rect> = None;

    let (bg_area, ix_area, term_area, grp_area) = if total_needed <= content_area.height
        || section_count == 1
    {
        let mut remaining = content_area;
        let bg_a = if has_bg {
            let [top, rest] = Layout::vertical([Constraint::Length(bg_needed), Constraint::Min(0)])
                .areas(remaining);
            remaining = rest;
            Some(top)
        } else {
            None
        };
        let ix_a = if has_ix {
            let [top, rest] = Layout::vertical([Constraint::Length(ix_needed), Constraint::Min(0)])
                .areas(remaining);
            remaining = rest;
            Some(top)
        } else {
            None
        };
        let term_a = if has_term {
            let [top, rest] =
                Layout::vertical([Constraint::Length(term_needed), Constraint::Min(0)])
                    .areas(remaining);
            remaining = rest;
            Some(top)
        } else {
            None
        };
        let grp_a = if has_groups && remaining.height > 0 {
            let [top, rest] =
                Layout::vertical([Constraint::Length(grp_needed), Constraint::Min(0)])
                    .areas(remaining);
            remaining = rest;
            Some(top)
        } else {
            None
        };
        if remaining.height > 0 {
            brain_area = Some(remaining);
        }
        (bg_a, ix_a, term_a, grp_a)
    } else {
        // Distribute evenly
        let per = content_area.height / section_count;
        let mut remaining = content_area;
        let bg_a = if has_bg {
            let [top, rest] =
                Layout::vertical([Constraint::Length(per), Constraint::Min(0)]).areas(remaining);
            remaining = rest;
            Some(top)
        } else {
            None
        };
        let ix_a = if has_ix {
            let [top, rest] =
                Layout::vertical([Constraint::Length(per), Constraint::Min(0)]).areas(remaining);
            remaining = rest;
            Some(top)
        } else {
            None
        };
        let term_a = if has_term {
            let [top, rest] =
                Layout::vertical([Constraint::Length(per), Constraint::Min(0)]).areas(remaining);
            remaining = rest;
            Some(top)
        } else {
            None
        };
        let grp_a = if has_groups && remaining.height > 0 {
            Some(remaining)
        } else {
            None
        };
        (bg_a, ix_a, term_a, grp_a)
    };

    if let Some(bg_area) = bg_area {
        let block = Block::default()
            .title_bottom(
                Line::from(Span::styled(" background ", Style::default().fg(DIM)))
                    .alignment(ratatui::layout::Alignment::Right),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(bg_area);
        frame.render_widget(block, bg_area);
        draw_agent_list(frame, inner, &bg_indices, app, ACCENT);
    }

    if let Some(ix_area) = ix_area {
        let block = Block::default()
            .title_bottom(
                Line::from(Span::styled(" interactive ", Style::default().fg(DIM)))
                    .alignment(ratatui::layout::Alignment::Right),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(ix_area);
        frame.render_widget(block, ix_area);
        draw_agent_list(frame, inner, &ix_indices, app, INTERACTIVE_COLOR);
    }

    if let Some(term_area) = term_area {
        let block = Block::default()
            .title_bottom(
                Line::from(Span::styled(" terminal ", Style::default().fg(DIM)))
                    .alignment(ratatui::layout::Alignment::Right),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(term_area);
        frame.render_widget(block, term_area);
        draw_agent_list(frame, inner, &term_indices, app, Color::Green);
    }

    if let Some(grp_area) = grp_area {
        let block = Block::default()
            .title_bottom(
                Line::from(Span::styled(" groups ", Style::default().fg(DIM)))
                    .alignment(ratatui::layout::Alignment::Right),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));
        let inner = block.inner(grp_area);
        frame.render_widget(block, grp_area);
        draw_groups_list(frame, inner, app);
    }

    if let Some(brain_area) = brain_area.filter(|area| area.height >= 3 && area.width >= 6) {
        if let Some(brain) = app.sidebar_brain.as_ref() {
            crate::tui::ui::panel::draw_brians_brain(frame, brain_area, brain);
        }
    }

    if let Some(dashboard_area) = dashboard_area {
        crate::tui::ui::system_dashboard::render_system_dashboard(
            frame,
            dashboard_area,
            &app.system_info,
            app.temperature_unit,
        );
    }
}

fn draw_agent_list(frame: &mut Frame, area: Rect, indices: &[usize], app: &mut App, accent: Color) {
    let card_h = 3u16;
    let row_h = 4u16;

    if area.height < card_h || indices.is_empty() {
        return;
    }

    // Calculate how many cards fit in the visible area
    let max_visible = ((area.height.saturating_sub(card_h)) / row_h + 1) as usize;

    // Find the selected agent's position within this list
    let selected_local = indices.iter().position(|&idx| idx == app.selected);

    // Compute scroll offset so the selected item is always visible
    let scroll_start = selected_local.map_or(0, |sel| {
        if sel >= max_visible {
            sel.saturating_sub(max_visible - 1)
        } else {
            0
        }
    });

    let has_scroll_up = scroll_start > 0;
    let has_scroll_down = indices.len().saturating_sub(scroll_start) > max_visible;

    let mut y = area.y;
    let end = indices.len().min(scroll_start + max_visible + 1);
    for (rel_i, &idx) in indices[scroll_start..end].iter().enumerate() {
        if y + card_h > area.y + area.height {
            break;
        }
        let card_area = Rect::new(area.x, y, area.width, card_h);
        let agent = &app.agents[idx];
        let selected = idx == app.selected;
        draw_sidebar_card(frame, card_area, agent, app, selected, accent);
        app.sidebar_click_map.push((idx, y, y + card_h));
        let global_i = scroll_start + rel_i;
        if global_i < indices.len() - 1 {
            y += row_h;
        } else {
            y += card_h;
        }
    }

    // Draw scroll indicators
    if has_scroll_up {
        let indicator = Paragraph::new("▲").style(Style::default().fg(DIM));
        let indicator_area = Rect::new(area.x + area.width.saturating_sub(2), area.y, 1, 1);
        frame.render_widget(indicator, indicator_area);
    }
    if has_scroll_down {
        let indicator = Paragraph::new("▼").style(Style::default().fg(DIM));
        let indicator_area = Rect::new(
            area.x + area.width.saturating_sub(2),
            area.y + area.height - 1,
            1,
            1,
        );
        frame.render_widget(indicator, indicator_area);
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
        AgentEntry::Terminal(idx) => app.terminal_agents[*idx].accent_color,
        _ => ACCENT,
    };

    let (mut status_color, agent_type, type_detail) = match agent {
        AgentEntry::Agent(a) => {
            let has_active = app.active_runs.contains_key(&a.id);
            let color = if !a.enabled {
                STATUS_DISABLED
            } else if has_active {
                STATUS_RUNNING
            } else if a.last_run_ok == Some(false) {
                STATUS_FAIL
            } else {
                STATUS_OK
            };
            (color, a.trigger_type_label(), a.cli.as_str())
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
        AgentEntry::Terminal(idx) => {
            let a = &app.terminal_agents[*idx];
            let color = match &a.status {
                AgentStatus::Running => STATUS_RUNNING,
                AgentStatus::Exited(0) => STATUS_OK,
                AgentStatus::Exited(_) => STATUS_FAIL,
            };
            (color, "term", a.shell.as_str())
        }
        AgentEntry::Group(_) => (STATUS_OK, "group", ""),
    };

    let mut is_waiting = match agent {
        AgentEntry::Interactive(idx) => app.interactive_agents[*idx].is_waiting_for_input(),
        AgentEntry::Terminal(idx) => app.terminal_agents[*idx].is_waiting_for_input(),
        _ => false,
    };

    // If the user has selected this agent and focused the agent/preview panel,
    // treat it as "attended" and suppress the waiting indicator so the user
    // isn't distracted while interacting.
    if selected && matches!(app.focus, Focus::Agent | Focus::Preview) {
        is_waiting = false;
    }

    // Blink animation: 10 ticks per phase (~500 ms on / 500 ms off) — quicker but
    // still readable.
    let blink_cycle = (app.animation_tick / 10) % 2;

    if is_waiting {
        status_color = if blink_cycle == 0 {
            super::STATUS_WAIT_ON
        } else {
            super::STATUS_WAIT_OFF
        };
    }

    // Line 1: accent bar + id + [▣] if in a group
    if area.height >= 1 {
        let accent_bar = Span::styled("▌", Style::default().fg(status_color));
        let name = agent.id(app);
        let in_group = app
            .split_groups
            .iter()
            .any(|g| g.session_a == name || g.session_b == name);
        let mut spans = vec![
            accent_bar,
            Span::raw(" "),
            Span::styled(
                name,
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(if selected { accent } else { Color::White }),
            ),
        ];
        if in_group {
            spans.push(Span::styled(" [▣]", Style::default().fg(DIM)));
        }
        let line = Line::from(spans);
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
            AgentEntry::Agent(a) => a.working_dir.as_deref().or_else(|| a.watch_path()),
            AgentEntry::Interactive(idx) => Some(app.interactive_agents[*idx].working_dir.as_str()),
            AgentEntry::Terminal(idx) => Some(app.terminal_agents[*idx].working_dir.as_str()),
            AgentEntry::Group(_) => None,
        };
        let dir_text = work_dir
            .filter(|d| !d.is_empty())
            .map(last_two_segments)
            .unwrap_or_else(|| "/".to_string());

        // Add waiting indicator if applicable
        let display_text = dir_text;

        let dir_span = Span::styled(display_text, Style::default().fg(DIM));
        let line = Line::from(vec![accent_bar, Span::raw(" "), dir_span]);
        let r = Rect::new(area.x, area.y + 2, area.width, 1);
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), r);
    }
}

fn draw_groups_list(frame: &mut Frame, area: Rect, app: &mut App) {
    // Collect the agent-list indices for Group entries
    let group_agent_indices: Vec<usize> = app
        .agents
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, AgentEntry::Group(_)))
        .map(|(i, _)| i)
        .collect();

    let mut y = area.y;
    for (pos, (&agent_idx, group)) in group_agent_indices
        .iter()
        .zip(app.split_groups.iter())
        .enumerate()
    {
        if y >= area.y + area.height {
            break;
        }
        let is_selected = agent_idx == app.selected;
        let is_active = app
            .active_split_id
            .as_deref()
            .is_some_and(|id| id == group.id);

        let label = format!("{} · {}", group.session_a, group.session_b);
        let bg = if is_selected {
            BG_SELECTED
        } else {
            Color::Reset
        };
        let fg = if is_selected {
            ACCENT // Use green (ACCENT) for selected group, like terminals
        } else if is_active {
            Color::Green
        } else {
            Color::White
        };
        let modifier = if is_active || is_selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };

        let prefix_color = if is_active { Color::Green } else { DIM };
        let active_tag = if is_active { " ●" } else { "" };

        let prefix = Span::styled("▌ ", Style::default().fg(prefix_color).bg(bg));
        let line = Line::from(vec![
            prefix,
            Span::styled(
                format!(
                    "{}{}",
                    truncate_str(&label, area.width.saturating_sub(6) as usize),
                    active_tag
                ),
                Style::default().fg(fg).bg(bg).add_modifier(modifier),
            ),
        ]);
        let r = Rect::new(area.x, y, area.width, 1);
        frame.render_widget(Paragraph::new(line), r);
        app.sidebar_click_map.push((agent_idx, y, y + 1));

        if pos < group_agent_indices.len() - 1 {
            y += 2; // spacer between groups
        } else {
            y += 1;
        }
    }
}
