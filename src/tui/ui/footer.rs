//! Footer rendering — context-sensitive key hints + version.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::DIM;
use crate::tui::app::{AgentEntry, App, Focus};

pub(super) fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints = match app.focus {
        Focus::Home => vec![
            ("↑↓", "select"),
            ("n", "new"),
            ("q", "quit"),
            ("F1", "legend"),
        ],
        Focus::Preview => vec![
            ("↑↓", "nav"),
            ("Enter", "focus"),
            ("e", "edit"),
            ("d", "toggle"),
            ("D", "delete"),
            ("r", "rerun"),
            ("n", "new"),
            ("q", "quit"),
        ],
        Focus::NewAgentDialog => vec![
            ("↑↓", "fields"),
            ("←→", "change"),
            ("Space", "enter dir"),
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Focus::Agent => {
            let is_pty = matches!(
                app.selected_agent(),
                Some(AgentEntry::Interactive(_)) | Some(AgentEntry::Terminal(_))
            );
            let in_split = app.active_split_id.is_some();
            if is_pty {
                let mut h = vec![
                    ("F4", "end"),
                    ("F10", "back"),
                    ("Ctrl+↑↓", "agents"),
                    ("Ctrl+T", "context"),
                ];
                if matches!(app.selected_agent(), Some(AgentEntry::Interactive(_))) {
                    h.push(("Ctrl+B", "prompt"));
                }
                if in_split {
                    h.push(("Ctrl+←→", "split focus"));
                    h.push(("Ctrl+X", "dissolve"));
                } else {
                    h.push(("Ctrl+S", "split"));
                }
                h.push(("Ctrl+N", "new"));
                h.push(("F1", "legend"));
                h
            } else {
                vec![
                    ("↑↓/jk", "scroll"),
                    ("Esc", "back"),
                    ("Ctrl+N", "new"),
                    ("F1", "legend"),
                ]
            }
        }
        Focus::ContextTransfer => vec![
            ("↑↓", "select"),
            ("Tab/Enter", "next step"),
            ("Esc", "cancel"),
        ],
        Focus::PromptTemplateDialog => vec![
            ("↑↓", "fields"),
            ("⇧↑↓←→", "cursor"),
            ("Ctrl+S", "send"),
            ("Ctrl+A/X", "add/remove"),
            ("Esc", "cancel"),
        ],
    };

    let mut spans = Vec::new();
    spans.push(Span::raw("  "));
    for (i, (key, desc)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            *key,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*desc, Style::default().fg(DIM)));
    }

    let version = if app.daemon_version.is_empty() {
        String::new()
    } else {
        format!(" v{} ", app.daemon_version)
    };
    let version_w = version.len() as u16;

    let hints_line = Line::from(spans);
    let hints_p = Paragraph::new(hints_line);
    frame.render_widget(hints_p, area);

    if version_w > 0 && area.width > version_w {
        let ver_area = Rect::new(area.x + area.width - version_w, area.y, version_w, 1);
        let version_span = Span::styled(
            &version,
            Style::default().fg(DIM).add_modifier(Modifier::BOLD),
        );
        let ver_p = Paragraph::new(Line::from(version_span));
        frame.render_widget(ver_p, ver_area);
    }
}
