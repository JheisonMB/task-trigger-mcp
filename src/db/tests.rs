use super::*;
use crate::domain::models::{
    BackgroundAgent, Cli, RunLog, RunStatus, TriggerType, WatchEvent, Watcher,
};
use chrono::{Duration, Utc};
use tempfile::NamedTempFile;

/// Create an in-memory-like DB backed by a temp file (`SQLite` needs a real file for WAL).
fn test_db() -> Database {
    let tmp = NamedTempFile::new().expect("create temp file");
    let path = tmp.path().to_path_buf();
    // Keep the temp file alive by leaking it (tests are short-lived).
    std::mem::forget(tmp);
    Database::new(&path).expect("create test db")
}

fn sample_task(id: &str) -> BackgroundAgent {
    BackgroundAgent {
        id: id.to_string(),
        prompt: "Run tests".to_string(),
        schedule_expr: "0 9 * * *".to_string(),
        cli: Cli::new("opencode"),
        model: None,
        working_dir: Some("/tmp/project".to_string()),
        enabled: true,
        created_at: Utc::now(),
        expires_at: None,
        last_run_at: None,
        last_run_ok: None,
        log_path: "/tmp/test.log".to_string(),
        timeout_minutes: 15,
    }
}

fn sample_watcher(id: &str) -> Watcher {
    Watcher {
        id: id.to_string(),
        path: "/tmp/watched".to_string(),
        events: vec![WatchEvent::Create, WatchEvent::Modify],
        prompt: "Handle file change".to_string(),
        cli: Cli::new("kiro"),
        model: Some("claude-4".to_string()),
        debounce_seconds: 5,
        recursive: true,
        enabled: true,
        created_at: Utc::now(),
        last_triggered_at: None,
        trigger_count: 0,
        timeout_minutes: 15,
    }
}

// ── Session lifecycle ───────────────────────────────────────────

#[test]
fn test_terminal_session_finish_removes_from_active_list() {
    let db = test_db();
    db.insert_terminal_session("term-1", "shell-1", "bash", "/tmp")
        .unwrap();

    let active = db.get_active_terminal_sessions().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "term-1");

    db.finish_terminal_session("term-1").unwrap();
    assert!(db.get_active_terminal_sessions().unwrap().is_empty());
}

#[test]
fn test_mark_orphaned_terminal_sessions_clears_idle_records() {
    let db = test_db();
    db.insert_terminal_session("term-1", "shell-1", "bash", "/tmp")
        .unwrap();
    db.insert_terminal_session("term-2", "shell-2", "zsh", "/tmp")
        .unwrap();

    assert_eq!(db.get_active_terminal_sessions().unwrap().len(), 2);
    db.mark_orphaned_terminal_sessions().unwrap();
    assert!(db.get_active_terminal_sessions().unwrap().is_empty());
}

// ── BackgroundAgent CRUD ─────────────────────────────────────────────────

#[test]
fn test_insert_and_get_task() {
    let db = test_db();
    let background_agent = sample_task("build-daily");
    db.insert_or_update_background_agent(&background_agent)
        .unwrap();

    let retrieved = db
        .get_background_agent("build-daily")
        .unwrap()
        .expect("background_agent exists");
    assert_eq!(retrieved.id, "build-daily");
    assert_eq!(retrieved.prompt, "Run tests");
    assert_eq!(retrieved.schedule_expr, "0 9 * * *");
    assert_eq!(retrieved.cli.as_str(), "opencode");
    assert_eq!(retrieved.working_dir.as_deref(), Some("/tmp/project"));
    assert!(retrieved.enabled);
}

#[test]
fn test_get_nonexistent_task() {
    let db = test_db();
    let result = db.get_background_agent("does-not-exist").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_upsert_task_overwrites() {
    let db = test_db();
    let mut background_agent = sample_task("my-background_agent");
    db.insert_or_update_background_agent(&background_agent)
        .unwrap();

    background_agent.prompt = "Updated prompt".to_string();
    background_agent.schedule_expr = "*/10 * * * *".to_string();
    db.insert_or_update_background_agent(&background_agent)
        .unwrap();

    let retrieved = db
        .get_background_agent("my-background_agent")
        .unwrap()
        .unwrap();
    assert_eq!(retrieved.prompt, "Updated prompt");
    assert_eq!(retrieved.schedule_expr, "*/10 * * * *");
}

#[test]
fn test_list_tasks_ordered_by_created_at_desc() {
    let db = test_db();

    let mut t1 = sample_task("first");
    t1.created_at = Utc::now() - Duration::hours(2);
    let mut t2 = sample_task("second");
    t2.created_at = Utc::now() - Duration::hours(1);
    let mut t3 = sample_task("third");
    t3.created_at = Utc::now();

    db.insert_or_update_background_agent(&t1).unwrap();
    db.insert_or_update_background_agent(&t2).unwrap();
    db.insert_or_update_background_agent(&t3).unwrap();

    let background_agents = db.list_background_agents().unwrap();
    assert_eq!(background_agents.len(), 3);
    assert_eq!(background_agents[0].id, "third");
    assert_eq!(background_agents[1].id, "second");
    assert_eq!(background_agents[2].id, "first");
}

#[test]
fn test_delete_task() {
    let db = test_db();
    db.insert_or_update_background_agent(&sample_task("to-delete"))
        .unwrap();
    assert!(db.get_background_agent("to-delete").unwrap().is_some());

    db.delete_background_agent("to-delete").unwrap();
    assert!(db.get_background_agent("to-delete").unwrap().is_none());
}

#[test]
fn test_update_task_enabled() {
    let db = test_db();
    db.insert_or_update_background_agent(&sample_task("toggle-me"))
        .unwrap();

    db.update_background_agent_enabled("toggle-me", false)
        .unwrap();
    let background_agent = db.get_background_agent("toggle-me").unwrap().unwrap();
    assert!(!background_agent.enabled);

    db.update_background_agent_enabled("toggle-me", true)
        .unwrap();
    let background_agent = db.get_background_agent("toggle-me").unwrap().unwrap();
    assert!(background_agent.enabled);
}

#[test]
fn test_update_task_last_run() {
    let db = test_db();
    db.insert_or_update_background_agent(&sample_task("run-me"))
        .unwrap();

    db.update_background_agent_last_run("run-me", true).unwrap();
    let background_agent = db.get_background_agent("run-me").unwrap().unwrap();
    assert!(background_agent.last_run_at.is_some());
    assert_eq!(background_agent.last_run_ok, Some(true));

    db.update_background_agent_last_run("run-me", false)
        .unwrap();
    let background_agent = db.get_background_agent("run-me").unwrap().unwrap();
    assert_eq!(background_agent.last_run_ok, Some(false));
}

#[test]
fn test_task_with_expiration() {
    let db = test_db();
    let mut background_agent = sample_task("expiring");
    background_agent.expires_at = Some(Utc::now() + Duration::hours(1));
    db.insert_or_update_background_agent(&background_agent)
        .unwrap();

    let retrieved = db.get_background_agent("expiring").unwrap().unwrap();
    assert!(retrieved.expires_at.is_some());
    assert!(!retrieved.is_expired());
}

// ── Watcher CRUD ──────────────────────────────────────────────

#[test]
fn test_insert_and_get_watcher() {
    let db = test_db();
    let watcher = sample_watcher("watch-src");
    db.insert_or_update_watcher(&watcher).unwrap();

    let retrieved = db
        .get_watcher("watch-src")
        .unwrap()
        .expect("watcher exists");
    assert_eq!(retrieved.id, "watch-src");
    assert_eq!(retrieved.path, "/tmp/watched");
    assert_eq!(retrieved.events.len(), 2);
    assert!(retrieved.events.contains(&WatchEvent::Create));
    assert!(retrieved.events.contains(&WatchEvent::Modify));
    assert_eq!(retrieved.cli.as_str(), "kiro");
    assert_eq!(retrieved.model.as_deref(), Some("claude-4"));
    assert_eq!(retrieved.debounce_seconds, 5);
    assert!(retrieved.recursive);
}

#[test]
fn test_get_nonexistent_watcher() {
    let db = test_db();
    assert!(db.get_watcher("nope").unwrap().is_none());
}

#[test]
fn test_list_and_delete_watchers() {
    let db = test_db();
    db.insert_or_update_watcher(&sample_watcher("w1")).unwrap();
    db.insert_or_update_watcher(&sample_watcher("w2")).unwrap();

    assert_eq!(db.list_watchers().unwrap().len(), 2);

    db.delete_watcher("w1").unwrap();
    assert_eq!(db.list_watchers().unwrap().len(), 1);
    assert!(db.get_watcher("w1").unwrap().is_none());
}

#[test]
fn test_list_enabled_watchers() {
    let db = test_db();
    let mut w1 = sample_watcher("enabled-w");
    w1.enabled = true;
    let mut w2 = sample_watcher("disabled-w");
    w2.enabled = false;

    db.insert_or_update_watcher(&w1).unwrap();
    db.insert_or_update_watcher(&w2).unwrap();

    let enabled = db.list_enabled_watchers().unwrap();
    assert_eq!(enabled.len(), 1);
    assert_eq!(enabled[0].id, "enabled-w");
}

#[test]
fn test_update_watcher_enabled() {
    let db = test_db();
    db.insert_or_update_watcher(&sample_watcher("toggle-w"))
        .unwrap();

    db.update_watcher_enabled("toggle-w", false).unwrap();
    let w = db.get_watcher("toggle-w").unwrap().unwrap();
    assert!(!w.enabled);
}

#[test]
fn test_update_watcher_triggered() {
    let db = test_db();
    db.insert_or_update_watcher(&sample_watcher("trig-w"))
        .unwrap();

    db.update_watcher_triggered("trig-w").unwrap();
    let w = db.get_watcher("trig-w").unwrap().unwrap();
    assert!(w.last_triggered_at.is_some());
    assert_eq!(w.trigger_count, 1);

    db.update_watcher_triggered("trig-w").unwrap();
    let w = db.get_watcher("trig-w").unwrap().unwrap();
    assert_eq!(w.trigger_count, 2);
}

// ── Run log operations ────────────────────────────────────────

#[test]
fn test_insert_and_list_runs() {
    let db = test_db();
    // Need a background_agent first for FK
    db.insert_or_update_background_agent(&sample_task("run-background_agent"))
        .unwrap();

    let run = RunLog {
        id: uuid::Uuid::new_v4().to_string(),
        background_agent_id: "run-background_agent".to_string(),
        status: RunStatus::Success,
        trigger_type: TriggerType::Scheduled,
        summary: None,
        started_at: Utc::now() - Duration::minutes(5),
        finished_at: Some(Utc::now()),
        exit_code: Some(0),
        timeout_at: None,
    };
    db.insert_run(&run).unwrap();

    let runs = db.list_runs("run-background_agent", 10).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].background_agent_id, "run-background_agent");
    assert_eq!(runs[0].exit_code, Some(0));
    assert!(matches!(runs[0].trigger_type, TriggerType::Scheduled));
}

#[test]
fn test_list_runs_limit() {
    let db = test_db();
    db.insert_or_update_background_agent(&sample_task("many-runs"))
        .unwrap();

    for i in 0..10 {
        let run = RunLog {
            id: uuid::Uuid::new_v4().to_string(),
            background_agent_id: "many-runs".to_string(),
            status: RunStatus::Success,
            trigger_type: TriggerType::Manual,
            summary: None,
            started_at: Utc::now() - Duration::minutes(i),
            finished_at: Some(Utc::now()),
            exit_code: Some(0),
            timeout_at: None,
        };
        db.insert_run(&run).unwrap();
    }

    let runs = db.list_runs("many-runs", 3).unwrap();
    assert_eq!(runs.len(), 3);
}

#[test]
fn test_delete_task_cascades_runs() {
    let db = test_db();
    db.insert_or_update_background_agent(&sample_task("cascade-background_agent"))
        .unwrap();
    let run = RunLog {
        id: uuid::Uuid::new_v4().to_string(),
        background_agent_id: "cascade-background_agent".to_string(),
        status: RunStatus::Pending,
        trigger_type: TriggerType::Watch,
        summary: None,
        started_at: Utc::now(),
        finished_at: None,
        exit_code: None,
        timeout_at: None,
    };
    db.insert_run(&run).unwrap();
    assert_eq!(
        db.list_runs("cascade-background_agent", 10).unwrap().len(),
        1
    );

    db.delete_background_agent("cascade-background_agent")
        .unwrap();
    assert_eq!(
        db.list_runs("cascade-background_agent", 10).unwrap().len(),
        0
    );
}

// ── BackgroundAgent field updates ────────────────────────────────────────

#[test]
fn test_update_task_fields_prompt() {
    let db = test_db();
    db.insert_or_update_background_agent(&sample_task("upd-background_agent"))
        .unwrap();

    let fields = BackgroundAgentFieldsUpdate {
        prompt: Some("New prompt"),
        ..Default::default()
    };
    assert!(db
        .update_background_agent_fields("upd-background_agent", &fields)
        .unwrap());

    let t = db
        .get_background_agent("upd-background_agent")
        .unwrap()
        .unwrap();
    assert_eq!(t.prompt, "New prompt");
    assert_eq!(t.schedule_expr, "0 9 * * *"); // unchanged
}

#[test]
fn test_update_task_fields_multiple() {
    let db = test_db();
    db.insert_or_update_background_agent(&sample_task("upd-multi"))
        .unwrap();

    let fields = BackgroundAgentFieldsUpdate {
        prompt: Some("Updated prompt"),
        schedule_expr: Some("*/10 * * * *"),
        cli: Some("kiro"),
        model: Some(Some("gpt-5")),
        ..Default::default()
    };
    assert!(db
        .update_background_agent_fields("upd-multi", &fields)
        .unwrap());

    let t = db.get_background_agent("upd-multi").unwrap().unwrap();
    assert_eq!(t.prompt, "Updated prompt");
    assert_eq!(t.schedule_expr, "*/10 * * * *");
    assert_eq!(t.cli.as_str(), "kiro");
    assert_eq!(t.model.as_deref(), Some("gpt-5"));
}

#[test]
fn test_update_task_fields_clear_optional() {
    let db = test_db();
    let mut background_agent = sample_task("upd-clear");
    background_agent.model = Some("claude-4".to_string());
    db.insert_or_update_background_agent(&background_agent)
        .unwrap();

    let fields = BackgroundAgentFieldsUpdate {
        model: Some(None), // clear model
        ..Default::default()
    };
    assert!(db
        .update_background_agent_fields("upd-clear", &fields)
        .unwrap());

    let t = db.get_background_agent("upd-clear").unwrap().unwrap();
    assert!(t.model.is_none());
}

#[test]
fn test_update_task_fields_no_fields_returns_false() {
    let db = test_db();
    db.insert_or_update_background_agent(&sample_task("upd-noop"))
        .unwrap();

    let fields = BackgroundAgentFieldsUpdate::default();
    assert!(!db
        .update_background_agent_fields("upd-noop", &fields)
        .unwrap());
}

#[test]
fn test_update_task_fields_nonexistent_returns_false() {
    let db = test_db();

    let fields = BackgroundAgentFieldsUpdate {
        prompt: Some("ghost"),
        ..Default::default()
    };
    assert!(!db
        .update_background_agent_fields("nonexistent", &fields)
        .unwrap());
}

// ── Watcher field updates ─────────────────────────────────────

#[test]
fn test_update_watcher_fields_prompt() {
    let db = test_db();
    db.insert_or_update_watcher(&sample_watcher("upd-watch"))
        .unwrap();

    let fields = WatcherFieldsUpdate {
        prompt: Some("New watcher prompt"),
        ..Default::default()
    };
    assert!(db.update_watcher_fields("upd-watch", &fields).unwrap());

    let w = db.get_watcher("upd-watch").unwrap().unwrap();
    assert_eq!(w.prompt, "New watcher prompt");
    assert_eq!(w.path, "/tmp/watched"); // unchanged
}

#[test]
fn test_update_watcher_fields_multiple() {
    let db = test_db();
    db.insert_or_update_watcher(&sample_watcher("upd-wmulti"))
        .unwrap();

    let events_json = serde_json::to_string(&vec![WatchEvent::Delete]).unwrap();
    let fields = WatcherFieldsUpdate {
        path: Some("/new/path"),
        events: Some(&events_json),
        debounce_seconds: Some(10),
        recursive: Some(false),
        ..Default::default()
    };
    assert!(db.update_watcher_fields("upd-wmulti", &fields).unwrap());

    let w = db.get_watcher("upd-wmulti").unwrap().unwrap();
    assert_eq!(w.path, "/new/path");
    assert_eq!(w.events, vec![WatchEvent::Delete]);
    assert_eq!(w.debounce_seconds, 10);
    assert!(!w.recursive);
}

#[test]
fn test_update_watcher_fields_clear_model() {
    let db = test_db();
    db.insert_or_update_watcher(&sample_watcher("upd-wclr"))
        .unwrap();
    // sample_watcher has model = Some("claude-4")

    let fields = WatcherFieldsUpdate {
        model: Some(None),
        ..Default::default()
    };
    assert!(db.update_watcher_fields("upd-wclr", &fields).unwrap());

    let w = db.get_watcher("upd-wclr").unwrap().unwrap();
    assert!(w.model.is_none());
}

#[test]
fn test_update_watcher_fields_no_fields_returns_false() {
    let db = test_db();
    db.insert_or_update_watcher(&sample_watcher("upd-wnoop"))
        .unwrap();

    let fields = WatcherFieldsUpdate::default();
    assert!(!db.update_watcher_fields("upd-wnoop", &fields).unwrap());
}

// ── Daemon state ──────────────────────────────────────────────

#[test]
fn test_set_and_get_state() {
    let db = test_db();
    db.set_state("port", "7755").unwrap();
    assert_eq!(db.get_state("port").unwrap(), Some("7755".to_string()));
}

#[test]
fn test_get_state_missing_key() {
    let db = test_db();
    assert!(db.get_state("missing").unwrap().is_none());
}

#[test]
fn test_set_state_overwrites() {
    let db = test_db();
    db.set_state("version", "0.1.0").unwrap();
    db.set_state("version", "0.2.0").unwrap();
    assert_eq!(db.get_state("version").unwrap(), Some("0.2.0".to_string()));
}
