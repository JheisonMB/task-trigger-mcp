# task-trigger-mcp

A self-contained Rust MCP server for managing scheduled and event-driven tasks.

## Status

**Current Version**: v0.1.0 (Work in Progress)

This project is in **early development**. The foundational architecture is in place but the MCP transport layer and many features are still being implemented.

### Completed ✅

- **Project Structure**: Complete Cargo setup with proper dependency management
- **State Models**: Task, Watcher, and event types defined
- **SQLite Database Layer**: Full CRUD operations for tasks, watchers, runs, and daemon state
- **Parameter/Response Types**: JSON schemas for all MCP tool parameters and responses  
- **Scheduler Utilities**: Natural language to cron conversion (e.g., "every 5 minutes" → "*/5 * * * *")
- **Variable Substitution**: Placeholder expansion in task prompts ({{TIMESTAMP}}, {{TASK_ID}}, etc.)

### In Progress 🚧

- **MCP Tool Handlers**: Converting rmcp macros to work with the 0.8 SDK limitations
- **HTTP/SSE Transport**: Setting up the daemon transport layer
- **File Watcher Engine**: Integrating with `notify` crate for file monitoring

### TODO 📋

High Priority:
- [ ] Implement working MCP tool handlers using rmcp 0.8 compatible patterns
- [ ] Create HTTP/SSE transport server (daemon mode)
- [ ] Implement file watching with debouncing
- [ ] Integration with OS schedulers (crontab on Linux, launchd on macOS)
- [ ] Task execution via subprocess with headless CLI support
- [ ] Log file management with rotation

Medium Priority:
- [ ] Daemon CLI commands (start, stop, status, logs)
- [ ] Tests (unit, integration)
- [ ] Configuration file support
- [ ] Error recovery and resilience

Lower Priority:
- [ ] Webhook trigger support
- [ ] Metrics/observability
- [ ] Web UI

## Building

```bash
cargo build --release
```

## Project Layout

```
src/
  ├── main.rs          - Entry point
  ├── state/           - Core data models
  ├── db/              - SQLite persistence layer
  ├── tools/           - MCP tool parameter/response types
  ├── daemon/          - MCP server handler (WIP)
  ├── scheduler/       - Cron parsing and variable substitution
  ├── watchers/        - File watching engine (stub)
  └── executor/        - Task execution (stub)
```

## Architecture

The daemon operates in three modes:
1. **Daemon mode** (SSE/HTTP): Long-running background process serving MCP tools
2. **Executor mode**: Called by OS scheduler to execute individual tasks
3. **Stdio mode** (fallback): Direct MCP server over stdin/stdout

## Dependencies

- **rmcp**: Official Rust MCP SDK v0.8
- **tokio**: Async runtime
- **rusqlite**: SQLite driver with bundled library
- **notify**: Cross-platform file system monitoring
- **chrono**: Date/time handling
- **serde/serde_json**: Serialization
- **schemars**: JSON schema generation

## Notes

The MCP SDK (rmcp 0.8) has strict type requirements for tool handlers. The current implementation uses a stub handler while we determine the best approach to implement all tools within rmcp's constraints.

## License

[To be determined]
