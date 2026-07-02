//! B7 trajectory evaluation — score an agent's tool-call trajectory against an
//! optional reference, persist the result, and expose a daily regression curve.
//! Mirrors the OpenAI Agents SDK `samples/python/06-evaluate/
//! trajectory-evaluation` rubric (optimal / suboptimal / incorrect).
//!
//! - [`scoring`] — pure 3-matcher (exact_match / in_order / any_order) ×
//!   3-grade rubric, decoupled from any wire format so it is exhaustively
//!   unit-testable.
//! - [`extract`] — rebuild the trajectory (ordered tool-call sequence) from
//!   persisted LLM traces, handling both Anthropic and OpenAI response shapes.
//! - [`db`] — the `eval_runs` table: one row per scored session, plus a
//!   daily-bucketed trend query for the regression curve.

pub mod cases;
pub mod db;
pub mod extract;
pub mod paired;
pub mod replay;
pub mod scoring;
pub mod verdicts;
