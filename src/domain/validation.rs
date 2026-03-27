//! Domain validation rules for identifiers, prompts, and paths.

pub const MAX_ID_LENGTH: usize = 64;
pub const MAX_PROMPT_LENGTH: usize = 50_000;
pub const MAX_PATH_LENGTH: usize = 4096;

/// Validate an identifier: non-empty, max length, alphanumeric + hyphens/underscores.
pub fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("ID cannot be empty".to_string());
    }
    if id.len() > MAX_ID_LENGTH {
        return Err(format!(
            "ID exceeds maximum length of {MAX_ID_LENGTH} characters"
        ));
    }
    if !id
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "ID must contain only alphanumeric characters, hyphens, and underscores".to_string(),
        );
    }
    Ok(())
}

/// Validate a prompt string: non-empty, max length.
pub fn validate_prompt(prompt: &str) -> Result<(), String> {
    if prompt.trim().is_empty() {
        return Err("Prompt cannot be empty".to_string());
    }
    if prompt.len() > MAX_PROMPT_LENGTH {
        return Err(format!(
            "Prompt exceeds maximum length of {MAX_PROMPT_LENGTH} characters"
        ));
    }
    Ok(())
}

/// Validate a path string: non-empty, max length, absolute.
pub fn validate_watch_path(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("Path cannot be empty".to_string());
    }
    if path.len() > MAX_PATH_LENGTH {
        return Err(format!(
            "Path exceeds maximum length of {MAX_PATH_LENGTH} characters"
        ));
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err("Path must be absolute".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── validate_id ───────────────────────────────────────────────

    #[test]
    fn test_validate_id_valid() {
        assert!(validate_id("my-task").is_ok());
        assert!(validate_id("task_123").is_ok());
        assert!(validate_id("a").is_ok());
        assert!(validate_id("ABC-def_456").is_ok());
    }

    #[test]
    fn test_validate_id_empty() {
        assert!(validate_id("").is_err());
    }

    #[test]
    fn test_validate_id_too_long() {
        let long_id = "a".repeat(MAX_ID_LENGTH + 1);
        assert!(validate_id(&long_id).is_err());
        let exact_id = "a".repeat(MAX_ID_LENGTH);
        assert!(validate_id(&exact_id).is_ok());
    }

    #[test]
    fn test_validate_id_invalid_chars() {
        assert!(validate_id("has space").is_err());
        assert!(validate_id("has.dot").is_err());
        assert!(validate_id("has/slash").is_err());
        assert!(validate_id("has@at").is_err());
        assert!(validate_id("has\nnewline").is_err());
    }

    // ── validate_prompt ───────────────────────────────────────────

    #[test]
    fn test_validate_prompt_valid() {
        assert!(validate_prompt("Run the tests").is_ok());
        assert!(validate_prompt("a").is_ok());
    }

    #[test]
    fn test_validate_prompt_empty() {
        assert!(validate_prompt("").is_err());
        assert!(validate_prompt("   ").is_err());
        assert!(validate_prompt("\t\n").is_err());
    }

    #[test]
    fn test_validate_prompt_too_long() {
        let long = "x".repeat(MAX_PROMPT_LENGTH + 1);
        assert!(validate_prompt(&long).is_err());
        let exact = "x".repeat(MAX_PROMPT_LENGTH);
        assert!(validate_prompt(&exact).is_ok());
    }

    // ── validate_watch_path ───────────────────────────────────────

    #[test]
    fn test_validate_watch_path_valid() {
        assert!(validate_watch_path("/tmp/project").is_ok());
        assert!(validate_watch_path("/home/user/src").is_ok());
    }

    #[test]
    fn test_validate_watch_path_empty() {
        assert!(validate_watch_path("").is_err());
        assert!(validate_watch_path("   ").is_err());
    }

    #[test]
    fn test_validate_watch_path_relative() {
        assert!(validate_watch_path("relative/path").is_err());
        assert!(validate_watch_path("./here").is_err());
    }

    #[test]
    fn test_validate_watch_path_too_long() {
        let long = format!("/{}", "a".repeat(MAX_PATH_LENGTH));
        assert!(validate_watch_path(&long).is_err());
    }
}
