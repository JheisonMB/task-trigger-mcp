//! System notifications — cross-platform desktop notifications.
//!
//! Sends native notifications when agents complete or fail.
//! Detected platforms: WSL → Windows toast, macOS → osascript, Linux → notify-send.
//! All notifications are fire-and-forget on a background thread.

use std::process::Command;

/// Detected runtime platform for notification dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    Wsl,
    MacOs,
    Linux,
}

fn detect_platform() -> Platform {
    if cfg!(target_os = "macos") {
        return Platform::MacOs;
    }

    // WSL: /proc/version contains "microsoft" or "Microsoft"
    if let Ok(ver) = std::fs::read_to_string("/proc/version") {
        if ver.to_lowercase().contains("microsoft") {
            return Platform::Wsl;
        }
    }

    Platform::Linux
}

/// Escape a string for use inside a PowerShell single-quoted string.
fn ps_escape(s: &str) -> String {
    s.replace('\'', "''")
}

/// Escape a string for use inside an AppleScript double-quoted string.
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn send_linux(title: &str, body: &str) {
    let _ = Command::new("notify-send")
        .arg("--app-name=Canopy")
        .arg(title)
        .arg(body)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn send_macos(title: &str, body: &str) {
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript_escape(body),
        applescript_escape(title),
    );
    let _ = Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn send_wsl(title: &str, body: &str) {
    // Clear any stale Canopy notifications from the Windows Action Center first
    // to prevent notification pile-up that keeps re-appearing.
    let clear_script = concat!(
        "Get-AppxPackage | Where-Object { $_.Name -like '*Canopy*' } | ForEach-Object { ",
        "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ",
        "ContentType = WindowsRuntime] > $null; ",
        "try { [Windows.UI.Notifications.ToastNotificationManager]::",
        "CreateToastNotifier('Canopy').Clear() } catch {} ",
        "}; "
    );
    let _ = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(clear_script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();

    // Use BurntToast-style toast via WinRT API with explicit dismissal time.
    // Creates the toast, shows it, and schedules removal from Action Center after 5 seconds.
    let ps_script = format!(
        concat!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ",
            "ContentType = WindowsRuntime] > $null; ",
            "$template = [Windows.UI.Notifications.ToastNotificationManager]::",
            "GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); ",
            "$nodes = $template.GetElementsByTagName('text'); ",
            "$nodes.Item(0).AppendChild($template.CreateTextNode('{}')) > $null; ",
            "$nodes.Item(1).AppendChild($template.CreateTextNode('{}')) > $null; ",
            "$toast = [Windows.UI.Notifications.ToastNotification]::new($template); ",
            "$toast.ExpirationTime = [DateTimeOffset]::UtcNow.Add([TimeSpan]::FromSeconds(5)); ",
            "[Windows.UI.Notifications.ToastNotificationManager]::",
            "CreateToastNotifier('Canopy').Show($toast)"
        ),
        ps_escape(title),
        ps_escape(body),
    );
    let _ = Command::new("powershell.exe")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(&ps_script)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Clear any stale Canopy notifications from the Windows Action Center.
/// Call this once at startup to prevent pile-up of old notifications.
pub fn clear_stale_notifications() {
    let platform = detect_platform();
    if platform != Platform::Wsl {
        return;
    }
    std::thread::spawn(move || {
        let clear_script = concat!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ",
            "ContentType = WindowsRuntime] > $null; ",
            "try { [Windows.UI.Notifications.ToastNotificationManager]::",
            "CreateToastNotifier('Canopy').Clear() } catch {}"
        );
        let _ = Command::new("powershell.exe")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(clear_script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    });
}

/// Send a desktop notification. Fire-and-forget — spawns a background thread
/// and never blocks the caller. Failures are silently ignored.
pub fn send_notification(title: &str, body: &str) {
    let title = title.to_owned();
    let body = body.to_owned();
    std::thread::spawn(move || {
        let platform = detect_platform();
        match platform {
            Platform::Wsl => send_wsl(&title, &body),
            Platform::MacOs => send_macos(&title, &body),
            Platform::Linux => send_linux(&title, &body),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_platform_returns_valid_enum() {
        let platform = detect_platform();
        assert!(
            matches!(platform, Platform::Wsl | Platform::MacOs | Platform::Linux),
            "Platform should be one of the three variants"
        );
    }

    #[test]
    fn test_ps_escape_single_quotes() {
        assert_eq!(ps_escape("hello"), "hello");
        assert_eq!(ps_escape("it's"), "it''s");
        assert_eq!(ps_escape("don't"), "don''t");
        assert_eq!(ps_escape("''"), "''''");
    }

    #[test]
    fn test_ps_escape_empty() {
        assert_eq!(ps_escape(""), "");
    }

    #[test]
    fn test_applescript_escape_backslash() {
        assert_eq!(applescript_escape("hello"), "hello");
        assert_eq!(applescript_escape("hello\\world"), "hello\\\\world");
        assert_eq!(applescript_escape("a\\b\\c"), "a\\\\b\\\\c");
    }

    #[test]
    fn test_applescript_escape_quotes() {
        assert_eq!(applescript_escape("say \"hello\""), "say \\\"hello\\\"");
        assert_eq!(applescript_escape("it's"), "it's");
    }

    #[test]
    fn test_applescript_escape_empty() {
        assert_eq!(applescript_escape(""), "");
    }

    #[test]
    fn test_applescript_escape_combined() {
        assert_eq!(
            applescript_escape("say \"hello\\world\""),
            "say \\\"hello\\\\world\\\""
        );
    }
}
