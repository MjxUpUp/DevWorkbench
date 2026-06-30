//! CI 修复验证（绕过 app_lib 测试 exe 0xc0000139 loader 问题）。
//!
//! app_lib 的 `#[cfg(test)]` 测试 exe 本机加载失败（STATUS_ENTRYPOINT_NOT_FOUND），
//! 跑不了 `injector.rs` / `executor.rs` 的单测。example 是普通 binary 不受影响——
//! 这里对真实 FTS5 + inject_for_agent 做端到端验证，覆盖两个 pre-existing 失败测试：
//!
//!   1. cross-project injection（store.rs 双重 sanitize 移除）：
//!      从 /proj/a 查 "Fix the Rust error handling"，应在 result 产出 "Cross-project"
//!      + proj_b 的 "Rust error handling"。修复前 `sanitize_fts_query` 把
//!      extract_keywords 的 OR 查询 `"fix" OR "rust" OR ...` 整体包成单短语 →
//!      FTS 要求连续序列 → 永不匹配 → cross-project 静默返回空。
//!
//!   2. memory dedup 机制（executor.rs 测试数据修复）：
//!      `add_entry` 对 (project_hash, content 前 200 字符) 去重。原测试 3 条目用
//!      相同 content "内容" + 相同 project_hash → 去重只留 1 条，断言对着未入库的
//!      条目 panic。memory_prompt_suffix 是私有 fn 无法从 example 直调，这里验证
//!      它依赖的去重机制本身——证明 distinct-content 修法正确且必要。
//!
//! ```sh
//! cargo run --example cross_project_smoke --release
//! ```

use std::error::Error;

use app_lib::activity::hash_project_path;
use app_lib::db;
use app_lib::knowledge::injector::inject_for_agent;
use app_lib::knowledge::store::{add_entry, get_entries_for_project};
use app_lib::models::{AgentType, KnowledgeEntry};

fn make_entry(id: &str, project_hash: &str, title: &str, content: &str) -> KnowledgeEntry {
    KnowledgeEntry {
        id: id.into(),
        project_hash: project_hash.into(),
        category: "insight".into(),
        title: title.into(),
        content: content.into(),
        source_agent: AgentType::ClaudeCode,
        source_session_id: None,
        source_type: "test".into(),
        confidence: 0.8,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
        access_count: 0,
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // === Fix 2: cross-project injection（store.rs 双重 sanitize 修复）===
    let tmp = tempfile::TempDir::new()?;
    let conn = db::init_db(&tmp.path().join("smoke.db"))?;
    let hash_a = hash_project_path("/proj/a");
    let hash_b = hash_project_path("/proj/b");
    add_entry(
        &conn,
        &make_entry("k1", &hash_a, "CSS tip", "Use CSS variables for theming"),
    )?;
    add_entry(
        &conn,
        &make_entry(
            "k2",
            &hash_b,
            "Rust error handling",
            "Use thiserror for Rust error types",
        ),
    )?;

    let result = inject_for_agent(
        &conn,
        &AgentType::ClaudeCode,
        "/proj/a",
        "Fix the Rust error handling",
    );
    if !result.contains("Cross-project") {
        return Err(format!(
            "FAIL (Fix 2): result 缺 'Cross-project' 标签（cross-project FTS 仍被双重 sanitize 破坏）:\n{result}"
        )
        .into());
    }
    if !result.contains("Rust error handling") {
        return Err(format!(
            "FAIL (Fix 2): result 缺 proj_b 的 'Rust error handling':\n{result}"
        )
        .into());
    }
    println!("PASS (Fix 2): cross-project 注入产出 'Cross-project' + 'Rust error handling'");

    // === Fix 1 机制: add_entry 对 (project_hash, content[:200]) 去重 ===
    let hash = "h";

    // 复现 CI 失败根因：3 条同 content + 同 project_hash → 去重只留 1 条
    let tmp2 = tempfile::TempDir::new()?;
    let conn2 = db::init_db(&tmp2.path().join("dedup.db"))?;
    for id in ["s1", "r1", "i1"] {
        add_entry(&conn2, &make_entry(id, hash, &format!("{id}-title"), "内容"))?;
    }
    let same_count = get_entries_for_project(&conn2, hash)?.len();
    if same_count != 1 {
        return Err(format!(
            "FAIL (Fix 1 前置): 同 content 应去重到 1 条（证明 CI 失败根因），实际 {same_count}"
        )
        .into());
    }

    // 修复后：distinct content → 全留 3 条
    let tmp3 = tempfile::TempDir::new()?;
    let conn3 = db::init_db(&tmp3.path().join("distinct.db"))?;
    for id in ["s1", "r1", "i1"] {
        add_entry(
            &conn3,
            &make_entry(id, hash, &format!("{id}-title"), &format!("{id}-content")),
        )?;
    }
    let distinct_count = get_entries_for_project(&conn3, hash)?.len();
    if distinct_count != 3 {
        return Err(format!(
            "FAIL (Fix 1): distinct content 应全留 3 条，实际 {distinct_count}"
        )
        .into());
    }
    println!(
        "PASS (Fix 1 机制): 同 content 去重→{same_count} 条；distinct content→{distinct_count} 条全留"
    );

    println!("\nCI 修复验证全部通过 ✅");
    Ok(())
}
