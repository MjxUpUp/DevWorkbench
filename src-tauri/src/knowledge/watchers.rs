use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use notify::Watcher;

/// Get the Claude Code project data directory for a given project path.
pub fn claude_project_dir(project_path: &str) -> Option<PathBuf> {
    let home = crate::commands::projects::dirs_home();
    let hash = sha256_hex(project_path);
    let dir = home.join(".claude").join("projects").join(hash);
    if dir.exists() {
        Some(dir)
    } else {
        None
    }
}

/// Start background file watchers for knowledge sources.
///
/// Monitors:
/// - `~/.claude/projects/` — Claude Code JSONL session files
/// - `~/.codex/` — Codex SQLite databases
///
/// Returns a `WatcherGuard` whose drop stops the watchers.
pub fn start_knowledge_watchers(
    db: crate::db::DbState,
) -> Result<WatcherGuard, String> {
    let home = crate::commands::projects::dirs_home();

    // Configure debounce: collect events over 2s before processing
    let (tx, rx) = mpsc::channel();

    // Create watcher that sends events to our channel
    let tx_clone = tx.clone();
    let mut channel_watcher = notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
        if let Ok(event) = res {
            let _ = tx_clone.send(event);
        }
    })
    .map_err(|e| format!("创建 channel watcher 失败: {}", e))?;

    // Watch Claude Code projects directory
    let claude_dir = home.join(".claude").join("projects");
    if claude_dir.exists() {
        channel_watcher
            .watch(&claude_dir, notify::RecursiveMode::Recursive)
            .map_err(|e| format!("监听 {} 失败: {}", claude_dir.display(), e))?;
        log::info!("Knowledge watcher: 监听 {}", claude_dir.display());
    }

    // Watch Codex directory
    let codex_dir = home.join(".codex");
    if codex_dir.exists() {
        channel_watcher
            .watch(&codex_dir, notify::RecursiveMode::Recursive)
            .map_err(|e| format!("监听 {} 失败: {}", codex_dir.display(), e))?;
        log::info!("Knowledge watcher: 监听 {}", codex_dir.display());
    }

    // Spawn background thread to process debounced events
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    let claude_dir_clone = claude_dir.clone();
    let codex_dir_clone = codex_dir.clone();

    let handle = std::thread::Builder::new()
        .name("knowledge-watcher".into())
        .spawn(move || {
            let mut pending_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

            loop {
                if cancel_clone.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }

                // Collect events with a 2-second debounce window
                match rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(event) => {
                        // Only care about data-modifying events
                        match event.kind {
                            notify::EventKind::Create(_)
                            | notify::EventKind::Modify(_)
                            | notify::EventKind::Any => {
                                for path in event.paths {
                                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                                    if ext == "jsonl" || ext == "sqlite" || ext == "json" {
                                        pending_paths.insert(path);
                                    }
                                }
                            }
                            _ => {}
                        }

                        // Drain remaining events in this debounce window
                        while let Ok(event) = rx.try_recv() {
                            match event.kind {
                                notify::EventKind::Create(_)
                                | notify::EventKind::Modify(_)
                                | notify::EventKind::Any => {
                                    for path in event.paths {
                                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                                        if ext == "jsonl" || ext == "sqlite" || ext == "json" {
                                            pending_paths.insert(path);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Debounce window expired — process collected paths
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }

                if pending_paths.is_empty() {
                    continue;
                }

                // Process pending file changes
                let paths_to_process: Vec<PathBuf> = pending_paths.drain().collect();
                for path in &paths_to_process {
                    if let Ok(conn) = db.get() {
                        process_file_change(&conn, path, &claude_dir_clone, &codex_dir_clone);
                    }
                    // If lock fails (e.g., main thread holding it), skip this cycle
                }
            }

            log::info!("Knowledge watcher thread stopped");
        })
        .map_err(|e| format!("启动 watcher 线程失败: {}", e))?;

    Ok(WatcherGuard {
        _watcher: channel_watcher,
        cancel,
        _handle: handle,
    })
}

/// Process a single file change detected by the watcher.
fn process_file_change(
    conn: &rusqlite::Connection,
    path: &std::path::Path,
    claude_dir: &std::path::Path,
    codex_dir: &std::path::Path,
) {
    // Determine which source this file belongs to
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext == "jsonl" && path.starts_with(claude_dir) {
        // Skip large JSONL files to avoid blocking the watcher thread with
        // multi-megabyte reads on every modify event during active sessions.
        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if file_size > super::collector::MAX_JSONL_FILE_SIZE {
            return;
        }
        // Claude Code JSONL — extract project hash from directory name
        // Path structure: ~/.claude/projects/{64-char-sha256}/{file}.jsonl
        // activity::hash_project_path truncates to 16 chars, so we must match.
        if let Some(parent) = path.parent() {
            if let Some(hash_dir) = parent.file_name().and_then(|n| n.to_str()) {
                let truncated_hash = &hash_dir[..16.min(hash_dir.len())];
                if let Ok(entries) = super::collector::parse_claude_jsonl_by_hash(
                    truncated_hash,
                    path,
                ) {
                    let mut count = 0;
                    for entry in &entries {
                        if super::store::add_entry(conn, entry).is_ok() {
                            count += 1;
                        }
                    }
                    if count > 0 {
                        log::info!("Knowledge watcher: 从 {} 采集 {} 条知识", path.display(), count);
                    }
                }
            }
        }
    } else if (ext == "sqlite" || path.file_name().and_then(|n| n.to_str()) == Some("state_5.sqlite"))
        && path.starts_with(codex_dir)
    {
        // Codex state/memory DB changed — re-collect for all known projects
        // We can't easily map back to a specific project, so scan known projects
        log::info!("Knowledge watcher: Codex 数据变更，跳过自动采集（由 session 触发采集覆盖）");
    }
}

/// Guard that stops watchers when dropped.
pub struct WatcherGuard {
    _watcher: notify::RecommendedWatcher,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _handle: std::thread::JoinHandle<()>,
}

impl Drop for WatcherGuard {
    fn drop(&mut self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_hex() {
        let h = sha256_hex("/test/path");
        assert!(!h.is_empty());
        assert_eq!(h.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_claude_project_dir_nonexistent() {
        // A path that definitely doesn't exist
        assert!(claude_project_dir("/__nonexistent_project_path__test__").is_none());
    }
}
