//! Right panel rendering — PTY output, brain automaton, banner, background_agent/watcher details, log.

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::{
    truncate_str, ACCENT, DIM, INTERACTIVE_COLOR, STATUS_DISABLED, STATUS_FAIL, STATUS_OK,
    STATUS_RUNNING,
};
use crate::tui::app::types::{AgentEntry, App, Focus, ProjectsPanelFocus};

pub mod details;
pub mod home;
pub mod log_fallback;
pub mod sync;
pub mod vt100;
pub mod warp;

pub use details::{draw_agent_details, draw_group_details};
pub(crate) use home::draw_brians_brain;
pub use log_fallback::draw_log_text;
pub(crate) use sync::draw_sync_panel;
pub use vt100::render_vt_screen;
#[allow(unused_imports)]
pub use warp::compact_cwd;
pub use warp::{draw_warp_input_box, render_command_chips};

use home::draw_canopy_banner_glitch;
use vt100::{render_indicators, render_vt_screen_with_mask};

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

    // Store actual PTY area dimensions so resize and mouse forwarding match exactly.
    app.last_panel_inner = (inner.width, inner.height);

    // When there are no agents, show the home view instead of "No agent selected"
    if app.agents.is_empty()
        && app.sidebar_mode != crate::tui::app::SidebarMode::Projects
        && !matches!(
            app.focus,
            Focus::NewAgentDialog
                | Focus::ContextTransfer
                | Focus::RagTransfer
                | Focus::PromptTemplateDialog
        )
    {
        if let Some(brain) = app.home_brain.as_ref() {
            draw_brians_brain(frame, inner, brain);
        }
        draw_canopy_banner_glitch(frame, inner, app);
        return;
    }

    match app.focus {
        Focus::Home => {
            if app.sidebar_mode == crate::tui::app::SidebarMode::Projects {
                draw_projects_mode_panel(frame, inner, app);
                return;
            }
            if let Some(brain) = app.home_brain.as_ref() {
                draw_brians_brain(frame, inner, brain);
            }
            draw_canopy_banner_glitch(frame, inner, app);
            return;
        }

        Focus::Preview => {
            if app.playground_active {
                draw_playground_panel(frame, inner, app);
                return;
            }
            if app.sidebar_mode == crate::tui::app::SidebarMode::Projects {
                draw_projects_mode_panel(frame, inner, app);
                return;
            }
            if app.agents_rag_focused && app.rag_info.total_chunks > 0 {
                draw_rag_info_overview(frame, inner, app);
                return;
            }
            match app.selected_agent() {
                Some(AgentEntry::Agent(a)) => {
                    draw_agent_details(frame, inner, a, app);
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
                            render_command_chips(frame, inner, app, &agent.name);
                            return;
                        }
                    }
                }
                Some(AgentEntry::Group(idx)) => {
                    draw_group_details(frame, inner, app, *idx);
                    return;
                }
                _ => {}
            }
        }

        Focus::Agent => match app.selected_agent() {
            Some(AgentEntry::Interactive(idx)) => {
                let idx = *idx;
                if let Some(agent) = app.interactive_agents.get(idx) {
                    if let Some(snap) = agent.screen_snapshot() {
                        let sensitive = agent.is_sensitive_input_active();
                        render_vt_screen_with_mask(frame, inner, &snap, sensitive);
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
                    let warp = agent.warp_mode;
                    let sensitive = agent.is_sensitive_input_active();
                    if warp {
                        // Split: PTY output above, warp input box below
                        let input_h = 3u16;
                        let pty_h = inner.height.saturating_sub(input_h);
                        let pty_area = Rect::new(inner.x, inner.y, inner.width, pty_h);
                        let input_area = Rect::new(inner.x, inner.y + pty_h, inner.width, input_h);

                        if let Some(snap) = agent.screen_snapshot() {
                            render_vt_screen_with_mask(frame, pty_area, &snap, sensitive);
                            render_indicators(frame, pty_area, &snap, app);
                        }

                        draw_warp_input_box(frame, input_area, app, idx);
                        app.last_panel_inner = (pty_area.width, pty_area.height);
                        return;
                    }

                    if let Some(snap) = agent.screen_snapshot() {
                        render_vt_screen_with_mask(frame, inner, &snap, sensitive);
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
                    draw_canopy_banner_glitch(frame, inner, app);
                    return;
                }
                _ => {}
            }
        }

        Focus::ContextTransfer => {
            // The context transfer modal is drawn as an overlay in ui/mod.rs.
            // Fall through to draw the underlying panel as background.
        }
        Focus::RagTransfer => {
            // The RAG transfer modal is drawn as an overlay in ui/mod.rs.
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

fn draw_project_overview(frame: &mut Frame, area: Rect, app: &App) {
    let Some(project) = app.selected_project() else {
        frame.render_widget(
            Paragraph::new("No registered projects").style(Style::default().fg(DIM)),
            area,
        );
        return;
    };

    let tags = project.tags.as_deref().unwrap_or("none");
    let indexed = project
        .indexed_at
        .map(|ts| ts.to_string())
        .unwrap_or_else(|| "pending".to_string());
    let description = project
        .description
        .as_deref()
        .unwrap_or("No description extracted yet.");
    let lines = vec![
        Line::from(vec![
            Span::styled("Project ", Style::default().fg(DIM)),
            Span::styled(
                &project.name,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(format!("Hash: {}", project.hash)),
        Line::from(format!("Path: {}", project.path)),
        Line::from(format!("Indexed: {}", indexed)),
        Line::from(format!("Tags: {}", tags)),
        Line::from(""),
        Line::from(description),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

fn draw_projects_mode_panel(frame: &mut Frame, area: Rect, app: &App) {
    if app.playground_active {
        draw_playground_panel(frame, area, app);
        return;
    }

    match app.projects_panel_focus {
        ProjectsPanelFocus::Projects => draw_project_overview(frame, area, app),
        ProjectsPanelFocus::RagInfo => draw_rag_queue_overview(frame, area, app),
    }
}

fn draw_rag_queue_overview(frame: &mut Frame, area: Rect, app: &App) {
    draw_rag_info_overview(frame, area, app);
}

fn draw_rag_info_overview(frame: &mut Frame, area: Rect, app: &App) {
    let status_span = if app.rag_paused {
        Span::styled(" ⏸ paused", Style::default().fg(Color::Yellow))
    } else if app.rag_info.processing_items > 0 {
        Span::styled(" ◉ indexing", Style::default().fg(Color::Yellow))
    } else {
        Span::styled(" ✓ idle", Style::default().fg(ACCENT))
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("Chunks: ", Style::default().fg(DIM)),
            Span::styled(
                app.rag_info.total_chunks.to_string(),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled("Indexed: ", Style::default().fg(DIM)),
            Span::styled(
                app.rag_info.indexed_projects.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Queue: ", Style::default().fg(DIM)),
            Span::styled(
                format!(
                    "{} queued · {} processing",
                    app.rag_info.queued_items, app.rag_info.processing_items
                ),
                Style::default().fg(Color::White),
            ),
            Span::raw("  "),
            status_span,
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter to open the global RAG playground.",
            Style::default().fg(ACCENT),
        )),
    ];

    if !app.global_rag_queue.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("Queue items ", Style::default().fg(DIM)),
            Span::styled(
                format!("({})", app.global_rag_queue.len()),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]));
        for (idx, item) in app.global_rag_queue.iter().enumerate().take(8) {
            let selected = idx == app.selected_rag_queue;
            let status_color = if item.status == "processing" {
                Color::Yellow
            } else {
                ACCENT
            };
            let line_style = if selected {
                Style::default().bg(super::BG_SELECTED)
            } else {
                Style::default()
            };
            let marker = if selected { "›" } else { " " };
            lines.push(Line::from(vec![
                Span::styled(marker, line_style.fg(status_color)),
                Span::raw(" "),
                Span::styled(
                    item.project_name.as_deref().unwrap_or(&item.project_hash),
                    line_style.fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("  {}", item.status), line_style.fg(DIM)),
            ]));
        }
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

fn draw_playground_panel(frame: &mut Frame, area: Rect, app: &App) {
    if app.playground_detail_mode {
        draw_playground_detail(frame, area, app);
    } else {
        draw_playground_list(frame, area, app);
    }
}

fn draw_playground_list(frame: &mut Frame, area: Rect, app: &App) {
    let scope_label = if let Some(hash) = &app.playground_project_hash {
        let name = app
            .projects
            .iter()
            .find(|p| &p.hash == hash)
            .map(|p| p.name.as_str())
            .unwrap_or("Unknown Project");
        format!("Project: {name}")
    } else {
        "Global".to_string()
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("RAG Playground ", Style::default().fg(DIM)),
            Span::styled(
                format!("({scope_label}) "),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(
                format!("· {}", app.playground_query),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "Type to search · ↑↓ navigate · Tab toggle scope · Enter focus · Ctrl+T transfer · Esc close",
            Style::default().fg(DIM),
        )),
        Line::from(""),
    ];

    if app.playground_results.is_empty() {
        lines.push(Line::from(Span::styled(
            if app.playground_query.trim().is_empty() {
                "Start typing to search indexed chunks."
            } else {
                "No matching chunks."
            },
            Style::default().fg(DIM),
        )));
        frame.render_widget(
            Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
            area,
        );
        return;
    }

    let max_visible = ((area.height.saturating_sub(4)) / 5).max(1) as usize;
    let total = app.playground_results.len();
    let scroll_start = if app.playground_selected >= max_visible {
        app.playground_selected.saturating_sub(max_visible - 1)
    } else {
        0
    };

    for (idx, chunk) in app
        .playground_results
        .iter()
        .enumerate()
        .skip(scroll_start)
        .take(max_visible)
    {
        let selected = idx == app.playground_selected;
        let style = if selected {
            Style::default().bg(super::BG_SELECTED)
        } else {
            Style::default()
        };
        let marker = if selected { "›" } else { " " };
        let project_name = app
            .projects
            .iter()
            .find(|p| p.hash == chunk.project_hash)
            .map(|p| p.name.as_str())
            .unwrap_or("?");

        let path = format!("{} · {} [{}]", project_name, chunk.source_path, chunk.lang);
        lines.push(Line::from(vec![
            Span::styled(marker, style.fg(ACCENT)),
            Span::raw(" "),
            Span::styled(
                truncate_str(&path, area.width.saturating_sub(3) as usize),
                style.fg(Color::White).add_modifier(Modifier::BOLD),
            ),
        ]));

        // Show up to 3 content preview lines
        let content_lines: Vec<&str> = chunk.content.lines().take(3).collect();
        for line in content_lines {
            let preview = truncate_str(line, area.width.saturating_sub(6) as usize);
            lines.push(Line::from(vec![
                Span::styled("  ", style),
                Span::styled(preview, style.fg(DIM)),
            ]));
        }
        lines.push(Line::from(""));
    }

    // Footer with count
    if total > max_visible {
        lines.push(Line::from(Span::styled(
            format!("  {}/{} results", app.playground_selected + 1, total),
            Style::default().fg(DIM),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

fn draw_playground_detail(frame: &mut Frame, area: Rect, app: &App) {
    let Some(chunk) = app.playground_results.get(app.playground_selected) else {
        return;
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled("‹ ", Style::default().fg(ACCENT)),
            Span::styled(
                format!(
                    "{} [{} · chunk {}]",
                    chunk.source_path, chunk.lang, chunk.chunk_index
                ),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "↑↓ scroll · Enter/Ctrl+T transfer · Esc back to list",
            Style::default().fg(DIM),
        )),
        Line::from(""),
    ];

    let content_lines: Vec<&str> = chunk.content.lines().collect();
    let visible_lines = area.height.saturating_sub(5) as usize;
    let scroll = app.playground_scroll as usize;
    let start = scroll.min(content_lines.len().saturating_sub(1));
    let end = (start + visible_lines).min(content_lines.len());

    for line in &content_lines[start..end] {
        lines.push(Line::from(Span::styled(
            truncate_str(line, area.width as usize),
            Style::default().fg(Color::White),
        )));
    }

    if content_lines.len() > visible_lines {
        let pct = if !content_lines.is_empty() {
            ((start + visible_lines).min(content_lines.len()) * 100 / content_lines.len()).min(100)
        } else {
            100
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("  ── {pct}% ──"),
            Style::default().fg(DIM),
        )));
    }

    frame.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
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

    // Check if this is a warp-mode terminal
    let warp_terminal_idx = match &found {
        Some(SessionRef::Terminal(idx)) if app.terminal_agents[*idx].warp_mode => Some(*idx),
        _ => None,
    };

    if let Some(t_idx) = warp_terminal_idx {
        let input_h = 3u16;
        let pty_h = inner.height.saturating_sub(input_h);
        let pty_area = Rect::new(inner.x, inner.y, inner.width, pty_h);
        let input_area = Rect::new(inner.x, inner.y + pty_h, inner.width, input_h);

        if let Some(snap) = snap {
            render_vt_screen(frame, pty_area, &snap);
            render_indicators(frame, pty_area, &snap, app);
        }

        if focused && matches!(app.focus, Focus::Agent) {
            draw_warp_input_box(frame, input_area, app, t_idx);
        }
    } else if let Some(snap) = snap {
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
