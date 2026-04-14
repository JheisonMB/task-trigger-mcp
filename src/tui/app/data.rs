//! Data refresh — daemon status, agent list, active runs, logs, MCP calls.

use anyhow::Result;

use crate::application::ports::{
    BackgroundAgentRepository, RunRepository, StateRepository, WatcherRepository,
};

use super::{is_process_running, relative_time, tail_lines, AgentEntry, App};

impl App {
    pub(super) fn refresh_daemon_status(&mut self) {
        let pid_path = self.data_dir.join("daemon.pid");
        self.daemon_pid = std::fs::read_to_string(&pid_path)
            .ok()
            .and_then(|s| s.trim().parse().ok());
        self.daemon_running = self.daemon_pid.map(is_process_running).unwrap_or(false);
        self.daemon_version = self
            .db
            .get_state("version")
            .ok()
            .flatten()
            .unwrap_or_default();
    }

    pub(super) fn refresh_agents(&mut self) -> Result<()> {
        let background_agents = self.db.list_background_agents()?;
        let watchers = self.db.list_watchers()?;

        self.agents.clear();
        for t in background_agents {
            self.agents.push(AgentEntry::BackgroundAgent(t));
        }
        for w in watchers {
            self.agents.push(AgentEntry::Watcher(w));
        }
        for i in 0..self.interactive_agents.len() {
            self.agents.push(AgentEntry::Interactive(i));
        }

        let total = self.agents.len();
        if total > 0 && self.selected >= total {
            self.selected = total - 1;
        }

        Ok(())
    }

    pub(super) fn refresh_active_runs(&mut self) -> Result<()> {
        self.active_runs.clear();
        for agent in &self.agents {
            let id = agent.id(self);
            if let Ok(Some(run)) = self.db.get_active_run(id) {
                self.active_runs.insert(id.to_string(), run);
            }
        }
        self.recent_runs = self.db.list_all_recent_runs(50)?;
        Ok(())
    }

    pub(super) fn refresh_log(&mut self) {
        let Some(agent) = self.agents.get(self.selected) else {
            self.log_content = String::from("No agent selected");
            return;
        };

        match agent {
            AgentEntry::Interactive(idx) => {
                if *idx >= self.interactive_agents.len() {
                    self.log_content = String::from("Agent removed");
                    return;
                }
                let output = self.interactive_agents[*idx].output();
                self.log_content = if output.is_empty() {
                    format!(
                        "Agent '{}' — waiting for output...",
                        self.interactive_agents[*idx].id
                    )
                } else {
                    output
                };
            }
            _ => {
                let id = agent.id(self).to_string();
                let log_path = self.data_dir.join("logs").join(format!("{id}.log"));

                let mut content = match std::fs::read_to_string(&log_path) {
                    Ok(c) => tail_lines(&c, 200),
                    Err(_) => String::new(),
                };

                if let Some(run) = self.active_runs.get(&id) {
                    let header = format!(
                        "⏳ Run {} in progress ({})\n{}\n",
                        &run.id[..8.min(run.id.len())],
                        relative_time(&run.started_at),
                        "─".repeat(40),
                    );
                    content = if content.is_empty() {
                        format!("{header}Waiting for output...")
                    } else {
                        format!("{header}{content}")
                    };
                } else if content.is_empty() {
                    content = format!("No logs yet for '{id}'");
                }

                self.log_content = content;
            }
        }
    }
}

pub(crate) fn send_mcp_task_run(port: &str, background_agent_id: &str) -> Result<()> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "agent_run",
            "arguments": { "id": background_agent_id }
        }
    })
    .to_string();

    let request = format!(
        "POST /mcp HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Accept: application/json\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {body}",
        body.len()
    );

    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect_timeout(&addr.parse()?, Duration::from_secs(3))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(request.as_bytes())?;
    let mut buf = [0u8; 4096];
    let _ = stream.read(&mut buf);
    Ok(())
}
