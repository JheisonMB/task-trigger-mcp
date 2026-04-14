//! UI rendering — sidebar with agent cards, log panel, header, footer, and dialogs.

mod dialogs;
mod footer;
mod header;
mod panel;
mod sidebar;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Color;
use ratatui::Frame;

use super::app::App;

// ── Shared palette ──────────────────────────────────────────────

pub(crate) const ACCENT: Color = Color::Rgb(76, 175, 80);
pub(crate) const DIM: Color = Color::Rgb(150, 150, 170);
pub(crate) const ERROR_COLOR: Color = Color::Rgb(229, 57, 53);
pub(crate) const BG_SELECTED: Color = Color::Rgb(20, 40, 20);
pub(crate) const INTERACTIVE_COLOR: Color = Color::Rgb(102, 187, 106);
pub(crate) const STATUS_DISABLED: Color = Color::Rgb(120, 120, 120);
pub(crate) const STATUS_RUNNING: Color = Color::Rgb(76, 175, 80);
pub(crate) const STATUS_OK: Color = Color::Rgb(66, 165, 245);
pub(crate) const STATUS_FAIL: Color = Color::Rgb(229, 57, 53);

// ── Main draw entry point ───────────────────────────────────────

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header_area, body, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    if app.sidebar_visible {
        let [sidebar, panel] =
            Layout::horizontal([Constraint::Length(26), Constraint::Min(0)]).areas(body);
        header::draw_header(frame, header_area, app);
        sidebar::draw_sidebar(frame, sidebar, app);
        panel::draw_log_panel(frame, panel, app);
    } else {
        header::draw_header(frame, header_area, app);
        panel::draw_log_panel(frame, body, app);
    }
    footer::draw_footer(frame, footer_area, app);

    if app.new_agent_dialog.is_some() {
        dialogs::draw_new_agent_dialog(frame, app);
    }

    if app.quit_confirm {
        dialogs::draw_quit_confirm(frame);
    }

    if app.show_legend {
        dialogs::draw_legend(frame);
    }

    if app.context_transfer_modal.is_some() {
        dialogs::draw_context_transfer_modal(frame, app);
    }

    // Top-level overlays rendered last so they appear above all content
    if app.show_copied {
        let full = frame.area();
        let msg = " \u{2592} COPIED \u{2592} "; // ▒ COPIED ▒
        let w = msg.chars().count() as u16; // display width (char count, not bytes)
        if full.width > w + 2 {
            let x = full.x + full.width - w - 1;
            let y = full.y + 1; // just below header
            let area = ratatui::layout::Rect::new(x, y, w, 1);
            let widget = ratatui::widgets::Paragraph::new(msg)
                .style(ratatui::style::Style::default().fg(ACCENT).bg(Color::Black));
            frame.render_widget(widget, area);
        }
    }
}

// ── Shared helpers ──────────────────────────────────────────────

/// Create a centered rect of given percentage width and fixed height.
pub(crate) fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let [_, center, _] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(height),
        Constraint::Fill(1),
    ])
    .areas(area);

    let [_, center, _] = Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .areas(center);

    center
}

pub(crate) fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else if max > 1 {
        format!("{}…", &s[..max - 1])
    } else {
        String::new()
    }
}

/// Extract the last two path segments, e.g. `/a/b/c/d` → `c/d`.
pub(crate) fn last_two_segments(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let parts: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return "/".to_string();
    }
    if parts.len() <= 2 {
        return trimmed.to_string();
    }
    format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1])
}
