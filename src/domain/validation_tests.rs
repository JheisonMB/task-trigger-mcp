use super::*;

// ── validate_id ───────────────────────────────────────────────

#[test]
fn test_validate_id_valid() {
    assert!(validate_id("my-background_agent").is_ok());
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

#[test]
fn test_validate_watch_path_root() {
    assert!(validate_watch_path("/").is_ok());
}

#[test]
fn test_validate_watch_path_with_special_chars() {
    assert!(validate_watch_path("/tmp/my-file_123.txt").is_ok());
}

#[test]
fn test_validate_watch_path_with_spaces_rejected() {
    assert!(validate_watch_path("/path with spaces").is_err());
}

#[test]
fn test_validate_id_exact_length() {
    let exact = "a".repeat(MAX_ID_LENGTH);
    assert!(validate_id(&exact).is_ok());
}

#[test]
fn test_validate_prompt_exact_length() {
    let exact = "x".repeat(MAX_PROMPT_LENGTH);
    assert!(validate_prompt(&exact).is_ok());
}
