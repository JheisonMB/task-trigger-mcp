//! Footer rendering — context-sensitive key hints + version.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::DIM;
use crate::tui::app::types::{AgentEntry, App, Focus};

pub(super) fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let hints = match app.focus {
        Focus::Home => vec![
            ("↑↓", "select"),
            ("n", "new"),
            ("F10", "preview"),
            ("F1", "stats"),
        ],
        Focus::Preview => {
            let is_bg = matches!(app.selected_agent(), Some(AgentEntry::Agent(_)));
            let mut h = vec![("↑↓", "nav"), ("Enter", "focus")];
            if is_bg {
                h.push(("e", "edit"));
                h.push(("d", "toggle"));
                h.push(("F4", "delete"));
                h.push(("r", "rerun"));
            }
            h.push(("n", "new"));
            h.push(("Esc", "home"));
            h
        }
        Focus::NewAgentDialog => vec![
            ("↑↓", "fields"),
            ("←→", "cycle"),
            ("Space", "pick/enter"),
            ("Enter", "confirm"),
            ("Esc", "cancel"),
        ],
        Focus::Agent => {
            let is_pty = matches!(
                app.selected_agent(),
                Some(AgentEntry::Interactive(_))
                    | Some(AgentEntry::Terminal(_))
                    | Some(AgentEntry::Group(_))
            );
            let in_split = app.active_split_id.is_some();
            if is_pty {
                let mut h = vec![
                    ("F10", "preview"),
                    ("Esc", "home"),
                    ("Shift+↑↓", "agents"),
                    ("Ctrl+T", "context"),
                ];
                if in_split {
                    h.push(("F4", "dissolve"));
                    h.push(("Shift+F4", "end"));
                    h.push(("Shift+←→", "split focus"));
                } else {
                    h.push(("F4", "end"));
                }
                if matches!(app.selected_agent(), Some(AgentEntry::Terminal(_))) {
                    h.push(("Tab", "catalog"));
                    h.push(("Ctrl+W", "wrap"));
                }
                if matches!(app.selected_agent(), Some(AgentEntry::Interactive(_))) {
                    h.push(("Ctrl+B", "prompt"));
                }
                h.push(("Ctrl+N", "new"));
                h.push(("F1", "legend"));
                h
            } else {
                vec![
                    ("↑↓/jk", "scroll"),
                    ("F10", "preview"),
                    ("Esc", "home"),
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

    // Show split session names when in split view
    let split_label = if let Some(ref split_id) = app.active_split_id {
        app.split_groups
            .iter()
            .find(|g| g.id == *split_id)
            .map(|g| {
                let left_marker = if app.split_right_focused { " " } else { "●" };
                let right_marker = if app.split_right_focused { "●" } else { " " };
                format!(
                    " {left_marker} {} │ {} {right_marker} ",
                    g.session_a, g.session_b
                )
            })
    } else {
        None
    };

    let version = if app.daemon_version.is_empty() {
        String::new()
    } else {
        format!(" v{} ", app.daemon_version)
    };

    let hints_line = Line::from(spans);
    let hints_p = Paragraph::new(hints_line);
    frame.render_widget(hints_p, area);

    // Render split label + version on the right side
    let right_text = match (&split_label, version.is_empty()) {
        (Some(sl), false) => format!("{sl}{version}"),
        (Some(sl), true) => sl.clone(),
        (None, false) => version.clone(),
        (None, true) => String::new(),
    };
    let right_w = right_text.len() as u16;

    if right_w > 0 && area.width > right_w {
        let right_area = Rect::new(area.x + area.width - right_w, area.y, right_w, 1);

        let mut right_spans = Vec::new();
        if let Some(ref sl) = split_label {
            right_spans.push(Span::styled(
                sl.as_str(),
                Style::default()
                    .fg(super::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if !version.is_empty() {
            right_spans.push(Span::styled(
                &version,
                Style::default().fg(DIM).add_modifier(Modifier::BOLD),
            ));
        }
        let right_p = Paragraph::new(Line::from(right_spans));
        frame.render_widget(right_p, right_area);
    }
}
