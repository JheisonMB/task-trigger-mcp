use chrono::Utc;
use regex::Regex;

#[allow(dead_code)]
pub fn substitute_variables(
    prompt: &str,
    task_id: &str,
    log_path: &str,
    file_path: Option<&str>,
    event_type: Option<&str>,
) -> String {
    let timestamp = Utc::now().to_rfc3339();

    let mut result = prompt.to_string();

    result = result.replace("{{TIMESTAMP}}", &timestamp);
    result = result.replace("{{TASK_ID}}", task_id);
    result = result.replace("{{LOG_PATH}}", log_path);

    if let Some(path) = file_path {
        result = result.replace("{{FILE_PATH}}", path);
    }

    if let Some(event) = event_type {
        result = result.replace("{{EVENT_TYPE}}", event);
    }

    result
}

/// Convierte expresión natural de horario a cron
/// Soporta patrones como "every day at 9am", "every 5 minutes", etc.
pub fn natural_to_cron(natural: &str) -> Option<String> {
    let natural_lower = natural.to_lowercase();
    let lower = natural_lower.trim();

    // "every 5 minutes" -> "*/5 * * * *"
    if let Some(caps) = Regex::new(r"every (\d+) minutes?")
        .ok()
        .and_then(|r| r.captures(lower))
    {
        if let Ok(minutes) = caps[1].parse::<u32>() {
            return Some(format!("*/{} * * * *", minutes));
        }
    }

    // "every hour" -> "0 * * * *"
    if lower.contains("every hour") {
        return Some("0 * * * *".to_string());
    }

    // "every day at 9am" -> "0 9 * * *"
    if let Some(caps) = Regex::new(r"every day at (\d+)(?::(\d+))?\s*(?:am|pm)?")
        .ok()
        .and_then(|r| r.captures(lower))
    {
        let hour = caps[1].parse::<u32>().ok()?;
        return Some(format!("0 {} * * *", hour));
    }

    // "daily at 9:30am" -> "30 9 * * *"
    if let Some(caps) = Regex::new(r"daily at (\d+):(\d+)")
        .ok()
        .and_then(|r| r.captures(lower))
    {
        let hour = caps[1].parse::<u32>().ok()?;
        let min = caps[2].parse::<u32>().ok()?;
        return Some(format!("{} {} * * *", min, hour));
    }

    // "weekly on monday at 10am" -> "0 10 * * 1"
    if let Some(caps) = Regex::new(r"weekly on (\w+) at (\d+)")
        .ok()
        .and_then(|r| r.captures(lower))
    {
        let day = match &caps[1].to_lowercase()[..] {
            "monday" => "1",
            "tuesday" => "2",
            "wednesday" => "3",
            "thursday" => "4",
            "friday" => "5",
            "saturday" => "6",
            "sunday" => "0",
            _ => return None,
        };
        let hour = caps[2].parse::<u32>().ok()?;
        return Some(format!("0 {} * * {}", hour, day));
    }

    // Si ya es un cron válido, retornarlo
    if is_valid_cron(lower) {
        return Some(lower.to_string());
    }

    None
}

fn is_valid_cron(expr: &str) -> bool {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 {
        return false;
    }

    // Validación básica: todos deben ser números, *, o rangos
    for part in parts {
        if part.is_empty() {
            return false;
        }
        if part != "*"
            && !part
                .chars()
                .all(|c| c.is_numeric() || c == '/' || c == '-' || c == ',')
        {
            return false;
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_variables() {
        let prompt = "Task {{TASK_ID}} at {{TIMESTAMP}} on {{FILE_PATH}}";
        let result = substitute_variables(
            prompt,
            "my-task",
            "/logs/my-task.log",
            Some("/home/file.txt"),
            None,
        );
        assert!(result.contains("my-task"));
        assert!(result.contains("/home/file.txt"));
        assert!(!result.contains("{{TIMESTAMP}}"));
    }

    #[test]
    fn test_natural_to_cron_minutes() {
        assert_eq!(
            natural_to_cron("every 5 minutes"),
            Some("*/5 * * * *".to_string())
        );
        assert_eq!(
            natural_to_cron("every 30 minutes"),
            Some("*/30 * * * *".to_string())
        );
    }

    #[test]
    fn test_natural_to_cron_hour() {
        assert_eq!(natural_to_cron("every hour"), Some("0 * * * *".to_string()));
    }
}
