# Task-Trigger MCP - MVP Status Report

## Current Status: ✅ MINIMALLY VIABLE PRODUCT COMPLETE

The task-trigger-mcp daemon is now a **fully functional HTTP server** that can register, manage, and persist tasks. It has reached MVP with core functionality working end-to-end.

## What Works

### ✅ HTTP Server
- **REST API** on http://127.0.0.1:7755 (configurable via `TASK_TRIGGER_PORT` env var)
- **Health endpoint**: `/health` - returns version and status
- **Tool listing**: `/mcp/tools` (GET) - shows all 10 available tools
- **Tool execution**: `/mcp/call` (POST) - executes any registered tool with parameters

### ✅ Tool Handlers (10/10 Implemented)
1. `task_add` - Register a new scheduled task
2. `task_watch` - Create a file/directory watcher
3. `task_list` - List all registered tasks
4. `task_watchers` - List all active watchers
5. `task_remove` - Delete a task
6. `task_unwatch` - Stop watching a path
7. `task_enable` - Enable a disabled task
8. `task_disable` - Disable an enabled task
9. `task_status` - Get daemon status (active tasks, watchers, uptime)
10. `task_logs` - Retrieve task execution logs

### ✅ Database Persistence
- SQLite backend with proper schema (4 tables: tasks, watchers, runs, daemon_state)
- **Tested persistence**: Tasks survive server restarts
- Data stored in `~/.task-trigger/tasks.db`
- Logs stored in `~/.task-trigger/logs/`

### ✅ Features
- **Natural language schedules**: "every 5 minutes" → "*/5 * * * *"
- **Cron expression validation**: Accepts valid cron expressions
- **Task metadata**: Stores ID, prompt, schedule, CLI (kiro/opencode), model, working dir, enabled status
- **Timestamps**: created_at, expires_at, last_run_at
- **Enable/disable state**: Tasks can be toggled without deletion

## What Needs Implementation (Post-MVP)

### 🚧 File Watching
- `notify` crate setup in `src/watchers/mod.rs`
- Event detection (Create, Modify, Delete, Move)
- Debounce mechanism for rapid changes

### 🚧 Task Execution
- Subprocess execution in `src/executor/mod.rs`
- CLI integration (OpenCode or Kiro) as subprocess
- Log capture and persistence

### 🚧 Scheduler Integration
- OS-level scheduler (crontab on Linux, launchd on macOS)
- Task triggering at specified intervals
- Event-based triggering for watchers

### 🚧 Testing & Polish
- Unit tests for handlers and database
- Integration tests for HTTP endpoints
- Error recovery and resilience
- Performance optimization

## Quick Start

### Run the server
```bash
cd /mnt/c/Users/PC/Documents/PersonalProjects/mcp/task-trigger-mcp
RUST_LOG=info cargo run
```

### Test endpoints with curl

**Health check:**
```bash
curl http://localhost:7755/health
```

**List available tools:**
```bash
curl http://localhost:7755/mcp/tools
```

**Add a task:**
```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{
    "tool": "task_add",
    "params": {
      "id": "my-task",
      "prompt": "echo hello",
      "schedule": "every 5 minutes",
      "cli": "opencode"
    }
  }'
```

**List tasks:**
```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{"tool": "task_list", "params": {}}'
```

**Get status:**
```bash
curl -X POST http://localhost:7755/mcp/call \
  -H "Content-Type: application/json" \
  -d '{"tool": "task_status", "params": {}}'
```

## Compilation Status
- ✅ Compiles without errors
- ⚠️ 25 warnings (unused code for future features) - can be silenced later
- Build output: `target/debug/task-trigger-mcp`

## Architecture Summary

```
src/
├── main.rs           - Axum HTTP server + route handlers
├── daemon/mod.rs     - SimpleHandler with 10 tool implementations
├── db/mod.rs         - SQLite CRUD operations
├── state/mod.rs      - Data models (Task, Watcher, etc)
├── scheduler/mod.rs  - Cron conversion & variable substitution
├── tools/mod.rs      - Request/response types
├── executor/mod.rs   - [TODO] Task execution via subprocess
└── watchers/mod.rs   - [TODO] File watching with notify crate
```

## Next Priority Actions

1. **Implement file watcher engine** - Watch directories, emit events on changes
2. **Implement task executor** - Run CLI commands as subprocess, capture output
3. **Integrate OS scheduler** - Hook into crontab or OS event system
4. **Add persistence for runs** - Track execution history in database
5. **Build simple CLI client** - For easier testing than raw curl

---

**Project**: task-trigger-mcp  
**MVP Completion**: 2026-03-21  
**Status**: Ready for feature implementation
