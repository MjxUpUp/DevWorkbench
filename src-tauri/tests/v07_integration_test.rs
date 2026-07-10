//! v0.7.0 Integration Tests
//!
//! Tests the config, quality, and activity modules
//! through their public API, using real SQLite databases.

// === Helpers ===

struct TempDb {
    _tmp: tempfile::TempDir,
    conn: rusqlite::Connection,
}

impl TempDb {
    fn new() -> Self {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let conn = app_lib::db::init_db(&db_path).expect("init_db failed");
        Self { _tmp: tmp, conn }
    }
}

// === Activity Integration ===

#[test]
fn test_activity_record_and_query() {
    let db = TempDb::new();

    let e1 = app_lib::activity::make_activity_event(
        "s1",
        "/proj/a",
        &app_lib::models::AgentType::ReactKernel,
        "session_started",
        "Started session",
        None,
        None,
    );
    let e2 = app_lib::activity::make_activity_event(
        "s1",
        "/proj/a",
        &app_lib::models::AgentType::ReactKernel,
        "session_completed",
        "Completed session",
        Some("All tests passed".to_string()),
        Some(vec!["src/main.rs".to_string()]),
    );
    let e3 = app_lib::activity::make_activity_event(
        "s2",
        "/proj/b",
        &app_lib::models::AgentType::ReactKernel,
        "session_started",
        "Started Codex session",
        None,
        None,
    );

    app_lib::activity::record_event(&db.conn, &e1).unwrap();
    app_lib::activity::record_event(&db.conn, &e2).unwrap();
    app_lib::activity::record_event(&db.conn, &e3).unwrap();

    // Query by project
    let proj_a = app_lib::activity::get_events_for_project(&db.conn, "/proj/a").unwrap();
    assert_eq!(proj_a.len(), 2);
    assert_eq!(proj_a[0].event_type, "session_completed"); // most recent first
    assert!(proj_a[0].files_changed.is_some());

    let proj_b = app_lib::activity::get_events_for_project(&db.conn, "/proj/b").unwrap();
    assert_eq!(proj_b.len(), 1);
    assert_eq!(proj_b[0].agent_type, app_lib::models::AgentType::ReactKernel);

    // Recent across all projects
    let recent = app_lib::activity::get_recent_events(&db.conn, 10).unwrap();
    assert_eq!(recent.len(), 3);
}

#[test]
fn test_activity_hash_consistency() {
    let h1 = app_lib::activity::hash_project_path("/foo/bar");
    let h2 = app_lib::activity::hash_project_path("/foo/bar");
    let h3 = app_lib::activity::hash_project_path("/foo/baz");
    assert_eq!(h1, h2, "Same path must produce same hash");
    assert_ne!(h1, h3, "Different paths must produce different hashes");
    assert_eq!(h1.len(), 16, "Hash should be 16 hex chars");
}

// === Quality Report Integration ===

#[test]
fn test_quality_report_save_and_get() {
    let db = TempDb::new();

    let report = app_lib::models::QualityReport {
        id: "qr-1".to_string(),
        session_id: "s-quality-1".to_string(),
        checks: vec![
            app_lib::models::QualityCheck {
                name: "compile".to_string(),
                status: "passed".to_string(),
                message: None,
            },
            app_lib::models::QualityCheck {
                name: "assertion".to_string(),
                status: "failed".to_string(),
                message: Some("Assertion weakened in test_foo: t.Fatal -> t.Log".to_string()),
            },
        ],
        overall_status: "failed".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
    };

    app_lib::quality::report::save_report(&db.conn, &report).unwrap();

    // Get by session
    let fetched =
        app_lib::quality::report::get_report_for_session(&db.conn, "s-quality-1").unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id, "qr-1");
    assert_eq!(fetched.checks.len(), 2);
    assert_eq!(fetched.overall_status, "failed");
    assert_eq!(fetched.checks[1].name, "assertion");
    assert!(fetched.checks[1].message.is_some());

    // Get all
    let all = app_lib::quality::report::get_all_reports(&db.conn).unwrap();
    assert_eq!(all.len(), 1);

    // Nonexistent session
    let missing =
        app_lib::quality::report::get_report_for_session(&db.conn, "nonexistent").unwrap();
    assert!(missing.is_none());
}

// === Quality Feedback Integration (cross-module) ===

#[test]
fn test_quality_feedback_creates_activity() {
    let db = TempDb::new();

    let report = app_lib::models::QualityReport {
        id: "qr-fb-1".to_string(),
        session_id: "s-fb-1".to_string(),
        checks: vec![app_lib::models::QualityCheck {
            name: "compile".to_string(),
            status: "failed".to_string(),
            message: Some("cargo check failed: unresolved import".to_string()),
        }],
        overall_status: "failed".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
    };

    app_lib::quality::feedback::create_feedback(
        &db.conn,
        &report,
        "/test/feedback/project",
        &app_lib::models::AgentType::ReactKernel,
    )
    .unwrap();

    // Should create an activity event
    let events =
        app_lib::activity::get_events_for_project(&db.conn, "/test/feedback/project").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "quality_gate");
    assert!(events[0].title.contains("failed"));
}

// === Config MCP Integration ===

#[test]
fn test_mcp_config_roundtrip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("mcp-servers.toml");

    let config = app_lib::models::McpConfigFile {
        servers: vec![app_lib::models::McpServerConfig {
            name: "test-server".to_string(),
            command: "npx".to_string(),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-test".to_string(),
            ],
            env: [("API_KEY".to_string(), "secret".to_string())]
                .into_iter()
                .collect(),
            enabled: true,
        }],
    };

    // Save
    app_lib::config::mcp::save_mcp_config(&config, &config_path).unwrap();

    // Load
    let loaded = app_lib::config::mcp::load_mcp_config(&config_path).unwrap();
    assert_eq!(loaded.servers.len(), 1);
    assert_eq!(loaded.servers[0].name, "test-server");
    assert_eq!(loaded.servers[0].command, "npx");
    assert_eq!(loaded.servers[0].args.len(), 2);
    assert_eq!(loaded.servers[0].env.get("API_KEY").unwrap(), "secret");
    assert!(loaded.servers[0].enabled);
}

#[test]
fn test_mcp_config_missing_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let config_path = tmp.path().join("nonexistent.toml");

    let result = app_lib::config::mcp::load_mcp_config(&config_path);
    assert!(result.is_err());
}

#[test]
fn test_mcp_translate_for_claude() {
    let config = app_lib::models::McpConfigFile {
        servers: vec![
            app_lib::models::McpServerConfig {
                name: "my-server".to_string(),
                command: "node".to_string(),
                args: vec!["server.js".to_string()],
                env: [("KEY".to_string(), "val".to_string())]
                    .into_iter()
                    .collect(),
                enabled: true,
            },
            app_lib::models::McpServerConfig {
                name: "disabled-server".to_string(),
                command: "node".to_string(),
                args: vec![],
                env: Default::default(),
                enabled: false,
            },
        ],
    };

    let parsed = app_lib::config::adapters::translate_for_claude(&config);

    let servers = parsed.get("mcpServers").unwrap().as_object().unwrap();
    assert!(servers.contains_key("my-server"));
    assert!(
        !servers.contains_key("disabled-server"),
        "disabled server should be excluded"
    );

    let my_server = servers.get("my-server").unwrap();
    assert_eq!(my_server["command"].as_str().unwrap(), "node");
}
