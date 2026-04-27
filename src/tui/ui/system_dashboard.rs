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
                if let Some(temp) = system_info.cpu_temperature_celsius() {
                    format!("{:.0}% {:.0}C", system_info.cpu_usage_percent(), temp)
                } else {
                    format!("{:.0}%", system_info.cpu_usage_percent())
                },
                Style::default().fg(DIM),
            ),
        ]),
        // Memory line
        Line::from(vec![
            Span::styled("mem: ", Style::default().fg(Color::White)),
            Span::styled(
                format!(
                    "{:.1}/{:.1}GB ({:.0}%)",
                    system_info.memory_used_gb(),
                    system_info.memory_total_gb(),
                    if system_info.memory_total > 0 {
                        (system_info.memory_used as f32 / system_info.memory_total as f32) * 100.0
                    } else {
                        0.0
                    }
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
                    let metrics = match (gpu.usage, gpu.temperature) {
                        (Some(usage), Some(temp)) => Some(format!("{usage:.0}% {temp:.0}C")),
                        (Some(usage), None) => Some(format!("{usage:.0}%")),
                        (None, Some(temp)) => Some(format!("{temp:.0}C")),
                        (None, None) => None,
                    };
                    let gpu_text = metrics.unwrap_or_else(|| "n/a".to_string());
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

        // Should have 4 or 5 lines depending on whether GPU info is available
        assert!(
            lines.len() >= 4,
            "Expected at least 4 lines, got {}",
            lines.len()
        );
        assert!(
            lines.len() <= 5,
            "Expected at most 5 lines, got {}",
            lines.len()
        );
        // Check key lines exist regardless of GPU line position
        let all_text: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all_text.contains("cpu:"), "Missing cpu line");
        assert!(all_text.contains("mem:"), "Missing mem line");
        assert!(all_text.contains("uptime:"), "Missing uptime line");
        assert!(all_text.contains("canopy:"), "Missing canopy line");
        assert!(all_text.contains("2m"), "Should show 2 minutes uptime");
    }
}
