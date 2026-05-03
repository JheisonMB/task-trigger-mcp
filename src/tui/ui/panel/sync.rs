use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::domain::sync::{MessageKind, MissionImpact, WorkspaceStatus};
use crate::tui::app::types::SyncPanelState;
use crate::tui::ui::{last_two_segments, truncate_str, ACCENT, DIM, ERROR_COLOR, STATUS_OK};

pub(crate) fn draw_sync_panel(frame: &mut Frame, area: Rect, state: &SyncPanelState) {
    let block = Block::default()
        .title(
            Line::from(Span::styled(" sync ", Style::default().fg(DIM)))
                .alignment(ratatui::layout::Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_style(Style::default().fg(DIM));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    draw_sync_section(frame, inner, state);
}

fn draw_sync_section(frame: &mut Frame, area: Rect, state: &SyncPanelState) {
    let vibe_fg = vibe_color(state.vibe);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("vibe ", Style::default().fg(DIM)),
            Span::styled(
                state.vibe.as_str(),
                Style::default().fg(vibe_fg).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled("sessions ", Style::default().fg(DIM)),
            Span::styled(
                state.participant_count.to_string(),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::from(vec![
            Span::styled("workdir ", Style::default().fg(DIM)),
            Span::styled(
                truncate_str(
                    &last_two_segments(&state.workdir),
                    area.width.saturating_sub(9) as usize,
                ),
                Style::default().fg(Color::White),
            ),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "active missions",
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        )),
    ];

    if state.active_intents.is_empty() {
        lines.push(Line::from(Span::styled(
            "No active missions",
            Style::default().fg(DIM),
        )));
    } else {
        for intent in &state.active_intents {
            lines.push(Line::from(vec![
                Span::styled("• ", Style::default().fg(intent_color(intent.impact))),
                Span::styled(
                    truncate_str(
                        &format!("{} · {}", intent.agent_name, intent.mission),
                        area.width.saturating_sub(2) as usize,
                    ),
                    Style::default().fg(Color::White),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    intent.impact.as_str(),
                    Style::default().fg(intent_color(intent.impact)),
                ),
                Span::raw(" · "),
                Span::styled(
                    intent.status.as_str(),
                    Style::default().fg(vibe_color(intent.status)),
                ),
            ]));
            if !intent.description.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        truncate_str(&intent.description, area.width.saturating_sub(2) as usize),
                        Style::default().fg(DIM),
                    ),
                ]));
            }
        }
    }

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "recent sync",
        Style::default().fg(DIM).add_modifier(Modifier::BOLD),
    )));

    let visible_messages = area.height.saturating_sub(lines.len() as u16) as usize;
    let start = state
        .recent_messages
        .len()
        .saturating_sub(visible_messages.max(1));
    for message in &state.recent_messages[start..] {
        let icon = match message.kind {
            MessageKind::Intent => "◉",
            MessageKind::Status => "≈",
            MessageKind::Query => "?",
            MessageKind::Answer => "↳",
            MessageKind::Info => "·",
        };
        lines.push(Line::from(vec![
            Span::styled(icon, Style::default().fg(kind_color(message.kind))),
            Span::raw(" "),
            Span::styled(
                truncate_str(
                    &format!("{} {}", message.agent_name, message.message),
                    area.width.saturating_sub(2) as usize,
                ),
                Style::default().fg(Color::White),
            ),
        ]));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .style(Style::default()),
        area,
    );
}

fn vibe_color(status: WorkspaceStatus) -> Color {
    match status {
        WorkspaceStatus::Stable => STATUS_OK,
        WorkspaceStatus::Unstable => ERROR_COLOR,
        WorkspaceStatus::Testing => Color::Yellow,
    }
}

fn intent_color(impact: MissionImpact) -> Color {
    match impact {
        MissionImpact::Low => ACCENT,
        MissionImpact::High => Color::Yellow,
        MissionImpact::Breaking => ERROR_COLOR,
    }
}

fn kind_color(kind: MessageKind) -> Color {
    match kind {
        MessageKind::Intent => ACCENT,
        MessageKind::Status => Color::Yellow,
        MessageKind::Query => Color::Cyan,
        MessageKind::Answer => STATUS_OK,
        MessageKind::Info => DIM,
    }
}
