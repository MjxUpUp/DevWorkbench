use crate::error::AppError;
use crate::models::McpConfigFile;
use std::path::Path;

/// Parse an MCP server configuration from a TOML file.
pub fn parse_mcp_config(content: &str) -> Result<McpConfigFile, AppError> {
    let raw: toml::Value = toml::from_str(content)?;

    let mut servers = Vec::new();

    let servers_table = raw
        .get("servers")
        .and_then(|s| s.as_table())
        .ok_or_else(|| AppError::Config("Missing [servers] section".to_string()))?;

    for (name, value) in servers_table {
        let table = value
            .as_table()
            .ok_or_else(|| AppError::Config(format!("Server '{}' must be a table", name)))?;

        let command = table
            .get("command")
            .and_then(|c| c.as_str())
            .ok_or_else(|| AppError::Config(format!("Server '{}' missing 'command'", name)))?
            .to_string();

        let args = table
            .get("args")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let env = table
            .get("env")
            .and_then(|e| e.as_table())
            .map(|t| {
                t.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        let enabled = table
            .get("enabled")
            .and_then(|e| e.as_bool())
            .unwrap_or(true);

        let target_agents = table
            .get("target_agents")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        v.as_str().and_then(|s| match s {
                            "claude" => Some(crate::models::AgentType::ClaudeCode),
                            "codex" => Some(crate::models::AgentType::Codex),
                            "cursor" => Some(crate::models::AgentType::CursorAgent),
                            "gemini" => Some(crate::models::AgentType::GeminiCli),
                            "copilot" => Some(crate::models::AgentType::Copilot),
                            "qwen" => Some(crate::models::AgentType::QwenCode),
                            "pi" => Some(crate::models::AgentType::Pi),
                            _ => None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_else(|| crate::models::AgentType::all());

        servers.push(crate::models::McpServerConfig {
            name: name.clone(),
            command,
            args,
            env,
            enabled,
            target_agents,
        });
    }

    Ok(McpConfigFile { servers })
}

/// Save MCP config to a TOML file at the given path.
pub fn save_mcp_config(config: &McpConfigFile, path: &Path) -> Result<(), AppError> {
    let mut toml_str = String::from("[servers]\n");

    for server in &config.servers {
        toml_str.push_str(&format!("\n[servers.{}]\n", server.name));
        toml_str.push_str(&format!("command = {:?}\n", server.command));

        if !server.args.is_empty() {
            let args: Vec<String> = server.args.iter().map(|a| format!("{:?}", a)).collect();
            toml_str.push_str(&format!("args = [{}]\n", args.join(", ")));
        }

        if !server.env.is_empty() {
            toml_str.push_str(&format!("[servers.{}.env]\n", server.name));
            for (k, v) in &server.env {
                toml_str.push_str(&format!("{} = {:?}\n", k, v));
            }
        }

        if !server.enabled {
            toml_str.push_str("enabled = false\n");
        }

        if server.target_agents.len() < crate::models::AgentType::all().len() {
            let agents: Vec<String> = server
                .target_agents
                .iter()
                .map(|a| match a {
                    crate::models::AgentType::ClaudeCode => "\"claude\"".to_string(),
                    crate::models::AgentType::Codex => "\"codex\"".to_string(),
                    crate::models::AgentType::CursorAgent => "\"cursor\"".to_string(),
                    crate::models::AgentType::GeminiCli => "\"gemini\"".to_string(),
                    crate::models::AgentType::Copilot => "\"copilot\"".to_string(),
                    crate::models::AgentType::QwenCode => "\"qwen\"".to_string(),
                    crate::models::AgentType::Pi => "\"pi\"".to_string(),
                    // ReactKernel is not a CLI — MCP translation is CLI-only, so
                    // it never legitimately appears in target_agents. Empty
                    // placeholder keeps the match exhaustive; unreachable.
                    crate::models::AgentType::ReactKernel => "\"\"".to_string(),
                })
                .collect();
            toml_str.push_str(&format!("target_agents = [{}]\n", agents.join(", ")));
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, toml_str)?;
    Ok(())
}

/// Load MCP config from a TOML file at the given path.
pub fn load_mcp_config(path: &Path) -> Result<McpConfigFile, AppError> {
    let content = std::fs::read_to_string(path)?;
    parse_mcp_config(&content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::AgentType;

    #[test]
    fn test_parse_mcp_config() {
        let toml = r#"
[servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
enabled = true
target_agents = ["claude", "codex"]

[servers.github]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
"#;
        let config = parse_mcp_config(toml).unwrap();
        assert_eq!(config.servers.len(), 2);

        let fs = &config.servers[0];
        assert_eq!(fs.name, "filesystem");
        assert_eq!(fs.command, "npx");
        assert_eq!(fs.args.len(), 3);
        assert!(fs.enabled);
        assert_eq!(fs.target_agents.len(), 2);

        let gh = &config.servers[1];
        assert_eq!(gh.name, "github");
        // default: all agents
        assert_eq!(gh.target_agents.len(), AgentType::all().len());
    }

    #[test]
    fn test_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("mcp-servers.toml");

        let config = McpConfigFile {
            servers: vec![crate::models::McpServerConfig {
                name: "test".to_string(),
                command: "echo".to_string(),
                args: vec!["hello".to_string()],
                env: std::collections::HashMap::new(),
                enabled: true,
                target_agents: vec![AgentType::ClaudeCode],
            }],
        };

        save_mcp_config(&config, &path).unwrap();
        assert!(path.exists());

        let loaded = load_mcp_config(&path).unwrap();
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "test");
    }

    #[test]
    fn test_roundtrip_with_env() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("mcp-servers.toml");

        let mut env = std::collections::HashMap::new();
        env.insert("API_KEY".to_string(), "secret123".to_string());
        env.insert("DEBUG".to_string(), "true".to_string());

        let config = McpConfigFile {
            servers: vec![crate::models::McpServerConfig {
                name: "github".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@mcp/server-github".to_string()],
                env,
                enabled: true,
                target_agents: vec![AgentType::ClaudeCode, AgentType::Codex],
            }],
        };

        save_mcp_config(&config, &path).unwrap();
        let loaded = load_mcp_config(&path).unwrap();
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "github");
        assert_eq!(loaded.servers[0].env.len(), 2);
        assert_eq!(loaded.servers[0].env.get("API_KEY"), Some(&"secret123".to_string()));
    }

    #[test]
    fn test_parse_missing_servers() {
        let result = parse_mcp_config("other = true");
        assert!(result.is_err());
    }
}
