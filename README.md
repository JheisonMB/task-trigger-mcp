# agent-canopy

A modern, self-contained MCP (Multi-Agent Control Point) server for orchestrating AI agent tasks and file event triggers. agent-canopy is designed for reliability, modularity, and performance—enabling advanced scheduling, file watching, and interactive agent management with zero runtime dependencies.

## Key Features

- **Efficient Internal Scheduler:** Event-driven, cron-based scheduling (no polling) using Tokio and precise sleep/wake logic.
- **File Watcher Engine:** Monitors files/directories for create, modify, delete, and move events using the notify crate.
- **Persistent State:** All tasks, watchers, execution logs, and agent state are stored in a bundled SQLite database.
- **Modular Architecture:** Clear separation of concerns (application, daemon, db, domain, executor, scheduler, tui, watchers).
- **Interactive Agents:** Each agent runs in a PTY with a virtual terminal (vt100), supporting full TUI management and colored output.
- **Task/Watcher Models:** Tasks and watchers support expiration, locking, per-run logs, and flexible triggers.
- **Cross-Platform:** Runs on Linux, macOS, and Windows (single static binary, no external dependencies).

## Architecture Overview

- **Daemon:** Owns the MCP server, scheduler, watcher engine, and database. Exposes a Streamable HTTP API and stdio mode.
- **Scheduler:** Computes next fire times for all active tasks, sleeping until needed. Wakes instantly on changes.
- **Watcher Engine:** Reacts to file system events, triggering tasks as defined.
- **Executor:** Runs tasks and agents, manages locking, logs, and status.
- **TUI:** Interactive terminal UI for managing agents and viewing output in real time.

## Main Modules

- `application/` — Application ports and abstractions
- `daemon/` — Daemon process and lifecycle
- `db/` — SQLite persistence and migrations
- `domain/` — Core models: Task, Watcher, ExecutionLog, etc.
- `executor/` — Task and agent execution logic
- `scheduler/` — Internal cron scheduler
- `tui/` — Terminal UI and agent management
- `watchers/` — File system watcher engine

## Usage

1. **Start the daemon:**
   ```bash
   canopy daemon start
   ```
2. **Add tasks and watchers:**
   Use the CLI or API to register scheduled tasks and file event watchers. Each task can specify:
   - `id`, `prompt`, `schedule_expr`, `cli`, `model`, `working_dir`, `timeout_minutes`, etc.
   - Watchers specify `path`, `events`, and trigger logic.
3. **Monitor and manage:**
   - View logs, status, and manage agents interactively via the TUI.
   - All state is persisted in `~/.canopy/tasks.db`.

## Extending

- Add new CLI integrations by extending the `domain` and `executor` modules.
- Implement custom triggers or agent types by building on the modular architecture.

## Tech Stack
- Rust 2021, Tokio, Axum, rusqlite, notify, vt100, ratatui, clap, serde, tracing

## License
MIT — see [LICENSE](LICENSE) for details.

---
Maintained by JheisonMB and UniverLab.
