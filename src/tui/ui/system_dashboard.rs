//! System dashboard UI component for sidebar

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::DIM;
use crate::system::SystemInfo;

/// Render the system dashboard in the sidebar
pub fn render_system_dashboard(
    frame: &mut Frame,
    area: Rect,
    system_info: &SystemInfo,
    app_uptime_seconds: u64,
) {
    // Only render if we have enough space
    if area.height < 6 {
        return;
    }

    let dashboard = create_system_dashboard_lines(system_info, app_uptime_seconds);

    frame.render_widget(
        Paragraph::new(dashboard)
            .block(
                Block::default()
                    .title(Span::styled(" sysinfo ", Style::default().fg(DIM)))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(DIM)),
            )
            .style(Style::default().fg(DIM)),
        area,
    );
}

/// Create the lines for the system dashboard
fn create_system_dashboard_lines(
    system_info: &SystemInfo,
    app_uptime_seconds: u64,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        // CPU line
        Line::from(vec![
            Span::styled("cpu: ", Style::default().fg(Color::White)),
            Span::styled(
                format!("{:.0}%", system_info.cpu_usage_percent()),
                Style::default().fg(DIM),
            ),
        ]),
        // Memory line
        Line::from(vec![
            Span::styled("mem: ", Style::default().fg(Color::White)),
            Span::styled(
                format!(
                    "{:.1}/{:.1}GB",
                    system_info.memory_used_gb(),
                    system_info.memory_total_gb()
                ),
                Style::default().fg(DIM),
            ),
        ]),
        // Uptime line
        Line::from(vec![
            Span::styled("uptime: ", Style::default().fg(Color::White)),
            Span::styled(system_info.format_uptime(), Style::default().fg(DIM)),
        ]),
        // Canopy runtime line
        Line::from(vec![
            Span::styled("canopy: ", Style::default().fg(Color::White)),
            Span::styled(
                format!("{}m", app_uptime_seconds / 60),
                Style::default().fg(DIM),
            ),
        ]),
    ];

    // Only add GPU line if GPU info is available
    if system_info.gpu_info.is_some() {
        lines.insert(
            2,
            Line::from(vec![
                Span::styled("gpu: ", Style::default().fg(Color::White)),
                if let Some(gpu) = &system_info.gpu_info {
                    let gpu_text = if gpu.vendor.eq_ignore_ascii_case("system") {
                        gpu.name.clone()
                    } else {
                        format!("{} {}", gpu.vendor, gpu.name)
                    };
                    Span::styled(gpu_text, Style::default().fg(DIM))
                } else {
                    Span::styled("integrated", Style::default().fg(DIM))
                },
            ]),
        );
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::SystemInfo;

    #[test]
    fn test_dashboard_creation() {
        let info = SystemInfo::new();
        let lines = create_system_dashboard_lines(&info, 120); // 120 seconds uptime

        // Should have 4 lines (CPU, mem, uptime, canopy) since GPU is None
        assert_eq!(lines.len(), 4);
        assert!(lines[0].to_string().contains("cpu:"));
        assert!(lines[1].to_string().contains("mem:"));
        assert!(lines[2].to_string().contains("uptime:"));
        assert!(lines[3].to_string().contains("canopy:"));
        assert!(lines[3].to_string().contains("2m")); // Should show 2 minutes
    }
}
