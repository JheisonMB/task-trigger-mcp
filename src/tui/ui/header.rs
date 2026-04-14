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

const SPINNER: [&str; 8] = ["⣷", "⣯", "⣟", "⡿", "⢿", "⣻", "⣽", "⣾"];

pub(super) fn draw_header(frame: &mut Frame, area: Rect, app: &mut App) {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let (status_char, status_color) = if app.daemon_running {
        // Smooth spinner in green, no blink
        let frame_idx = ((millis / 125) % 8) as usize;
        (SPINNER[frame_idx], ACCENT)
    } else {
        // Blinking █ in red when stopped
        let blink_on = (millis / 500) % 2 == 0;
        let color = if blink_on { ERROR_COLOR } else { Color::Rgb(120, 60, 60) };
        ("█", color)
    };

    let wf = app.whimsg.tick();

    let mut spans: Vec<Span> = Vec::new();
    // Leading padding so the green kaomoji/title block isn't flush against the left border
    spans.push(Span::raw(" "));

    if wf.title_visible > 0 {
        // Title partially or fully visible — dark text on green background
        let visible = first_n_chars(TITLE, wf.title_visible);
        spans.push(Span::styled(
            format!("{} ", visible),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(102, 187, 106))
                .add_modifier(Modifier::BOLD),
        ));
    } else if !wf.kaomoji.is_empty() && wf.text_visible == 0 && wf.text.is_empty() {
        // Kaomoji flash — dark text on green background
        spans.push(Span::styled(
            format!("{} ", wf.kaomoji),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(102, 187, 106)),
        ));
    } else if !wf.kaomoji.is_empty() {
        // Kaomoji with green background + message in gray without background
        let visible_text = first_n_chars(&wf.text, wf.text_visible);
        spans.push(Span::styled(
            format!("{} ", wf.kaomoji),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Rgb(102, 187, 106)),
        ));
        spans.push(Span::styled(
            format!("{} ", visible_text),
            Style::default()
                .fg(Color::Rgb(140, 140, 140))
                .add_modifier(Modifier::ITALIC),
        ));
    } else {
        // Blank phase — leading space already present
    }


    let left = Paragraph::new(Line::from(spans));
    frame.render_widget(left, area);

    // Daemon status: single character one cell from the right edge
    if area.width > 3 {
        let status = Paragraph::new(Line::from(Span::styled(
            status_char,
            Style::default().fg(status_color),
        )));
        let status_area = Rect::new(area.x + area.width - 2, area.y, 1, 1);
        frame.render_widget(status, status_area);
    }
}
