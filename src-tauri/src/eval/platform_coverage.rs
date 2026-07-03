//! Platform-coverage eval (P4·调整): the 反刷分 self-audit — does dev workbench
//! practice what it preaches? The eval system judges agents on objective facts;
//! this turns the same lens on dev workbench ITSELF: are the IPC commands the
//! frontend invokes actually registered on the backend? A frontend `invoke('x')`
//! with no matching `generate_handler!` entry is a DEAD BUTTON — the user clicks,
//! nothing happens, the UI lies. That is exactly the kind of fake the anti-gaming
//! core exists to catch, so the audit system must audit its own host.
//!
//! Two independently-grepped sets, compared:
//!  - **F** (frontend): every `invoke('x')` / `invoke<T>('x')` in `src/`, harvested
//!    at build time by `scripts/gen-invoked-commands.mjs` → `src/generated/
//!    invoked-commands.ts`, passed in here as `frontend_invokes`.
//!  - **B** (backend): every command in `tauri::generate_handler![…]` in `lib.rs`,
//!    embedded at compile time via `include_str!`.
//! Neither set is hand-curated — both are objective facts about the source — so
//! neither side can self-declare compliance (反刷分 #1: 客观事实代码判). The diff is
//! the verdict:
//!  - `F \ B` = **dead buttons** (frontend invokes, backend never registered) → FAIL.
//!  - `B \ F` = **dead code** (backend registered, frontend never calls) → WARN
//!    (not fail; may be an event callback, internal helper, or genuinely unwired).
//!
//! No LLM, no browser: a static, deterministic fact. The "可点可用" dimension is
//! guarded by the playwright `*.spec.ts` suite, not duplicated here (same boundary
//! `platform_e2e` draws).
//!
//! `input_prompt` / case-table invariants do NOT apply — there is no agent, no
//! case row, no replay. The "case" is the source itself.

use std::collections::BTreeSet;

use serde::Serialize;

/// The backend entry source, embedded at compile time. `include_str!` resolves
/// relative to this file (`src-tauri/src/eval/`), so `../lib.rs` is the app's
/// `generate_handler!` site. If lib.rs ever moves, this path breaks at compile
/// time — preferable to a silent runtime miss.
const BACKEND_LIB_SRC: &str = include_str!("../lib.rs");

/// One audit check's outcome — surfaced so the UI shows which dimension
/// passed/failed (not just an opaque pass bool). Mirrors `platform_e2e::E2ECheck`.
#[derive(Debug, Clone, Serialize)]
pub struct CoverageCheck {
    pub name: String,
    pub pass: bool,
    pub detail: String,
}

/// The self-audit verdict. `pass` requires ZERO dead buttons (frontend↔backend
/// misalignment that would produce a dead button). Dead code is reported but
/// does not fail — it is waste, not a lie.
#[derive(Debug, Clone, Serialize)]
pub struct CoverageVerdict {
    pub pass: bool,
    pub frontend_count: usize,
    pub backend_count: usize,
    pub aligned_count: usize,
    /// `F \ B` — frontend invokes these but backend never registered them. Each
    /// is a dead button (FAIL): the UI offers an action that does nothing.
    pub dead_buttons: Vec<String>,
    /// `B \ F` — backend registered these but no frontend code calls them. WARN
    /// only: could be an event-driven command, internal helper, or genuinely
    /// unwired (a real dead-code finding worth a human look, but not a lie).
    pub dead_code: Vec<String>,
    pub checks: Vec<CoverageCheck>,
}

/// Extract the backend's registered command names from `generate_handler![…]` in
/// lib.rs. Each entry is `path::to::cmd_name` (or bare `cmd_name`); the Tauri
/// command name IS the final `::` segment (default naming, no rename in this
/// codebase — every entry in lib.rs:300-413 is a plain `commands::…::name`).
/// Returned sorted + deduped.
///
/// Pure + deterministic: the same lib.rs source always yields the same set. If
/// `generate_handler!` is absent or malformed, returns empty (the verdict then
/// reports backend_count=0, which fails loudly — a missing handler registry is
/// itself a catastrophic finding, never silently green).
pub fn extract_registered_commands() -> Vec<String> {
    extract_registered_commands_in(BACKEND_LIB_SRC)
}

/// Parameterized core of [`extract_registered_commands`] — takes the lib.rs
/// source as an argument so a test can feed an empty / malformed registry and
/// exercise the "backend parses to empty → fail loudly" guard (the const
/// `include_str!` is never empty in production, so the delegating wrapper alone
/// could never reach that branch). Pure + deterministic: same src ⇒ same set.
pub fn extract_registered_commands_in(lib_src: &str) -> Vec<String> {
    let key = "generate_handler![";
    let mut out: Vec<String> = Vec::new();
    let Some(start) = lib_src.find(key) else {
        return out; // verdict will flag backend_count=0.
    };
    let body_start = start + key.len();
    let rest = &lib_src[body_start..];
    // generate_handler! has no nested `[…]` in this codebase, so the first `]`
    // closes the macro. Bracket-depth counting would be more general but offers
    // no extra safety today (verified: the macro body has no `]`); instead the
    // tripwire test asserts the parsed count is ≥100, so any premature truncation
    // that drops the set into the tens trips before a verdict can silently shrink.
    let body = match rest.find(']') {
        Some(end) => &rest[..end],
        None => rest,
    };
    for raw in body.split(',') {
        let tok = raw.trim();
        if tok.is_empty() {
            continue;
        }
        // `commands::tools::detect_tools` → `detect_tools`; bare `cmd` → `cmd`.
        let name = tok.rsplit("::").next().unwrap_or(tok).trim();
        // Strip any trailing line-comment / whitespace artifact.
        let name = name.split_whitespace().next().unwrap_or(name);
        if !name.is_empty() {
            out.push(name.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Run the IPC-coverage self-audit: compare the frontend's invoked-command set
/// (F, passed in from the build-time manifest) against the backend's registered
/// set (B, parsed from lib.rs). Returns the per-dimension verdict. Pure — no IO,
/// no LLM, no DB — so the result is a deterministic fact about the two source
/// trees (反刷分 #1).
pub fn run_platform_coverage(frontend_invokes: &[String]) -> CoverageVerdict {
    coverage_verdict(frontend_invokes, &extract_registered_commands())
}

/// Parameterized core of [`run_platform_coverage`] — takes the backend command
/// set as an argument so a test can feed an empty backend and prove the
/// "missing registry ⇒ fail loudly" guard actually fires (the production path
/// embeds lib.rs via `include_str!`, which is never empty, so the guard's
/// `!backend.is_empty()` half could never be exercised through the wrapper).
/// Pure — no IO, no LLM, no DB — so the result is a deterministic fact about
/// the two sets (反刷分 #1).
pub fn coverage_verdict(frontend_invokes: &[String], backend: &[String]) -> CoverageVerdict {
    let f: BTreeSet<&String> = frontend_invokes.iter().collect();
    let b: BTreeSet<&String> = backend.iter().collect();

    let dead_buttons: Vec<String> = f
        .difference(&b)
        .map(|s| (*s).clone())
        .collect();
    let dead_code: Vec<String> = b
        .difference(&f)
        .map(|s| (*s).clone())
        .collect();
    let aligned_count = f.intersection(&b).count();

    let mut checks: Vec<CoverageCheck> = Vec::new();
    // Each dead button is its own FAIL check — the UI lists exactly which
    // commands are dead buttons, not just a count.
    for cmd in &dead_buttons {
        checks.push(CoverageCheck {
            name: format!("dead_button:{cmd}"),
            pass: false,
            detail: "前端 invoke 但后端 generate_handler! 未注册（死按钮）".into(),
        });
    }
    checks.push(CoverageCheck {
        name: "frontend_invoke_count".into(),
        pass: true,
        detail: format!("{} commands", f.len()),
    });
    checks.push(CoverageCheck {
        name: "backend_register_count".into(),
        pass: !backend.is_empty(),
        detail: if backend.is_empty() {
            "未解析到 generate_handler!（注册表缺失——灾难性）".into()
        } else {
            format!("{} commands", b.len())
        },
    });
    checks.push(CoverageCheck {
        name: "aligned".into(),
        pass: true,
        detail: format!("{aligned_count} commands 前后端对齐"),
    });
    if !dead_code.is_empty() {
        // Dead code is informational (WARN), surfaced as a passing check so it
        // shows in the report without failing the verdict.
        checks.push(CoverageCheck {
            name: "dead_code (WARN)".into(),
            pass: true,
            detail: format!("{} commands 后端注册但前端未调用（事件/内部/未接线）", dead_code.len()),
        });
    }

    CoverageVerdict {
        // Dead buttons fail; dead code does not. An empty backend registry also
        // fails (parse tripwire) — guarded by the backend_register_count check.
        pass: dead_buttons.is_empty() && !backend.is_empty(),
        frontend_count: f.len(),
        backend_count: b.len(),
        aligned_count,
        dead_buttons,
        dead_code,
        checks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// lib.rs registers >100 commands today; a non-zero parse proves the
    /// `include_str!` + macro-parse path still resolves. If lib.rs moves or the
    /// handler macro changes shape, this trips before any verdict can silently
    /// report backend_count=0. The ≥100 threshold (not just non-empty) catches a
    /// premature `]` truncation that would shrink the parsed set into the tens.
    #[test]
    fn backend_registry_parses_nonempty() {
        let b = extract_registered_commands();
        assert!(
            b.len() >= 100,
            "generate_handler! must parse to ≥100 commands, got {} — macro shape may have changed/truncated",
            b.len()
        );
        // A few commands known to be in lib.rs:300-413 — anchors that survive
        // refactors and prove names (not just counts) parse correctly.
        for anchor in ["spawn_agent_session", "load_projects", "list_eval_cases", "eval_platform_e2e"] {
            assert!(
                b.iter().any(|c| c == anchor),
                "anchor command {anchor:?} missing from parsed backend set"
            );
        }
    }

    /// The `extract_registered_commands_in` core parses an empty / handler-less
    /// source to an empty set — the precondition the empty-backend guard below
    /// relies on. (The const `include_str!` wrapper is never empty in production,
    /// so this branch can only be hit through the parameterized core.)
    #[test]
    fn extract_empty_when_no_handler_macro() {
        let b = extract_registered_commands_in("fn main() {} // no generate_handler! here");
        assert!(b.is_empty(), "a source with no generate_handler! must parse to empty");
    }

    #[test]
    fn subset_frontend_passes_with_no_dead_buttons() {
        // Frontend set fully contained in backend → no dead buttons → pass.
        let b = extract_registered_commands();
        let f: Vec<String> = b.iter().take(10).cloned().collect();
        let v = run_platform_coverage(&f);
        assert!(v.pass, "subset should pass: {:?}", v.dead_buttons);
        assert!(v.dead_buttons.is_empty());
        assert_eq!(v.aligned_count, 10);
        // dead_code = backend minus the 10 we kept.
        assert_eq!(v.dead_code.len(), b.len() - 10);
    }

    #[test]
    fn unknown_frontend_command_is_a_dead_button_fail() {
        // A command the frontend invokes but the backend never registers is a
        // dead button — the exact lie this audit exists to catch.
        let mut f = extract_registered_commands();
        f.push("definitely_not_registered_xyz".into());
        let v = run_platform_coverage(&f);
        assert!(!v.pass, "dead button must fail the verdict");
        assert!(v.dead_buttons.iter().any(|c| c == "definitely_not_registered_xyz"));
        // The dead button is surfaced as its own named FAIL check.
        assert!(v
            .checks
            .iter()
            .any(|c| c.name == "dead_button:definitely_not_registered_xyz" && !c.pass));
    }

    #[test]
    fn empty_backend_registry_fails_loudly() {
        // If generate_handler! ever disappears / becomes unparseable, backend
        // parses to empty → pass MUST be false — never silently green on a
        // missing registry. This feeds an empty backend set directly through the
        // parameterized core (the production wrapper embeds lib.rs via
        // include_str!, which is never empty, so this guard could not otherwise
        // be reached). `pass = dead_buttons.is_empty() && !backend.is_empty()`
        // — the !backend.is_empty() half is the guard under test here.
        let empty_backend: Vec<String> =
            extract_registered_commands_in("fn main() {} // no generate_handler!");
        assert!(empty_backend.is_empty());
        let v = coverage_verdict(&["some_cmd".to_string()], &empty_backend);
        assert!(!v.pass, "empty backend registry must FAIL, never silently green");
        assert_eq!(v.backend_count, 0);
        // The backend_register_count check is the explicit, named FAIL signal.
        assert!(v
            .checks
            .iter()
            .any(|c| c.name == "backend_register_count" && !c.pass));
    }
}
