use crate::error::AppError;
use crate::models::McpConfigFile;
use std::path::Path;

/// Translate MCP config to Claude Code format (.claude/mcp.json).
pub fn translate_for_claude(config: &McpConfigFile) -> serde_json::Value {
    let mut servers = serde_json::Map::new();

    for server in &config.servers {
        if !server.enabled {
            continue;
        }

        let mut obj = serde_json::Map::new();
        obj.insert("command".to_string(), serde_json::Value::String(server.command.clone()));
        obj.insert("args".to_string(), serde_json::to_value(&server.args).unwrap());

        if !server.env.is_empty() {
            obj.insert("env".to_string(), serde_json::to_value(&server.env).unwrap());
        }

        servers.insert(server.name.clone(), serde_json::Value::Object(obj));
    }

    serde_json::json!({ "mcpServers": serde_json::Value::Object(servers) })
}

/// Translate MCP config to Codex format (CODER_MCP_SERVERS env / codex config).
pub fn translate_for_codex(config: &McpConfigFile) -> serde_json::Value {
    let servers: Vec<serde_json::Value> = config
        .servers
        .iter()
        .filter(|s| s.enabled)
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "command": s.command,
                "args": s.args,
            })
        })
        .collect();

    serde_json::json!({ "mcpServers": servers })
}

/// Translate MCP config to Cursor format (.cursor/mcp.json).
pub fn translate_for_cursor(config: &McpConfigFile) -> serde_json::Value {
    translate_for_claude(config) // Cursor uses the same format as Claude
}

/// Translate MCP config to Gemini CLI format.
pub fn translate_for_gemini(config: &McpConfigFile) -> serde_json::Value {
    let servers: Vec<serde_json::Value> = config
        .servers
        .iter()
        .filter(|s| s.enabled)
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "command": s.command,
                "args": s.args,
            })
        })
        .collect();

    serde_json::json!({ "mcpServers": servers })
}

/// Translate MCP config to GitHub Copilot format.
pub fn translate_for_copilot(config: &McpConfigFile) -> serde_json::Value {
    let servers: Vec<serde_json::Value> = config
        .servers
        .iter()
        .filter(|s| s.enabled)
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "command": s.command,
                "args": s.args,
            })
        })
        .collect();

    serde_json::json!({ "mcpServers": servers })
}

/// Apply all translations: write the translated config to the project directory
/// for each installed agent.
pub fn apply_translations(
    config: &McpConfigFile,
    project_path: &Path,
) -> Result<Vec<String>, AppError> {
    let mut applied = Vec::new();

    // Claude Code → .claude/mcp.json
    let claude_path = project_path.join(".claude").join("mcp.json");
    let claude_config = translate_for_claude(config);
    if let Some(parent) = claude_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&claude_config)
        .map_err(|_| AppError::ConfigWriteFailed {
            agent: "Claude Code".to_string(),
            path: claude_path.display().to_string(),
        })?;
    std::fs::write(&claude_path, json)?;
    applied.push(format!("Claude Code: {}", claude_path.display()));

    // Cursor → .cursor/mcp.json
    let cursor_path = project_path.join(".cursor").join("mcp.json");
    let cursor_config = translate_for_cursor(config);
    if let Some(parent) = cursor_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&cursor_config)
        .map_err(|_| AppError::ConfigWriteFailed {
            agent: "Cursor".to_string(),
            path: cursor_path.display().to_string(),
        })?;
    std::fs::write(&cursor_path, json)?;
    applied.push(format!("Cursor: {}", cursor_path.display()));

    // Codex, Gemini, Copilot — write as .dev-workbench/mcp-{agent}.json
    let dw_dir = project_path.join(".dev-workbench");
    let _ = std::fs::create_dir_all(&dw_dir);

    for (agent, translator) in [
        ("codex", translate_for_codex as fn(&McpConfigFile) -> serde_json::Value),
        ("gemini", translate_for_gemini),
        ("copilot", translate_for_copilot),
    ] {
        let path = dw_dir.join(format!("mcp-{}.json", agent));
        let translated = translator(config);
        let json = serde_json::to_string_pretty(&translated)
            .map_err(|_| AppError::ConfigWriteFailed {
                agent: agent.to_string(),
                path: path.display().to_string(),
            })?;
        std::fs::write(&path, json)?;
        applied.push(format!("{}: {}", agent, path.display()));
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::McpServerConfig;
    use std::collections::HashMap;

    fn make_config() -> McpConfigFile {
        McpConfigFile {
            servers: vec![McpServerConfig {
                name: "filesystem".to_string(),
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@mcp/server-filesystem".to_string()],
                env: HashMap::new(),
                enabled: true,
            }],
        }
    }

    #[test]
    fn test_translate_for_claude() {
        let config = make_config();
        let json = translate_for_claude(&config);
        let servers = json.get("mcpServers").unwrap().as_object().unwrap();
        assert!(servers.contains_key("filesystem"));
        assert_eq!(servers["filesystem"]["command"], "npx");
    }

    #[test]
    fn test_translate_disabled_server() {
        let mut config = make_config();
        config.servers[0].enabled = false;
        let json = translate_for_claude(&config);
        let servers = json.get("mcpServers").unwrap().as_object().unwrap();
        assert!(!servers.contains_key("filesystem"));
    }

    #[test]
    fn test_apply_translations() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_dir = tmp.path().join("my-project");
        std::fs::create_dir_all(&project_dir).unwrap();

        let config = make_config();
        let applied = apply_translations(&config, &project_dir).unwrap();

        assert!(applied.len() >= 2);
        assert!(project_dir.join(".claude/mcp.json").exists());
        assert!(project_dir.join(".cursor/mcp.json").exists());
    }
}
