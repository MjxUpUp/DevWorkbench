//! v0.7.0 Integration Tests
//!
//! Tests the knowledge, config, quality, and activity modules
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

// === Knowledge Store Integration ===

#[test]
fn test_knowledge_add_search_delete_flow() {
    let db = TempDb::new();
    let entry = app_lib::models::KnowledgeEntry {
        id: "k-int-1".to_string(),
        project_hash: "abcd1234".to_string(),
        category: "insight".to_string(),
        title: "Rust async patterns with tokio".to_string(),
        content: "Use tokio::spawn for concurrent tasks and join_all to collect results"
            .to_string(),
        source_agent: app_lib::models::AgentType::ClaudeCode,
        source_session_id: None,
        source_type: "auto_collect".to_string(),
        confidence: 0.85,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        access_count: 0,
        status: "active".to_string(),
        effectiveness: 0.0,
    };

    // Add
    app_lib::knowledge::store::add_entry(&db.conn, &entry).unwrap();

    // Search
    let results = app_lib::knowledge::store::search_entries(&db.conn, "tokio async", 10).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, "k-int-1");
    assert!((results[0].confidence - 0.85).abs() < 0.01);

    // Get by project
    let proj_entries =
        app_lib::knowledge::store::get_entries_for_project(&db.conn, "abcd1234").unwrap();
    assert_eq!(proj_entries.len(), 1);

    // Delete
    app_lib::knowledge::store::delete_entry(&db.conn, "k-int-1").unwrap();
    let after_delete = app_lib::knowledge::store::search_entries(&db.conn, "tokio", 10).unwrap();
    assert!(after_delete.is_empty());
}

#[test]
fn test_knowledge_fts_multiple_entries() {
    let db = TempDb::new();

    let entries = vec![
        make_knowledge_entry(
            "k1",
            "proj_a",
            "CSS Grid vs Flexbox",
            "Use CSS Grid for 2D layouts and Flexbox for 1D alignment",
        ),
        make_knowledge_entry(
            "k2",
            "proj_a",
            "React hooks best practices",
            "Use useEffect cleanup to avoid memory leaks in React components",
        ),
        make_knowledge_entry(
            "k3",
            "proj_b",
            "Tauri IPC commands",
            "Use invoke() to call Rust functions from the frontend",
        ),
    ];

    for e in &entries {
        app_lib::knowledge::store::add_entry(&db.conn, e).unwrap();
    }

    // FTS should find relevant entries
    let css_results = app_lib::knowledge::store::search_entries(&db.conn, "CSS Grid", 10).unwrap();
    assert_eq!(css_results.len(), 1);
    assert_eq!(css_results[0].id, "k1");

    let react_results =
        app_lib::knowledge::store::search_entries(&db.conn, "React hooks", 10).unwrap();
    assert_eq!(react_results.len(), 1);
    assert_eq!(react_results[0].id, "k2");

    // Project filter
    let proj_a = app_lib::knowledge::store::get_entries_for_project(&db.conn, "proj_a").unwrap();
    assert_eq!(proj_a.len(), 2);
}

// === Knowledge Collector Integration ===

#[test]
fn test_knowledge_collect_from_log() {
    let tmp = tempfile::TempDir::new().unwrap();
    let log_path = tmp.path().join("session-123.log");
    std::fs::write(&log_path, "Building project with cargo build\nRunning tests with cargo test\nAll 10 tests passed successfully\n").unwrap();

    let entries = app_lib::knowledge::collector::parse_devworkbench_log(
        "/test/project",
        &log_path,
        "session-123",
        &app_lib::models::AgentType::ClaudeCode,
    )
    .unwrap();

    assert!(!entries.is_empty());
    for e in &entries {
        assert_eq!(e.source_session_id, Some("session-123".to_string()));
        assert_eq!(e.source_type, "devworkbench_log");
    }
}

#[test]
fn test_knowledge_collect_from_jsonl() {
    let tmp = tempfile::TempDir::new().unwrap();
    let jsonl_path = tmp.path().join("session.jsonl");

    // Write a minimal Claude Code JSONL with an assistant text block
    let jsonl_content = r#"{"role":"assistant","content":[{"type":"text","text":"This is a detailed analysis of the error handling patterns in the project. The codebase uses thiserror for custom error types and anyhow for application-level error propagation."}]}
"#;
    std::fs::write(&jsonl_path, jsonl_content).unwrap();

    let entries = app_lib::knowledge::collector::parse_claude_jsonl(
        "/test/project",
        &jsonl_path,
        Some("session-456"),
    )
    .unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].source_type, "claude_jsonl");
    assert!(entries[0].content.contains("thiserror"));
}

// === Knowledge Injector Integration ===

#[test]
fn test_knowledge_inject_for_claude_code() {
    let db = TempDb::new();
    let project_path = "/proj/inject_test";
    let project_hash = app_lib::activity::hash_project_path(project_path);
    let entry = make_knowledge_entry(
        "k-inj-1",
        &project_hash,
        "Error handling pattern",
        "Use thiserror for library errors",
    );
    app_lib::knowledge::store::add_entry(&db.conn, &entry).unwrap();

    let prompt = "Fix the error handling in this module";
    let injected = app_lib::knowledge::injector::inject_for_agent(
        &db.conn,
        &app_lib::models::AgentType::ClaudeCode,
        project_path,
        prompt,
    );

    // Claude Code gets a structured markdown block
    assert!(
        injected.contains("Project Knowledge"),
        "should contain knowledge header"
    );
    assert!(injected.contains(prompt), "should contain original prompt");
}

#[test]
fn test_knowledge_inject_for_other_agents() {
    let db = TempDb::new();
    let project_path = "/proj/inject_test_other";
    let project_hash = app_lib::activity::hash_project_path(project_path);
    let entry = make_knowledge_entry(
        "k-inj-2",
        &project_hash,
        "Build configuration",
        "Use cargo nextest for faster test runs",
    );
    app_lib::knowledge::store::add_entry(&db.conn, &entry).unwrap();

    let prompt = "Run the test suite";
    let injected = app_lib::knowledge::injector::inject_for_agent(
        &db.conn,
        &app_lib::models::AgentType::Codex,
        project_path,
        prompt,
    );

    // Non-Claude agents get a simpler bracket format
    assert!(injected.contains("[Project Knowledge]"));
    assert!(injected.contains(prompt));
}

#[test]
fn test_knowledge_inject_empty_returns_original_prompt() {
    let db = TempDb::new();
    let prompt = "Do something";
    let result = app_lib::knowledge::injector::inject_for_agent(
        &db.conn,
        &app_lib::models::AgentType::ClaudeCode,
        "/nonexistent",
        prompt,
    );
    assert_eq!(result, prompt);
}

// === Activity Integration ===

#[test]
fn test_activity_record_and_query() {
    let db = TempDb::new();

    let e1 = app_lib::activity::make_activity_event(
        "s1",
        "/proj/a",
        &app_lib::models::AgentType::ClaudeCode,
        "session_started",
        "Started session",
        None,
        None,
    );
    let e2 = app_lib::activity::make_activity_event(
        "s1",
        "/proj/a",
        &app_lib::models::AgentType::ClaudeCode,
        "session_completed",
        "Completed session",
        Some("All tests passed".to_string()),
        Some(vec!["src/main.rs".to_string()]),
    );
    let e3 = app_lib::activity::make_activity_event(
        "s2",
        "/proj/b",
        &app_lib::models::AgentType::Codex,
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
    assert_eq!(proj_b[0].agent_type, app_lib::models::AgentType::Codex);

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
fn test_quality_feedback_creates_activity_and_knowledge() {
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
        &app_lib::models::AgentType::ClaudeCode,
    )
    .unwrap();

    // Should create an activity event
    let events =
        app_lib::activity::get_events_for_project(&db.conn, "/test/feedback/project").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_type, "quality_gate");
    assert!(events[0].title.contains("failed"));

    // Should create a knowledge entry for the failure
    let hash = app_lib::activity::hash_project_path("/test/feedback/project");
    let knowledge = app_lib::knowledge::store::get_entries_for_project(&db.conn, &hash).unwrap();
    assert_eq!(knowledge.len(), 1);
    assert_eq!(knowledge[0].category, "quality_failure");
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

// === Cross-module: Full Session Lifecycle ===

#[test]
fn test_full_session_knowledge_lifecycle() {
    let db = TempDb::new();
    let tmp = tempfile::TempDir::new().unwrap();

    // 1. Record session start activity
    let start_event = app_lib::activity::make_activity_event(
        "lifecycle-s1",
        "/proj/lifecycle",
        &app_lib::models::AgentType::ClaudeCode,
        "session_started",
        "Started session: fix error handling",
        None,
        None,
    );
    app_lib::activity::record_event(&db.conn, &start_event).unwrap();

    // 2. Simulate agent output
    let log_path = tmp.path().join("outputs").join("lifecycle-s1.log");
    std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    std::fs::write(&log_path, "Analyzing error types in the codebase. Found 3 places using unwrap() instead of proper error handling. Recommend using thiserror for custom error types and anyhow for application errors.").unwrap();

    // 3. Collect knowledge from session output
    let entries = app_lib::knowledge::collector::parse_devworkbench_log(
        "/proj/lifecycle",
        &log_path,
        "lifecycle-s1",
        &app_lib::models::AgentType::ClaudeCode,
    )
    .unwrap();
    for entry in &entries {
        app_lib::knowledge::store::add_entry(&db.conn, entry).unwrap();
    }
    assert!(
        !entries.is_empty(),
        "Should collect knowledge entries from log"
    );

    // 4. Record session completion activity
    let complete_event = app_lib::activity::make_activity_event(
        "lifecycle-s1",
        "/proj/lifecycle",
        &app_lib::models::AgentType::ClaudeCode,
        "session_completed",
        "Completed: fix error handling",
        Some("Replaced unwrap() with proper error handling".to_string()),
        Some(vec!["src/error.rs".to_string(), "src/main.rs".to_string()]),
    );
    app_lib::activity::record_event(&db.conn, &complete_event).unwrap();

    // 5. Verify knowledge is searchable
    let search_results =
        app_lib::knowledge::store::search_entries(&db.conn, "error handling thiserror", 10)
            .unwrap();
    assert!(
        !search_results.is_empty(),
        "Knowledge should be searchable after collection"
    );

    // 6. Verify activity timeline
    let events = app_lib::activity::get_events_for_project(&db.conn, "/proj/lifecycle").unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event_type, "session_completed");
    assert_eq!(events[0].files_changed.as_ref().unwrap().len(), 2);

    // 7. Verify knowledge injection works with collected data
    let injected = app_lib::knowledge::injector::inject_for_agent(
        &db.conn,
        &app_lib::models::AgentType::ClaudeCode,
        "/proj/lifecycle",
        "Continue fixing errors",
    );
    assert!(
        injected.contains("error"),
        "Injected prompt should contain knowledge context"
    );
}

// === Test Helpers ===

fn make_knowledge_entry(
    id: &str,
    project_hash: &str,
    title: &str,
    content: &str,
) -> app_lib::models::KnowledgeEntry {
    app_lib::models::KnowledgeEntry {
        id: id.to_string(),
        project_hash: project_hash.to_string(),
        category: "insight".to_string(),
        title: title.to_string(),
        content: content.to_string(),
        source_agent: app_lib::models::AgentType::ClaudeCode,
        source_session_id: None,
        source_type: "test".to_string(),
        confidence: 0.8,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        access_count: 0,
        status: "active".to_string(),
        effectiveness: 0.0,
    }
}
