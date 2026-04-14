//! Header bar rendering — title + daemon status indicator.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{ACCENT, ERROR_COLOR};
use crate::tui::app::App;

pub(super) fn draw_header(frame: &mut Frame, area: Rect, app: &mut App) {
    let status_text = if app.daemon_running {
        format!(" RUNNING (PID: {}) ", app.daemon_pid.unwrap_or(0))
    } else {
        " STOPPED ".to_string()
    };
    let status_w = status_text.chars().count() as u16;

    // Whimsical header: tick the generator and decide what to show
    let whim = app.whimsg.tick();
    let title_span = if let Some(msg) = whim {
        Span::styled(
            format!(" {msg}"),
            Style::default()
                .fg(Color::Rgb(180, 180, 180))
                .add_modifier(Modifier::ITALIC),
        )
    } else {
        Span::styled(
            " agent-canopy",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )
    };

    let left = Paragraph::new(Line::from(title_span));
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
