# task-trigger-mcp — Specification v2.0

## Overview

A self-contained MCP server written in Rust that enables AI agents to register, manage, and execute scheduled and event-driven tasks. Replaces the `task-trigger` skill entirely — the agent interacts only through MCP tools, with zero bash scripting or platform-specific logic on the agent side.

Single static binary. No runtime dependencies. Cross-platform (Linux/WSL, macOS).

---

## Design Principles

- **Agent-agnostic**: works with any MCP-compatible client (OpenCode, Kiro, Claude Code, etc.)
- **Model-agnostic**: all logic lives in the binary, not in the agent's reasoning — dumb models can use it correctly
- **Zero external dependencies at runtime**: no inotifywait, no cron daemon awareness, no shell scripts
- **Persistent daemon**: runs as a long-lived background process; watchers and schedulers survive agent disconnection
- **Persistent state**: SQLite for tasks, watchers, logs, and expiry tracking
- **Idempotent operations**: calling `task_add` twice with the same ID updates, not duplicates
- **Token-efficient**: file watching is event-driven via native OS APIs — zero polling, zero idle token consumption

---

## Binary Modes

The binary operates in three modes:

**Daemon mode** (`task-trigger-mcp daemon start`): starts the MCP server as a persistent background process over SSE/HTTP transport on localhost. Watchers and schedulers run continuously regardless of whether any agent is connected. This is the primary mode.

**Executor mode** (internal): invoked by the OS scheduler (crontab/launchd) with a task ID. Checks expiry, spawns the CLI headlessly, rotates logs, and exits. The OS scheduler always calls this mode — never the CLI directly.

**Stdio mode** (`task-trigger-mcp stdio`): legacy/fallback mode for clients that only support stdio transport. Watchers registered in this mode stop when the process exits.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│              task-trigger-mcp daemon             │
│                                                  │
│  ┌─────────────┐    ┌──────────────────────────┐ │
│  │ MCP Server  │    │   Watcher Engine         │ │
│  │ SSE/HTTP    │    │   (notify crate threads) │ │
│  │ localhost   │    │   event-driven, always   │ │
│  │ :PORT       │    │   running                │ │
│  └──────┬──────┘    └──────────┬───────────────┘ │
│         │                      │                  │
│         └──────────┬───────────┘                  │
│                    │                              │
│             ┌──────▼──────┐                       │
│             │   SQLite    │                       │
│             │  tasks.db   │                       │
│             └─────────────┘                       │
└─────────────────────────────────────────────────┘
         ▲                    ▲
         │ SSE/HTTP           │ crontab/launchd
    OpenCode / Kiro      OS Scheduler
    (connects when          (calls executor
     needed, leaves)         mode on schedule)
```

**Key property**: the agent connects and disconnects freely. Watchers keep running. Scheduled tasks keep firing. The daemon is the source of truth.

---

## Installation

```bash
# Via cargo
cargo install task-trigger-mcp

# Start daemon (one-time setup)
task-trigger-mcp daemon start

# Optional: install as systemd service (Linux/WSL)
task-trigger-mcp daemon install-service
```

Distributed as precompiled static binaries via GitHub Releases. Targets: `x86_64-unknown-linux-musl`, `aarch64-apple-darwin`, `x86_64-apple-darwin`.

### Agent Configuration

In OpenCode/Kiro config, point to the running daemon via SSE:

```json
{
  "mcpServers": {
    "task-trigger": {
      "transport": "sse",
      "url": "http://localhost:7755/sse"
    }
  }
}
```

For stdio fallback:

```json
{
  "mcpServers": {
    "task-trigger": {
      "command": "task-trigger-mcp",
      "args": ["stdio"]
    }
  }
}
```

---

## Daemon Management CLI

```bash
task-trigger-mcp daemon start            # start daemon in background
task-trigger-mcp daemon stop             # stop daemon
task-trigger-mcp daemon status           # check if running, port, version
task-trigger-mcp daemon restart          # restart
task-trigger-mcp daemon install-service  # install as systemd unit (Linux/WSL)
task-trigger-mcp daemon logs             # tail daemon logs
```

Default port: `7755`. Configurable via `--port` flag or `TASK_TRIGGER_PORT` env var.

---

## MCP Tool API

### `task_add`

Registers a new scheduled task. The agent passes structured parameters — the binary handles cron registration, plist generation, platform detection, and directory setup internally.

**Parameters:**

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Unique identifier. Lowercase, hyphens. If exists, updates. |
| `prompt` | string | yes | The instruction the CLI will execute headlessly |
| `schedule` | string | yes | Cron expression OR natural language (see Schedule Format) |
| `cli` | enum | yes | `opencode` or `kiro` |
| `model` | string | no | Provider/model string. Omit to use CLI default |
| `duration_minutes` | integer | no | Auto-expire after N minutes from registration |
| `working_dir` | string | no | Working directory for the CLI. Defaults to `$HOME` |

**Schedule format:** Accepts standard 5-field cron expressions or natural language phrases (`"every day at 9am"`, `"every 5 minutes"`, `"cada hora"`). The binary resolves natural language to cron internally.

**Behavior:** Detects platform at runtime, writes the appropriate scheduler entry (crontab line or launchd plist), creates log directory, persists task to SQLite. Always routes through the executor mode binary.

---

### `task_watch`

Registers a file or directory watcher. When the watched path triggers an event, the daemon spawns the CLI headlessly with the given prompt. The watcher is managed entirely by the daemon — it survives agent disconnection.

**Parameters:**

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Unique identifier |
| `path` | string | yes | Absolute path to file or directory |
| `events` | array | yes | Any of: `create`, `modify`, `delete`, `move` |
| `prompt` | string | yes | Instruction for the CLI on trigger |
| `cli` | enum | yes | `opencode` or `kiro` |
| `model` | string | no | Provider/model string |
| `debounce_seconds` | integer | no | Debounce window. Default: 2 |
| `recursive` | boolean | no | Watch subdirectories. Default: false |

**Behavior:** Uses native OS APIs (inotify on Linux, FSEvents on macOS) via the `notify` crate. The watcher thread is owned by the daemon process, not by the MCP session. Registering a watcher is persistent — it survives daemon restarts (reloaded from SQLite on startup). Zero token consumption while idle.

---

### `task_list`

Returns all registered scheduled tasks with their current status.

**Returns per task:** id, prompt (truncated), schedule expression, next scheduled run, last run timestamp, last run result (success/failure), enabled status, expiry info if temporal.

---

### `task_watchers`

Returns all active file watchers with their current status.

**Returns per watcher:** id, watched path, events, CLI, status (active/stopped), last triggered timestamp, trigger count.

---

### `task_remove`

Removes a task completely: unregisters from the OS scheduler, stops any associated watcher, deletes from SQLite.

**Parameters:** `id` (string)

---

### `task_unwatch`

Pauses a file watcher without deleting its definition from SQLite. Can be resumed later with `task_watch` using the same ID.

**Parameters:** `id` (string)

---

### `task_enable` / `task_disable`

Enable or disable a scheduled task without removing it. Useful for temporarily pausing a task.

**Parameters:** `id` (string)

---

### `task_run`

Executes a task immediately, outside its schedule. Useful for testing or one-off execution.

**Parameters:** `id` (string)

**Behavior:** Spawns the CLI headlessly with the task's prompt and model. Output appended to the task's log file with a `[manual]` trigger marker.

---

### `task_logs`

Returns the log output for a task or watcher.

**Parameters:**

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Task or watcher ID |
| `lines` | integer | no | Last N lines. Default: 50 |
| `since` | string | no | ISO 8601 timestamp filter |

---

### `task_status`

Returns overall health of the daemon and scheduler state.

**Returns:** daemon version, transport (SSE/stdio), port, uptime, scheduler availability (crontab/launchd), number of active tasks, number of active watchers, temporal tasks with time remaining, log directory health.

---

## State Schema (SQLite)

Four tables:

**tasks**: `id`, `prompt`, `schedule_expr`, `cli_path`, `model`, `working_dir`, `enabled`, `created_at`, `expires_at`, `last_run_at`, `last_run_ok`, `log_path`

**watchers**: `id`, `path`, `events` (JSON array), `cli_path`, `model`, `prompt`, `debounce_seconds`, `recursive`, `enabled`, `created_at`, `last_triggered_at`, `trigger_count`

**runs**: `id` (autoincrement), `task_id`, `started_at`, `finished_at`, `exit_code`, `trigger_type` (`scheduled` | `manual` | `watch`)

**daemon_state**: `key`, `value` — for daemon-level metadata (port, version, last_start)

On daemon startup, all enabled watchers are reloaded from SQLite and re-registered with the OS watcher engine automatically.

---

## Expiry and Auto-cleanup

When `duration_minutes` is provided, `expires_at` is computed at registration time and stored in SQLite.

In executor mode, before spawning the CLI, the binary checks `expires_at`. If expired: disables the task, removes the scheduler entry, and exits without executing.

`task_status` reports time remaining for all temporal tasks.

---

## CLI Invocation

The binary knows how to invoke each supported CLI headlessly:

**OpenCode:** `opencode run --prompt "<prompt>"` with optional `-m "<model>"`

**Kiro:** `kiro-cli chat --no-interactive --trust-all-tools "<prompt>"` with optional `--model "<model>"`

The binary always uses the full resolved path to the CLI binary (via the `which` crate) to avoid PATH issues in crontab and launchd environments.

---

## Prompt Variable Substitution

At execution time, the binary expands variables in the prompt string before passing it to the CLI:

- `{{TIMESTAMP}}` → current ISO 8601 timestamp
- `{{FILE_PATH}}` → the watched path (file watchers only)
- `{{EVENT_TYPE}}` → the event that triggered (file watchers only: `create`, `modify`, `delete`, `move`)
- `{{TASK_ID}}` → the task's ID
- `{{LOG_PATH}}` → the task's log file path

---

## Logging

Each task/watcher has a dedicated log file at `$HOME/.task-trigger/logs/<id>.log`. Log entries include timestamp, trigger type, exit code, and stdout/stderr from the CLI. Logs are append-only with automatic rotation at 5MB.

Daemon-level logs at `$HOME/.task-trigger/daemon.log`.

---

## Platform Support

| Feature | Linux / WSL | macOS |
|---|---|---|
| Daemon transport | SSE HTTP localhost | SSE HTTP localhost |
| Cron scheduling | crontab | launchd plist |
| File watching | inotify (native) | FSEvents (native) |
| Service install | systemd unit | launchd agent |
| State | SQLite | SQLite |
| Binary format | ELF static (musl) | Mach-O |

Platform detected at runtime via `std::env::consts::OS`. No compile-time conditionals exposed to the agent.

---

## Non-Goals (v1)

- No Windows Task Scheduler support
- No web UI or dashboard
- No MCP sampling dependency
- No git operations
- No distributed or multi-machine scheduling
- No built-in prompt templating beyond variable substitution
- No authentication on the local SSE endpoint (localhost only, trusted environment assumed)

---

## Implementation Stack

| Concern | Crate |
|---|---|
| MCP SDK | `rmcp` + `rmcp-macros` |
| Async runtime | `tokio` |
| SSE/HTTP transport | `axum` (via rmcp http feature) |
| File watching | `notify` |
| Cron parsing | `cron` |
| State | `rusqlite` |
| Serialization | `serde` + `serde_json` |
| Schema for tools | `schemars` |
| CLI detection | `which` |
| Logging | `tracing` + `tracing-subscriber` |
| Natural language schedule | internal module |

---

## Directory Layout (runtime)

```
$HOME/.task-trigger/
  tasks.db              ← SQLite database
  daemon.pid            ← PID file for daemon management
  daemon.log            ← daemon-level logs
  logs/
    <task-id>.log       ← per-task/watcher logs
  launchd/              ← macOS plist files (symlinked to ~/Library/LaunchAgents)
```

---

## Startup Sequence (daemon)

1. Check if daemon already running via `daemon.pid`
2. Bind SSE/HTTP server on configured port
3. Write PID file
4. Load all enabled tasks from SQLite → re-register crontab/launchd entries
5. Load all enabled watchers from SQLite → re-register with `notify` engine
6. Begin serving MCP tool calls
7. On shutdown: stop watchers, flush logs, remove PID file

---

## Roadmap (post-v1)

- `task_rewatch`: resume a paused watcher
- Webhook trigger support (HTTP endpoint that fires a task)
- Claude Code CLI support
- Optional auth token for SSE endpoint
