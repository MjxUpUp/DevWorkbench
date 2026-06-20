//! C1 ACP server binary — serves the Dev Workbench kernel as an ACP-servable
//! agent over stdio, so an IDE (Zed) or another agent can drive it the same way
//! it drives Claude Code / codex.
//!
//! Wire it as an ACP agent launch command, e.g.:
//!   `dev-workbench-acp`
//! The server then speaks `initialize` / `session/new` / `session/prompt` on
//! stdin/stdout. All diagnostics go to stderr — stdout is the protocol.
//!
//! Logging is intentionally NOT initialized here: stdout is reserved for the
//! JSON-RPC protocol, and a headless server has no UI to surface logs to. The
//! `log::warn!` / `log::error!` calls inside [`serve_stdio`] are best-effort
//! and simply no-op without a logger installed (acceptable for an MVP server
//! the parent process supervises).

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    app_lib::acp::server::serve_stdio()
        .await
        .map_err(|e| format!("dev-workbench-acp server exited: {e}"))?;
    Ok(())
}
