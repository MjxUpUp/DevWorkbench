//! Live end-to-end probe of the anti-gaming replay driver (`run_replay`) — the
//! one code path the in-crate `#[cfg(test)]` suite CANNOT exercise: it spins up
//! a real Plan-mode ReactAgent against a live LLM, lets it run one turn, then
//! rebuilds the trajectory from the session's persisted LLM traces and scores
//! it against a frozen eval-case contract (反刷分三原则 #1 客观事实 / #2 因果归因 /
//! #3 配对回放).
//!
//! # Platform limitation (Windows 0xc0000139)
//! Unlike `openai_live` (which touches only the model layer), THIS example
//! calls `run_replay` → `build_react_agent`, whose signature carries
//! `tauri::AppHandle`. The resulting exe links Tauri's GUI native stack
//! (comctl32 / gdi32 / dwmapi / shell32 / oleaut32) and so hits the SAME
//! pre-existing `STATUS_ENTRYPOINT_NOT_FOUND (0xc0000139)` load failure on
//! Windows that the `app_lib` test binary does (see memory:
//! applib-test-binary-entrypoint-block — root-caused to Tauri coupling, not a
//! code defect; the fix is a Tauri mock runtime this project does not use).
//! Confirmed empirically: `openai_live.exe` loads clean, `replay_live.exe`
//! 0xc0000139's — the only difference is the Tauri link via run_replay.
//!
//! So: run this on **macOS / Linux** (CI), where the GUI DLLs are absent and
//! the exe links fine. The deterministic anti-gaming core it drives
//! (`score_replay` / `compare_paired` / `extract_trajectory`) IS unit-tested
//! in-process; this example is the live-wiring check for the `run_replay`
//! driver itself, mirroring how `build_react_agent`'s live smoke runs on CI.
//!
//! # Provider + isolation
//! The agent's provider comes from the user's `~/.dev-workbench/providers.toml`
//! (configured via the in-app provider settings — the SAME path build_react_agent
//! reads at runtime). No key is read or persisted by this example; it reuses the
//! already-configured provider. The DB is a throwaway tempfile (fresh schema via
//! `DbState::open`'s `CREATE TABLE IF NOT EXISTS` — no migrations needed on a
//! fresh DB) deleted on exit, so the run leaves NOTHING in the user's real
//! `data.db`: no seeded case, no trace, no verdict.
//!
//! ```sh
//! # mac/linux: minimal — default read-only prompt against a repo
//! DW_REPLAY_WORKING_DIR=/some/repo cargo run --example replay_live --release
//! # custom prompt + a frozen step contract + a negative (forbidden) tool
//! DW_REPLAY_WORKING_DIR=/some/repo \
//!   DW_REPLAY_PROMPT="列出 src 下所有 .ts 文件" \
//!   DW_REPLAY_EXPECTED_STEPS='[{"name":"Glob"},{"name":"Read"}]' \
//!   DW_REPLAY_NEGATIVE='[{"name":"Bash"}]' \
//!   cargo run --example replay_live --release
//! ```
//!
//! Exit 0 = the agent finished a turn, its trajectory was rebuilt from real
//! response bodies, scored against the contract, and produced a verdict. The
//! printed lines name score / grade / verdict / attribution / negative-hit /
//! the actual tool-call sequence — so a human can see whether the agent's
//! real behavior matched the frozen contract (the whole point of 反刷分).

use std::error::Error;

use app_lib::db::DbState;
use app_lib::eval::cases::{insert_eval_case, EvalCaseRow, NewEvalCase};
use app_lib::eval::replay::{run_replay, ReplayInput, ReplayVerdict};
use app_lib::eval::scoring::Matcher;

const DEFAULT_PROMPT: &str = "Read this repo's README and summarize it in one sentence.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let working_dir = std::env::var("DW_REPLAY_WORKING_DIR").map_err(|_| {
        "set DW_REPLAY_WORKING_DIR to the repo the agent should explore (read-only Plan sandbox)"
    })?;
    if !std::path::Path::new(&working_dir).exists() {
        return Err(format!("DW_REPLAY_WORKING_DIR does not exist: {working_dir}").into());
    }
    let prompt = std::env::var("DW_REPLAY_PROMPT").unwrap_or_else(|_| DEFAULT_PROMPT.to_string());
    let model = std::env::var("DW_REPLAY_MODEL").ok(); // None = user's configured default
    let expected_steps = std::env::var("DW_REPLAY_EXPECTED_STEPS").ok();
    let negative = std::env::var("DW_REPLAY_NEGATIVE").ok();
    // matcher default = exact_match (the strictest — proves a clean trajectory).
    let matcher = match std::env::var("DW_REPLAY_MATCHER").as_deref() {
        Ok("in_order") => Matcher::InOrder,
        Ok("any_order") => Matcher::AnyOrder,
        _ => Matcher::ExactMatch,
    };

    // Honest precondition: build_react_agent resolves the provider from the
    // user's providers.toml. If none is configured, run_replay will still build
    // an agent but it will fail at the first LLM call (empty key) — surface the
    // cause up front instead of a cryptic HTTP 401 mid-run.
    let data_dir = app_lib::commands::projects::dirs_home().join(".dev-workbench");
    let providers_toml = data_dir.join("providers.toml");
    if !providers_toml.exists() || std::fs::read_to_string(&providers_toml)?.trim().is_empty() {
        return Err(format!(
            "no provider configured: write ~/.dev-workbench/providers.toml via the in-app \
             provider settings (or set up a provider there) before running this. {} not found / empty",
            providers_toml.display()
        )
        .into());
    }

    println!("working_dir = {working_dir}");
    println!("model       = {}", model.clone().unwrap_or_else(|| "__default__".into()));
    println!("matcher     = {matcher:?}");
    println!("prompt      = {prompt:?}");
    println!("expected    = {expected_steps:?}");
    println!("negative    = {negative:?}");

    // Throwaway DB: fresh schema from DbState::open (CREATE TABLE IF NOT EXISTS
    // covers llm_traces / verdicts / eval_cases / trace_settings on a fresh DB —
    // no v6→v22 migrations needed). tempdir drops + deletes the file on exit.
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("replay-live.db");
    let db = DbState::open(&db_path)?;

    // Seed one reviewed (non-draft) case carrying the frozen contract. The
    // contract is the anti-gaming anchor: the agent can't rewrite it after the
    // fact to cover its tracks (C1 — input_prompt is also locked to the case).
    let case_id = "replay-live-smoke".to_string();
    let case = NewEvalCase {
        id: case_id.clone(),
        name: "replay_live smoke".into(),
        category: "agent".into(),
        input_prompt: prompt.clone(),
        expected_steps_json: expected_steps.clone(),
        expected_output: None,
        expected_observables_json: None,
        negative_json: negative.clone(),
        source_session_id: None,
        commit_sha: None,
        draft: false,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    {
        let conn = db.get()?;
        insert_eval_case(&conn, &case)?;
    }
    let case_row = EvalCaseRow {
        id: case.id,
        name: case.name,
        category: case.category,
        input_prompt: case.input_prompt,
        expected_steps_json: case.expected_steps_json,
        expected_output: case.expected_output,
        expected_observables_json: case.expected_observables_json,
        negative_json: case.negative_json,
        source_session_id: case.source_session_id,
        commit_sha: case.commit_sha,
        draft: case.draft,
        created_at: case.created_at,
    };

    // Drive one replay turn. run_replay builds a Plan-mode agent (read-only
    // sandbox — Bash/Write blocked, Read/Glob/Grep allowed), runs it one turn,
    // rebuilds the trajectory from the session's REAL response bodies, scores
    // it against the frozen contract, and (best-effort) persists an eval
    // verdict — here into the throwaway DB, so nothing leaks.
    let session_id = "replay-live-session".to_string();
    let input = ReplayInput {
        session_id: session_id.clone(),
        working_dir: working_dir.clone(),
        model: model.clone(),
        enable_skills: true, // plain agent replay: skills available (default)
        // The verdict goes into the throwaway DB anyway; pass Some("eval") to
        // exercise the same persist path a real replay run takes.
        verdict_gate: Some("eval".to_string()),
    };
    println!("\ndriving run_replay (live agent, one turn)…");
    let verdict: ReplayVerdict = run_replay(input, &case_row, matcher, &db).await?;

    // Also surface the rebuilt trajectory — the objective fact (反刷分 #1):
    // rebuilt from real traces, not the agent's self-report.
    let actual_steps: Vec<String> = {
        let conn = db.get()?;
        let traces = app_lib::trace::db::list_traces_for_session(&conn, &session_id)?;
        app_lib::eval::extract::extract_trajectory(&traces)
            .iter()
            .map(|s| s.name.clone())
            .collect()
    };

    println!("\n--- ReplayVerdict (客观事实 rebuilt from real traces) ---");
    println!("score            = {}", verdict.score);
    println!("grade            = {:?}", verdict.grade);
    println!("verdict          = {}", verdict.verdict);
    println!("attribution      = {:?}", verdict.attribution);
    println!("negative_violated= {}", verdict.negative_violated);
    println!("actual_steps     = {actual_steps:?}");
    println!("reason           = {}", verdict.reason);

    // The run is considered to have produced a usable verdict as long as the
    // agent finished a turn and the trajectory was rebuilt (>=1 step). Whether
    // it PASS or FAIL depends on the live model's real behavior vs the frozen
    // contract — that is the signal, not a pass/fail gate for this smoke.
    assert!(
        !actual_steps.is_empty(),
        "trajectory rebuilt empty — the agent made no tool calls (check the model/provider)"
    );

    println!("\nALL OK — run_replay verified end-to-end: live agent → rebuilt trajectory → scored vs frozen contract → verdict.");
    Ok(())
}
