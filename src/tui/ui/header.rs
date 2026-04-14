//! Header bar rendering — animated title + daemon status indicator.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{ACCENT, ERROR_COLOR};
use crate::tui::app::App;
use crate::tui::whimsg::TITLE;

/// Return the first `n` chars of `s`, respecting char boundaries.
fn first_n_chars(s: &str, n: usize) -> &str {
    let end = s
        .char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    &s[..end]
}

pub(super) fn draw_header(frame: &mut Frame, area: Rect, app: &mut App) {
    let status_text = if app.daemon_running {
        format!(" RUNNING (PID: {}) ", app.daemon_pid.unwrap_or(0))
    } else {
        " STOPPED ".to_string()
    };
    let status_w = status_text.chars().count() as u16;

    let wf = app.whimsg.tick();

    let spans: Vec<Span> = if wf.title_visible > 0 {
        // Title partially or fully visible
        let visible = first_n_chars(TITLE, wf.title_visible);
        vec![Span::styled(
            format!(" {visible}"),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )]
    } else if !wf.kaomoji.is_empty() && wf.text_visible == 0 && wf.text.is_empty() {
        // Kaomoji flash (no message yet)
        vec![Span::styled(
            format!(" {}", wf.kaomoji),
            Style::default().fg(Color::Rgb(102, 187, 106)),
        )]
    } else if !wf.kaomoji.is_empty() {
        // Kaomoji + partial/full message
        let visible_text = first_n_chars(&wf.text, wf.text_visible);
        vec![
            Span::styled(
                format!(" {} ", wf.kaomoji),
                Style::default().fg(Color::Rgb(102, 187, 106)),
            ),
            Span::styled(
                visible_text.to_string(),
                Style::default()
                    .fg(Color::Rgb(140, 140, 140))
                    .add_modifier(Modifier::ITALIC),
            ),
        ]
    } else {
        // Blank phase
        vec![Span::raw(" ")]
    };

    let left = Paragraph::new(Line::from(spans));
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
