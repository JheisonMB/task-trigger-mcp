# task-trigger-mcp

A self-contained MCP server that lets AI agents register, manage, and execute **scheduled** and **event-driven** tasks. Single static binary. No runtime dependencies. Cross-platform (Linux/WSL, macOS).

Your agent says *"run tests every day at 9am"* — the model converts that to a cron expression, and the binary handles scheduling, file watching, CLI invocation, log rotation, and everything else internally. The agent never writes bash scripts or touches crontab.

---

## How It Works

```mermaid
graph TB
    subgraph daemon["task-trigger-mcp daemon"]
        MCP["MCP Server<br/>SSE/HTTP :7755"]
        SCHED["Cron Scheduler<br/>(internal, tokio)"]
        WE["Watcher Engine<br/>(notify crate)"]
        DB[(SQLite<br/>tasks.db)]
        MCP <--> DB
        SCHED <--> DB
        WE <--> DB
        SCHED -- "on schedule" --> EXEC
        WE -- "on file event" --> EXEC
        EXEC["Executor"]
    end

    Agent["MCP Client<br/>(OpenCode, Kiro,<br/>Claude Desktop)"] -- "SSE / stdio" --> MCP
    EXEC --> CLI["Headless CLI<br/>(opencode run / kiro-cli)"]

    style daemon fill:#1a1a2e,stroke:#16213e,color:#eee
    style Agent fill:#0f3460,stroke:#16213e,color:#eee
    style CLI fill:#e94560,stroke:#16213e,color:#eee
```

**Key property**: the agent connects and disconnects freely. Watchers keep running. Scheduled tasks keep firing. The daemon is the source of truth.

---

## Installation

### From source

```bash
git clone https://github.com/JheisonMB/task-trigger-mcp.git
cd task-trigger-mcp
cargo build --release
# Binary at target/release/task-trigger-mcp
```

### Via cargo

```bash
cargo install task-trigger-mcp
```

Available on [crates.io](https://crates.io/crates/task-trigger-mcp).

### GitHub Releases

Check the [Releases](https://github.com/JheisonMB/task-trigger-mcp/releases) page for precompiled binaries and prerelease builds.

---

## MCP Client Configuration

Add this to your OpenCode config file (`~/.opencode/config.json`):

```json
{
  "mcp": {
    "task-trigger": {
      "type": "local",
      "command": ["task-trigger-mcp"],
      "args": ["stdio"],
      "enabled": true
    }
  }
}
```

**Note:** This runs task-trigger-mcp in stdio mode. Scheduled tasks will pause when OpenCode disconnects. For persistent task execution, run the daemon separately:

```bash
task-trigger-mcp daemon start
```

And reconfigure to use remote MCP:

```json
{
  "mcp": {
    "task-trigger": {
      "type": "remote",
      "url": "http://localhost:7755/sse",
      "enabled": true
    }
  }
}
```

---

## Quick Start

```bash
# 1. Start the daemon
task-trigger-mcp daemon start

# 2. Check it's running
task-trigger-mcp daemon status

# 3. Your agent now has access to 10 task management tools
```

The daemon is a single long-running process that owns:

1. **MCP Server** (SSE/HTTP on port 7755) — so agents can connect and call tools
2. **Internal Cron Scheduler** (tokio) — checks every 30 seconds which tasks are due and executes them
3. **File Watcher Engine** (notify crate) — monitors files/directories for changes and triggers executions
4. **SQLite Database** — persists all task/watcher definitions, run history, and logs

There is no dependency on `crontab`, `launchd`, or any OS scheduler. Everything runs inside the daemon.

### What happens when the daemon stops?

| Component | Behavior |
|---|---|
| **Scheduled tasks** | Stop executing. They resume when the daemon restarts. |
| **File watchers** | Stop monitoring. They are reloaded from SQLite on restart. |
| **Task definitions** | Persist in SQLite. Nothing is lost. |

### How to make it survive reboots

Run `task-trigger-mcp daemon start` in your shell startup file (`.bashrc`, `.zshrc`), or set up a systemd/launchd service manually. A built-in `daemon install-service` command is planned for a future release (see Roadmap).

### Why not use crontab?

- Zero external dependencies — the binary is fully self-contained
- No permission issues with crontab editing
- No state synchronization problems (SQLite is the single source of truth)
- Works identically on Linux, WSL, and macOS
- Simpler architecture — one process, one database, one scheduler

---

## MCP Tools

The server exposes 10 tools to the agent:

| Tool | Description |
|---|---|
| `task_add` | Register a scheduled task with a 5-field cron expression (`*/5 * * * *`, `0 9 * * 1-5`). The model converts natural language to cron. |
| `task_watch` | Watch a file/directory for create, modify, delete, or move events |
| `task_list` | List all scheduled tasks with status, last run, and expiry info |
| `task_watchers` | List all file watchers with status and trigger counts |
| `task_remove` | Remove a task or watcher completely |
| `task_unwatch` | Pause a file watcher without deleting it |
| `task_enable` | Re-enable a disabled task or watcher |
| `task_disable` | Disable a task or watcher without removing it |
| `task_run` | Execute a task immediately, outside its schedule |
| `task_logs` | Get log output for a task or watcher with optional line/time filters |
| `task_status` | Daemon health: uptime, transport, scheduler status, active counts |

### Schedule format (cron)

The `schedule` field in `task_add` expects a standard 5-field cron expression:

```
┌───────── minute (0-59)
│ ┌─────── hour (0-23)
│ │ ┌───── day of month (1-31)
│ │ │ ┌─── month (1-12)
│ │ │ │ ┌─ day of week (0-6, 0=Sun)
│ │ │ │ │
* * * * *
```

Common patterns:
- `*/5 * * * *` — every 5 minutes
- `0 9 * * *` — daily at 9am
- `0 9 * * 1-5` — weekdays at 9am
- `0 */2 * * *` — every 2 hours
- `30 14 1,15 * *` — 1st and 15th at 2:30pm

The model is responsible for converting natural language (e.g. "every day at 9am") into cron expressions. The tool description includes common patterns to guide the model.

---

## Usage Examples

### Schedule a daily test run

> Agent: "Run the test suite every day at 9am"

The model calls `task_add`:
```json
{
  "id": "daily-tests",
  "prompt": "Run cargo test in the project and report any failures",
  "schedule": "0 9 * * *",
  "cli": "opencode",
  "working_dir": "/home/user/my-project"
}
```

### Watch for source changes

> Agent: "Watch src/ for changes and run the linter"

The model calls `task_watch`:
```json
{
  "id": "lint-on-change",
  "path": "/home/user/my-project/src",
  "events": ["create", "modify"],
  "prompt": "Run cargo clippy and fix any warnings",
  "cli": "opencode",
  "recursive": true,
  "debounce_seconds": 5
}
```

### Temporary task with auto-expiry

> Agent: "Check deployment status every minute for the next hour"

```json
{
  "id": "monitor-deploy",
  "prompt": "Check deployment status and report",
  "schedule": "*/1 * * * *",
  "cli": "opencode",
  "duration_minutes": 60
}
```

This task auto-disables after 60 minutes.

### Prompt variables

Prompts support variable substitution at execution time:

- `{{TIMESTAMP}}` — current ISO 8601 timestamp
- `{{TASK_ID}}` — the task's ID
- `{{LOG_PATH}}` — path to the task's log file
- `{{FILE_PATH}}` — the watched file path (watchers only)
- `{{EVENT_TYPE}}` — the event that fired (watchers only)

---

## Daemon Management

```bash
task-trigger-mcp daemon start     # start in background
task-trigger-mcp daemon stop      # stop daemon
task-trigger-mcp daemon status    # check if running
task-trigger-mcp daemon restart   # restart
task-trigger-mcp daemon logs      # tail daemon logs
```

---

## Runtime Directory

```
~/.task-trigger/
  tasks.db              # SQLite database
  daemon.pid            # PID file for daemon management
  daemon.log            # daemon-level logs
  logs/
    <task-id>.log       # per-task/watcher logs (5MB rotation)
```

---

## Platform Support

| Feature | Linux / WSL | macOS |
|---|---|---|
| Daemon transport | SSE/HTTP localhost | SSE/HTTP localhost |
| Cron scheduling | Internal (tokio) | Internal (tokio) |
| File watching | inotify | FSEvents |
| Service install | systemd unit | launchd agent |
| Binary format | ELF static (musl) | Mach-O |

---

## Tech Stack

| Concern | Crate |
|---|---|
| MCP SDK | `rmcp` + `rmcp-macros` |
| Async runtime | `tokio` |
| HTTP transport | `axum` (via rmcp) |
| Cron parsing | `cron` |
| File watching | `notify` |
| State | `rusqlite` (bundled) |
| Serialization | `serde` + `serde_json` |
| CLI detection | `which` |
| Logging | `tracing` |

---

## Roadmap

- `daemon install-service` — install as systemd unit (Linux/WSL) or launchd agent (macOS) for reboot persistence
- Webhook trigger support (HTTP endpoint that fires a task)
- Claude Code CLI support
- Optional auth token for SSE endpoint

---

## License

MIT
