//! System dashboard UI component for sidebar

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::DIM;
use crate::domain::canopy_config::TemperatureUnit;
use crate::system::SystemInfo;

/// Format bytes smartly: show in MB if < 1 GB, otherwise in GB, with 2 decimals
fn format_bytes_smart(bytes: u64) -> String {
    let gb = bytes as f32 / 1_073_741_824.0;
    if gb < 1.0 {
        let mb = bytes as f32 / 1_048_576.0;
        format!("{:.2}MB", mb)
    } else {
        format!("{:.2}GB", gb)
    }
}

/// Format megabytes smartly: show in MB if < 1024 MB, otherwise in GB, with 2 decimals
fn format_megabytes_smart(mb: u64) -> String {
    let gb = mb as f32 / 1024.0;
    if gb < 1.0 {
        format!("{:.2}MB", mb)
    } else {
        format!("{:.2}GB", gb)
    }
}

/// Render the system dashboard in the sidebar
pub fn render_system_dashboard(
    frame: &mut Frame,
    area: Rect,
    system_info: &SystemInfo,
    app_uptime_seconds: u64,
    temperature_unit: TemperatureUnit,
) {
    // Only render if we have enough space
    if area.height < 6 {
        return;
    }

    let dashboard =
        create_system_dashboard_lines(system_info, app_uptime_seconds, temperature_unit);

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
    temperature_unit: TemperatureUnit,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        // CPU line
        Line::from(vec![
            Span::styled("cpu: ", Style::default().fg(Color::White)),
            Span::styled(
                if let Some(temp) = system_info.cpu_temperature_celsius() {
                    format!(
                        "{:.0}% {}",
                        system_info.cpu_usage_percent(),
                        format_temperature(temp, temperature_unit)
                    )
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
                    "{:.0}% {}",
                    if system_info.memory_total > 0 {
                        (system_info.memory_used as f32 / system_info.memory_total as f32) * 100.0
                    } else {
                        0.0
                    },
                    format_bytes_smart(system_info.memory_used)
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
                    // Format VRAM if available (similar to memory format: percentage first, then used size)
                    let vram_text = if let (Some(vram_used), Some(vram_total)) = (gpu.vram_used, gpu.vram_total) {
                        if vram_total > 0 {
                            let vram_percent = (vram_used as f32 / vram_total as f32) * 100.0;
                            Some(format!("{:.0}% {}", vram_percent, format_megabytes_smart(vram_used)))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Combine usage, temperature, and VRAM
                    let metrics = match (gpu.usage, gpu.temperature, vram_text) {
                        (Some(usage), Some(temp), Some(vram)) => 
                            Some(format!("{usage:.0}% {} | {vram}", format_temperature(temp, temperature_unit))),
                        (Some(usage), Some(temp), None) => 
                            Some(format!("{usage:.0}% {}", format_temperature(temp, temperature_unit))),
                        (Some(usage), None, Some(vram)) => 
                            Some(format!("{usage:.0}% | {vram}")),
                        (Some(usage), None, None) => 
                            Some(format!("{usage:.0}%")),
                        (None, Some(temp), Some(vram)) => 
                            Some(format!("{} | {vram}", format_temperature(temp, temperature_unit))),
                        (None, Some(temp), None) => 
                            Some(format_temperature(temp, temperature_unit)),
                        (None, None, Some(vram)) => 
                            Some(vram),
                        (None, None, None) => None,
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

fn format_temperature(temp_celsius: f32, unit: TemperatureUnit) -> String {
    match unit {
        TemperatureUnit::Celsius => format!("{temp_celsius:.0}°C"),
        TemperatureUnit::Fahrenheit => {
            let temp_f = temp_celsius * 9.0 / 5.0 + 32.0;
            format!("{temp_f:.0}°F")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::SystemInfo;

    #[test]
    fn test_dashboard_creation() {
        let info = SystemInfo::new();
        let lines = create_system_dashboard_lines(&info, 120, TemperatureUnit::Celsius); // 120 seconds uptime

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
