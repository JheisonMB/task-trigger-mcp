# MCP Configuración Multi-Cliente

Configuración unificada de servidores MCP (`fetch`, `filesystem`, `memory`) para distintos clientes.

---

## OpenCode — `opencode.jsonc`

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "fetch": {
      "type": "local",
      "command": ["uvx", "mcp-server-fetch"],
      "enabled": true
    },
    "filesystem": {
      "type": "local",
      "command": [
        "npx",
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/mnt/c/Users/PC/Documents"
      ],
      "enabled": true
    },
    "memory": {
      "type": "local",
      "command": ["npx", "-y", "@modelcontextprotocol/server-memory"],
      "enabled": true,
      "environment": {
        "MEMORY_FILE_PATH": "/mnt/c/Users/PC/Documents/mcp-memory/memory.jsonl"
      }
    }
  }
}
```

---

## Copilot CLI — `~/.copilot/mcp-config.json`

```json
{
  "mcpServers": {
    "fetch": {
      "type": "local",
      "command": "uvx",
      "args": ["mcp-server-fetch"],
      "env": {},
      "tools": ["*"]
    },
    "filesystem": {
      "type": "local",
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/mnt/c/Users/PC/Documents"
      ],
      "env": {},
      "tools": ["*"]
    },
    "memory": {
      "type": "local",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"],
      "env": {
        "MEMORY_FILE_PATH": "/mnt/c/Users/PC/Documents/mcp-memory/memory.jsonl"
      },
      "tools": ["*"]
    }
  }
}
```

---

## Mistral Vibe — `~/.vibe/config.toml`

```toml
[[mcp_servers]]
name = "fetch"
transport = "stdio"
command = "uvx"
args = ["mcp-server-fetch"]

[[mcp_servers]]
name = "filesystem"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/mnt/c/Users/PC/Documents"]

[[mcp_servers]]
name = "memory"
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-memory"]
env = { "MEMORY_FILE_PATH" = "/mnt/c/Users/PC/Documents/mcp-memory/memory.jsonl" }
```

---

## Codex — `~/.codex/config.toml`

```toml
[mcp_servers.fetch]
command = "uvx"
args = ["mcp-server-fetch"]

[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/mnt/c/Users/PC/Documents"]

[mcp_servers.memory]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-memory"]

[mcp_servers.memory.env]
MEMORY_FILE_PATH = "/mnt/c/Users/PC/Documents/mcp-memory/memory.jsonl"
```

---

## Kiro — `.kiro/settings/mcp.json` o `~/.kiro/settings/mcp.json`

```json
{
  "mcpServers": {
    "fetch": {
      "command": "uvx",
      "args": ["mcp-server-fetch"]
    },
    "filesystem": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/mnt/c/Users/PC/Documents"
      ]
    },
    "memory": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"],
      "env": {
        "MEMORY_FILE_PATH": "/mnt/c/Users/PC/Documents/mcp-memory/memory.jsonl"
      }
    }
  }
}
```

---

## Qwen Code — `settings.json`

```json
{
  "mcpServers": {
    "fetch": {
      "command": "uvx",
      "args": ["mcp-server-fetch"]
    },
    "filesystem": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/mnt/c/Users/PC/Documents"
      ]
    },
    "memory": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"],
      "env": {
        "MEMORY_FILE_PATH": "/mnt/c/Users/PC/Documents/mcp-memory/memory.jsonl"
      }
    }
  }
}
```

---

## Claude Code — `.mcp.json` (proyecto) o vía CLI

```json
{
  "mcpServers": {
    "fetch": {
      "type": "stdio",
      "command": "uvx",
      "args": ["mcp-server-fetch"]
    },
    "filesystem": {
      "type": "stdio",
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/mnt/c/Users/PC/Documents"
      ]
    },
    "memory": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"],
      "env": {
        "MEMORY_FILE_PATH": "/mnt/c/Users/PC/Documents/mcp-memory/memory.jsonl"
      }
    }
  }
}
```

---

## Gemini CLI — `~/.gemini/settings.json` o `.gemini/settings.json`

```json
{
  "mcpServers": {
    "fetch": {
      "command": "uvx",
      "args": ["mcp-server-fetch"]
    },
    "filesystem": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/mnt/c/Users/PC/Documents"
      ]
    },
    "memory": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-memory"],
      "env": {
        "MEMORY_FILE_PATH": "/mnt/c/Users/PC/Documents/mcp-memory/memory.jsonl"
      }
    }
  }
}
```
