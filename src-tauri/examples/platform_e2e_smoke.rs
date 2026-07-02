//! In-process smoke for the platform-e2e data-plane driver (`run_platform_e2e`)
//! — verifies it returns a clean PASS verdict against a known-good seed/expect,
//! WITHOUT needing the Tauri app or a live LLM. This is the verification the
//! in-crate `#[cfg(test)]` suite can't provide on Windows: those tests live in
//! the dev-workbench (app_lib) crate whose test exe hits the pre-existing
//! 0xc0000139 GUI-DLL load failure. An example binary that touches ONLY the
//! rusqlite + eval-math path (no `tauri::AppHandle`, unlike `replay_live`)
//! links no GUI DLLs and loads clean — same reason `openai_live` runs.
//!
//! This specifically guards the schema bootstrap: `run_platform_e2e` opens a
//! bare in-memory connection and must pre-create the `schema_version` table
//! before the v20→v21 / v21→v22 migrations run (otherwise their trailing
//! `INSERT INTO schema_version` fails and the driver returns Err on every call
//! — a real bug a code review caught; mac/linux CI would have surfaced it via
//! the 4 unit tests, Windows can't run them).
//!
//! ```sh
//! cargo run --example platform_e2e_smoke
//! ```
//! Exit 0 = run_platform_e2e returned a PASS verdict with all 4 checks green.

use std::error::Error;

use app_lib::eval::platform_e2e::{
    run_platform_e2e, E2EExpect, E2EReplayExpect, E2ESeed, E2ESeedCase, E2ESeedVerdict,
};
use app_lib::eval::scoring::Matcher;
use app_lib::eval::scoring::Grade::Optimal;

fn main() -> Result<(), Box<dyn Error>> {
    // 2 approved cases + 1 draft; 1 eval-gate verdict; replay [read, edit] on
    // c1 → Optimal. The driver should report pass=true with 4 checks.
    let seed = E2ESeed {
        cases: vec![
            E2ESeedCase {
                id: "c1".into(),
                name: "demo".into(),
                category: "agent".into(),
                input_prompt: "do".into(),
                expected_steps_json: Some(r#"[{"name":"read"},{"name":"edit"}]"#.into()),
                negative_json: Some(r#"[{"name":"bash"}]"#.into()),
                draft: false,
            },
            E2ESeedCase {
                id: "c2".into(),
                name: "demo2".into(),
                category: "agent".into(),
                input_prompt: "do".into(),
                expected_steps_json: None,
                negative_json: None,
                draft: false,
            },
            E2ESeedCase {
                id: "c3".into(),
                name: "draft".into(),
                category: "agent".into(),
                input_prompt: "do".into(),
                expected_steps_json: None,
                negative_json: None,
                draft: true,
            },
        ],
        verdicts: vec![E2ESeedVerdict {
            gate: "eval".into(),
            verdict: "PASS".into(),
            session_id: None,
            case_id: Some("c1".into()),
        }],
    };
    let expect = E2EExpect {
        approved_case_count: Some(2),
        total_case_count: Some(3),
        verdict_count_for_gate: Some(("eval".into(), 1)),
        replay: Some(E2EReplayExpect {
            case_id: "c1".into(),
            actual_steps: vec!["read".into(), "edit".into()],
            matcher: Matcher::ExactMatch,
            expected_grade: Optimal,
        }),
    };

    let v = run_platform_e2e(seed, expect).map_err(|e| {
        // The BLOCKING bug surfaced here as: "verdicts schema: no such table:
        // schema_version". If this fires again, the schema_version pre-create
        // in run_platform_e2e regressed.
        format!("run_platform_e2e returned Err: {e}")
    })?;

    println!("pass       = {}", v.pass);
    println!("checks ({}):", v.checks.len());
    for c in &v.checks {
        println!("  {} {} — {}", if c.pass { "✓" } else { "✗" }, c.name, c.detail);
    }
    if !v.mismatches.is_empty() {
        println!("mismatches: {:?}", v.mismatches);
    }

    assert!(v.pass, "expected a clean PASS verdict, got mismatches: {:?}", v.mismatches);
    assert_eq!(v.checks.len(), 4, "expected 4 checks (approved/total/verdict/replay)");
    assert!(v.checks.iter().all(|c| c.pass), "all checks must pass");

    println!("\nALL OK — run_platform_e2e data plane verified: schema bootstrap + seed + real logic assertions all green.");
    Ok(())
}
