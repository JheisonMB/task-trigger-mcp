# task-trigger-mcp — Specification v2.1

## Overview

A self-contained MCP server written in Rust that enables AI agents to register, manage, and execute scheduled and event-driven tasks. Replaces the `task-trigger` skill entirely — the agent interacts only through MCP tools, with zero bash scripting or platform-specific logic on the agent side.

Single static binary. No runtime dependencies. Cross-platform (Linux/WSL, macOS).

---

## Design Principles

- **Agent-agnostic**: works with any MCP-compatible client (OpenCode, Kiro, Claude Code, etc.)
- **Model-agnostic**: all logic lives in the binary, not in the agent's reasoning — dumb models can use it correctly
- **Zero external dependencies at runtime**: no inotifywait, no crontab, no launchd, no shell scripts
- **Self-contained scheduling**: internal tokio-based cron scheduler — no OS scheduler dependency
- **Persistent daemon**: runs as a long-lived background process; watchers and schedulers survive agent disconnection
- **Persistent state**: SQLite for tasks, watchers, logs, and expiry tracking
- **Idempotent operations**: calling `task_add` twice with the same ID updates, not duplicates
- **Token-efficient**: file watching is event-driven via native OS APIs — zero polling, zero idle token consumption

---

## Binary Modes

The binary operates in two modes:

**Daemon mode** (`task-trigger-mcp daemon start`): starts the MCP server as a persistent background process over SSE/HTTP transport on localhost. The internal cron scheduler and file watchers run continuously regardless of whether any agent is connected. Running with no subcommand starts the SSE server in the foreground.

**Stdio mode** (`task-trigger-mcp stdio`): legacy/fallback mode for clients that only support stdio transport. The internal scheduler and watchers also run, but stop when the process exits.

---

## Architecture

```
┌─────────────────────────────────────────────────┐
│              task-trigger-mcp daemon             │
│                                                  │
│  ┌─────────────┐    ┌──────────────────────────┐ │
│  │ MCP Server  │    │   Watcher Engine         │ │
│  │ SSE/HTTP    │    │   (notify crate)         │ │
│  │ localhost   │    │   event-driven, always   │ │
│  │ :7755       │    │   running                │ │
│  └──────┬──────┘    └──────────┬───────────────┘ │
│         │                      │                  │
│         └──────────┬───────────┘                  │
│                    │                              │
│  ┌────────────┐  ┌─▼────────────┐ ┌────────────┐ │
│  │ Cron       │  │   SQLite     │ │  Executor  │ │
│  │ Scheduler  │  │   tasks.db   │ │  (spawns   │ │
│  │ (tokio)    │  └──────────────┘ │   CLI)     │ │
│  └────────────┘                   └────────────┘ │
└─────────────────────────────────────────────────┘
         ▲
         │ SSE/HTTP or stdio
    OpenCode / Kiro / Claude Desktop
    (connects when needed, leaves)
```

**Key property**: the agent connects and disconnects freely. Watchers keep running. Scheduled tasks keep firing. The daemon is the source of truth.

### Why not crontab/launchd?

The internal scheduler approach was chosen over OS scheduler integration for these reasons:

- **Zero external dependencies** — the binary is fully self-contained
- **No permission issues** — no crontab editing, no launchd plist management
- **No state sync** — SQLite is the single source of truth (no reconciling DB vs OS state)
- **Cross-platform identical behavior** — same code on Linux, WSL, and macOS
- **Simpler architecture** — one process, one database, one scheduler

---

## Installation

```bash
# Via cargo
cargo install task-trigger-mcp

# Start daemon
task-trigger-mcp daemon start
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
task-trigger-mcp daemon start     # start daemon in background
task-trigger-mcp daemon stop      # stop daemon
task-trigger-mcp daemon status    # check if running, port, version, task/watcher counts
task-trigger-mcp daemon restart   # restart
task-trigger-mcp daemon logs      # tail daemon logs (last 50 lines)
```

Running the binary with no arguments starts the SSE server in the foreground (useful for development/debugging).

Default port: `7755`. Configurable via `--port` flag or `TASK_TRIGGER_PORT` env var.

---

## MCP Tool API

### `task_add`

Registers a new scheduled task. If the ID already exists, it updates the existing task.

**Parameters:**

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Unique identifier. Alphanumeric, hyphens, underscores. If exists, updates. |
| `prompt` | string | yes | The instruction the CLI will execute headlessly |
| `schedule` | string | yes | Standard 5-field cron expression (see Schedule Format) |
| `cli` | enum | yes | `opencode` or `kiro` |
| `model` | string | no | Provider/model string. Omit to use CLI default |
| `duration_minutes` | integer | no | Auto-expire after N minutes from registration |
| `working_dir` | string | no | Working directory for the CLI. Defaults to `$HOME` |

**Schedule format:** Standard 5-field cron only: `minute hour day-of-month month day-of-week`. The model is responsible for converting natural language ("every day at 9am") to cron expressions ("0 9 * * *"). The tool description includes common patterns to guide the model.

**Behavior:** Validates the cron expression, creates the log directory, computes `expires_at` if `duration_minutes` is set, persists the task to SQLite. The daemon's internal cron scheduler picks it up automatically — no external scheduler registration needed.

---

### `task_watch`

Registers a file or directory watcher. When the watched path triggers a matching event, the daemon spawns the CLI headlessly with the given prompt.

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

**Behavior:** Uses native OS APIs (inotify on Linux, FSEvents on macOS) via the `notify` crate. The watcher thread is owned by the daemon process, not by the MCP session. Watchers are persistent — they survive daemon restarts (reloaded from SQLite on startup). Zero token consumption while idle.

---

### `task_list`

Returns all registered scheduled tasks with their current status.

**Returns per task:** id, prompt (truncated to 80 chars), schedule expression, last run timestamp, last run result (success/failure), enabled status, expiry info with time remaining.

---

### `task_watchers`

Returns all active file watchers with their current status.

**Returns per watcher:** id, watched path, events, CLI, status (active/paused/registered), debounce/recursive settings, last triggered timestamp, trigger count.

---

### `task_remove`

Removes a task or watcher completely: stops any active file watcher, deletes from SQLite.

**Parameters:** `id` (string)

---

### `task_unwatch`

Pauses a file watcher without deleting its definition from SQLite. Can be resumed with `task_enable`.

**Parameters:** `id` (string)

---

### `task_enable` / `task_disable`

Enable or disable a scheduled task or watcher without removing it. Enabling a watcher restarts the filesystem monitor. Disabling a watcher stops it.

**Parameters:** `id` (string)

---

### `task_run`

Executes a task immediately, outside its schedule. Useful for testing or one-off execution.

**Parameters:** `id` (string)

**Behavior:** Spawns the CLI headlessly with the task's prompt and model. Output appended to the task's log file with a `manual` trigger marker. Returns the exit code.

---

### `task_logs`

Returns the log output for a task or watcher, plus the 5 most recent execution records from the database.

**Parameters:**

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Task or watcher ID |
| `lines` | integer | no | Last N lines. Default: 50 |
| `since` | string | no | ISO 8601 timestamp filter |

---

### `task_status`

Returns overall health of the daemon.

**Returns:** daemon version, transport (SSE/stdio), port, uptime, scheduler type (internal/tokio), number of active tasks, number of active watchers, temporal tasks with time remaining, log directory path.

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

The internal cron scheduler checks `expires_at` before executing. If expired: the task is skipped. `task_list` and `task_status` report time remaining for all temporal tasks.

---

## Internal Cron Scheduler

The daemon runs a tokio-based cron scheduler that:

1. Checks every 30 seconds which enabled tasks are due
2. Uses the `cron` crate for schedule matching (7-field format internally; 5-field user input is converted by prepending `0` for seconds and appending `*` for year)
3. Deduplicates firings using a `last_fired` HashMap (prevents double-execution within the same cron minute)
4. Skips expired tasks and disabled tasks
5. Uses a `CancellationToken` for graceful shutdown
6. Runs inside the daemon process — no external OS scheduler involvement

---

## CLI Invocation

The binary knows how to invoke each supported CLI headlessly:

**OpenCode:** `opencode run --prompt "<prompt>"` with optional `-m "<model>"`

**Kiro:** `kiro-cli chat --no-interactive --trust-all-tools "<prompt>"` with optional `--model "<model>"`

The binary uses the `which` crate to resolve the full path to the CLI binary, avoiding PATH issues.

---

## Prompt Variable Substitution

At execution time, the binary expands variables in the prompt string before passing it to the CLI:

- `{{TIMESTAMP}}` — current ISO 8601 timestamp
- `{{FILE_PATH}}` — the watched path (file watchers only)
- `{{EVENT_TYPE}}` — the event that triggered (file watchers only: `create`, `modify`, `delete`, `move`)
- `{{TASK_ID}}` — the task's ID
- `{{LOG_PATH}}` — the task's log file path

---

## Logging

Each task/watcher has a dedicated log file at `$HOME/.task-trigger/logs/<id>.log`. Log entries include timestamp, trigger type, exit code, and stdout/stderr from the CLI. Logs are append-only with automatic rotation at 5MB.

Daemon-level logs at `$HOME/.task-trigger/daemon.log`.

---

## Platform Support

| Feature | Linux / WSL | macOS |
|---|---|---|
| Daemon transport | SSE HTTP localhost | SSE HTTP localhost |
| Cron scheduling | Internal (tokio) | Internal (tokio) |
| File watching | inotify (native) | FSEvents (native) |
| State | SQLite | SQLite |
| Binary format | ELF static (musl) | Mach-O |

Platform detected at runtime via `std::env::consts::OS`. No compile-time conditionals exposed to the agent (only used internally for process management via `libc` on unix).

---

## Non-Goals (v1)

- No Windows Task Scheduler support
- No web UI or dashboard
- No MCP sampling dependency
- No git operations
- No distributed or multi-machine scheduling
- No built-in prompt templating beyond variable substitution
- No authentication on the local SSE endpoint (localhost only, trusted environment assumed)
- No natural language to cron conversion (the model handles this)
- No OS scheduler integration (crontab/launchd) — internal scheduler only

---

## Implementation Stack

| Concern | Crate |
|---|---|
| MCP SDK | `rmcp` 0.1.5 + `rmcp-macros` |
| Async runtime | `tokio` |
| SSE/HTTP transport | `axum` (via rmcp http feature) |
| File watching | `notify` |
| Cron parsing | `cron` |
| State | `rusqlite` (bundled) |
| Serialization | `serde` + `serde_json` |
| Schema for tools | `schemars` |
| CLI detection | `which` |
| Logging | `tracing` + `tracing-subscriber` |
| Time | `chrono` |
| CLI parsing | `clap` |
| Error handling | `thiserror` + `anyhow` |
| Graceful shutdown | `tokio_util` (CancellationToken) |
| Unix process mgmt | `libc` (unix only) |

---

## Directory Layout (runtime)

```
$HOME/.task-trigger/
  tasks.db              <- SQLite database
  daemon.pid            <- PID file for daemon management
  daemon.log            <- daemon-level logs
  logs/
    <task-id>.log       <- per-task/watcher logs (5MB rotation)
```

---

## Startup Sequence (daemon)

1. Check if daemon already running via `daemon.pid`
2. Initialize tracing
3. Bind SSE/HTTP server on configured port
4. Write PID file
5. Store daemon state (port, version, start time) in SQLite
6. Load all enabled watchers from SQLite -> re-register with `notify` engine
7. Start internal cron scheduler (tokio loop, 30s interval)
8. Begin serving MCP tool calls
9. On shutdown (SIGTERM/Ctrl+C): cancel scheduler, stop watchers, remove PID file

---

## Roadmap (post-v1)

- `daemon install-service`: install as systemd unit (Linux/WSL) or launchd agent (macOS) for reboot persistence
- `task_rewatch`: resume a paused watcher (currently use `task_enable`)
- Webhook trigger support (HTTP endpoint that fires a task)
- Claude Code CLI support
- Optional auth token for SSE endpoint
- Integration tests (end-to-end MCP tool testing)
