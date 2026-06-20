//! LLM call tracing — persists every GlmChatModel HTTP request/response to the
//! `llm_traces` table for post-hoc debugging. This is the observability layer
//! that was missing: a failed session used to leave only
//! `stream ENDED status=Failed` in the log — no request body, no HTTP status,
//! no error body. With this, the real request/response of every LLM call is on
//! disk, queryable per session. See `sink` for the trait + DB writer and `db`
//! for the row helper.

pub mod db;
pub mod sink;
pub mod timing;

pub use sink::{optional_shared, redact_secrets, truncate, DbTraceSink, LlmTrace, NullTraceSink, TraceSink};
pub use timing::{TimingChecker, TimingWarning};
