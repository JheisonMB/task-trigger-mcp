use super::*;

use chrono::Duration;

#[test]
fn test_task_not_expired_no_expiry() {
    let background_agent = BackgroundAgent {
        id: "t1".to_string(),
        prompt: "test".to_string(),
        schedule_expr: "* * * * *".to_string(),
        cli: Cli::new("opencode"),
        model: None,
        working_dir: None,
        enabled: true,
        created_at: Utc::now(),
        expires_at: None,
        last_run_at: None,
        last_run_ok: None,
        log_path: "/tmp/t.log".to_string(),
        timeout_minutes: 15,
    };
    assert!(!background_agent.is_expired());
}

#[test]
fn test_task_not_expired_future() {
    let background_agent = BackgroundAgent {
        id: "t2".to_string(),
        prompt: "test".to_string(),
        schedule_expr: "* * * * *".to_string(),
        cli: Cli::new("opencode"),
        model: None,
        working_dir: None,
        enabled: true,
        created_at: Utc::now(),
        expires_at: Some(Utc::now() + Duration::hours(1)),
        last_run_at: None,
        last_run_ok: None,
        log_path: "/tmp/t.log".to_string(),
        timeout_minutes: 15,
    };
    assert!(!background_agent.is_expired());
}

#[test]
fn test_task_expired_past() {
    let background_agent = BackgroundAgent {
        id: "t3".to_string(),
        prompt: "test".to_string(),
        schedule_expr: "* * * * *".to_string(),
        cli: Cli::new("opencode"),
        model: None,
        working_dir: None,
        enabled: true,
        created_at: Utc::now() - Duration::hours(2),
        expires_at: Some(Utc::now() - Duration::hours(1)),
        last_run_at: None,
        last_run_ok: None,
        log_path: "/tmp/t.log".to_string(),
        timeout_minutes: 15,
    };
    assert!(background_agent.is_expired());
}

#[test]
fn test_watch_event_from_str() {
    assert_eq!(WatchEvent::from_str("create"), Some(WatchEvent::Create));
    assert_eq!(WatchEvent::from_str("modify"), Some(WatchEvent::Modify));
    assert_eq!(WatchEvent::from_str("delete"), Some(WatchEvent::Delete));
    assert_eq!(WatchEvent::from_str("move"), Some(WatchEvent::Move));
    assert_eq!(WatchEvent::from_str("invalid"), None);
    assert_eq!(WatchEvent::from_str(""), None);
}

#[test]
fn test_watch_event_display() {
    assert_eq!(WatchEvent::Create.to_string(), "create");
    assert_eq!(WatchEvent::Modify.to_string(), "modify");
    assert_eq!(WatchEvent::Delete.to_string(), "delete");
    assert_eq!(WatchEvent::Move.to_string(), "move");
}

#[test]
fn test_cli_from_str() {
    assert_eq!(Cli::from_str("opencode").as_str(), "opencode");
    assert_eq!(Cli::from_str("kiro").as_str(), "kiro");
    assert_eq!(Cli::from_str("gemini").as_str(), "gemini");
    // Unknown strings are accepted as-is
    assert_eq!(Cli::from_str("unknown").as_str(), "unknown");
    // Empty string defaults to opencode
    assert_eq!(Cli::from_str("").as_str(), "opencode");
}

#[test]
fn test_cli_as_str() {
    assert_eq!(Cli::new("opencode").as_str(), "opencode");
    assert_eq!(Cli::new("kiro").as_str(), "kiro");
    assert_eq!(Cli::new("gemini").as_str(), "gemini");
}

#[test]
fn test_cli_display() {
    assert_eq!(format!("{}", Cli::new("opencode")), "opencode");
    assert_eq!(format!("{}", Cli::new("kiro")), "kiro");
    assert_eq!(format!("{}", Cli::new("gemini")), "gemini");
}

#[test]
fn test_cli_resolve_explicit_opencode() {
    assert_eq!(Cli::resolve(Some("opencode")).unwrap().as_str(), "opencode");
}

#[test]
fn test_cli_resolve_explicit_kiro() {
    assert_eq!(Cli::resolve(Some("kiro")).unwrap().as_str(), "kiro");
}

#[test]
fn test_cli_resolve_explicit_gemini() {
    assert_eq!(Cli::resolve(Some("gemini")).unwrap().as_str(), "gemini");
}

#[test]
fn test_cli_resolve_unknown_returns_ok() {
    // Any non-empty string is now valid; unknown CLIs fail at execution time
    assert_eq!(Cli::resolve(Some("vim")).unwrap().as_str(), "vim");
}

#[test]
fn test_parse_list_valid_events() {
    let input = vec!["create".to_string(), "modify".to_string()];
    let events = WatchEvent::parse_list(&input).unwrap();
    assert_eq!(events, vec![WatchEvent::Create, WatchEvent::Modify]);
}

#[test]
fn test_parse_list_all_events() {
    let input = vec![
        "create".to_string(),
        "modify".to_string(),
        "delete".to_string(),
        "move".to_string(),
    ];
    let events = WatchEvent::parse_list(&input).unwrap();
    assert_eq!(events.len(), 4);
}

#[test]
fn test_parse_list_invalid_event_returns_error() {
    let input = vec!["create".to_string(), "bogus".to_string()];
    let err = WatchEvent::parse_list(&input).unwrap_err();
    assert!(err.contains("Invalid event type 'bogus'"));
}

#[test]
fn test_parse_list_empty_returns_error() {
    let input: Vec<String> = vec![];
    let err = WatchEvent::parse_list(&input).unwrap_err();
    assert!(err.contains("At least one event type must be specified"));
}

#[test]
fn test_trigger_type_from_str() {
    assert!(matches!(
        TriggerType::from_str("scheduled"),
        TriggerType::Scheduled
    ));
    assert!(matches!(
        TriggerType::from_str("manual"),
        TriggerType::Manual
    ));
    assert!(matches!(TriggerType::from_str("watch"), TriggerType::Watch));
    // Unknown defaults to Scheduled
    assert!(matches!(
        TriggerType::from_str("unknown"),
        TriggerType::Scheduled
    ));
}

#[test]
fn test_trigger_type_roundtrip() {
    for tt in [
        TriggerType::Scheduled,
        TriggerType::Manual,
        TriggerType::Watch,
    ] {
        assert!(
            matches!(TriggerType::from_str(tt.as_str()), t if std::mem::discriminant(&t) == std::mem::discriminant(&tt))
        );
    }
}
