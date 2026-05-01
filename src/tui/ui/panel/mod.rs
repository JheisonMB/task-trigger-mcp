//! Right panel rendering — PTY output, brain automaton, banner, background_agent/watcher details, log.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::{
    ACCENT, DIM, INTERACTIVE_COLOR, STATUS_DISABLED, STATUS_FAIL, STATUS_OK, STATUS_RUNNING,
};
use crate::tui::app::types::{AgentEntry, App, Focus};

pub mod details;
pub mod home;
pub mod log_fallback;
pub mod vt100;
pub mod warp;

pub use details::{draw_agent_details, draw_group_details};
pub(crate) use home::draw_brians_brain;
pub use log_fallback::draw_log_text;
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

    // Store actual inner dimensions so PTY resize matches exactly
    app.last_panel_inner = (inner.width, inner.height);

    // When there are no agents, show the home view instead of "No agent selected"
    if app.agents.is_empty()
        && !matches!(
            app.focus,
            Focus::NewAgentDialog | Focus::ContextTransfer | Focus::PromptTemplateDialog
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
            if let Some(brain) = app.home_brain.as_ref() {
                draw_brians_brain(frame, inner, brain);
            }
            draw_canopy_banner_glitch(frame, inner, app);
            return;
        }

        Focus::Preview => match app.selected_agent() {
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
        },

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
