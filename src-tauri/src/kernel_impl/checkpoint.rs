//! Shadow-git checkpoint — snapshot the working tree before a ReactAgent turn
//! so the user can roll the agent's changes back. Mirrors Forge's file-sentinel
//! philosophy (preserve, never destroy) but flips it: instead of quarantining
//! unauthorized writes, we capture a git checkpoint at session start and restore
//! it on demand. This covers Claude Code's blind spot (no native checkpoint).
//!
//! Mechanism: `git stash create -u` freezes tracked changes + untracked files
//! into a dangling commit WITHOUT touching any ref (unlike `git stash`, which
//! moves refs/stash). We persist head_sha + the file lists; rollback restores
//! tracked files via `git checkout -- <f>` and deletes agent-created untracked
//! files — never a blanket `git reset --hard`.
//!
//! Storage: `<project>/.forge/checkpoints/<session_id>.json` (in-project, so it
//! travels with the repo; under the Forge self-protected `.forge/` root, so the
//! agent itself can't delete its own checkpoint via a Bash write).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub session_id: String,
    pub project_path: String,
    pub created_at: String,
    /// `git rev-parse HEAD` at checkpoint time. Rollback refuses unless this
    /// still matches HEAD (the user hasn't committed/checked-out since), unless
    /// `force` is set — otherwise restoring tracked files to the old HEAD would
    /// silently discard the user's newer commits.
    pub head_sha: String,
    /// `git stash create -u` output (dangling commit SHA). None when the tree
    /// was clean. Kept for audit (`git stash show -p <sha>`); rollback does NOT
    /// apply it — applying would re-introduce the very changes we're undoing.
    pub stash_sha: Option<String>,
    /// Untracked files present at checkpoint (`git ls-files --others
    /// --exclude-standard`). Rollback preserves these (they pre-date the agent);
    /// it deletes only untracked files created DURING the session.
    pub untracked_at_checkpoint: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackResult {
    /// Tracked files reset to HEAD (agent modifications discarded).
    pub restored_files: Vec<String>,
    /// Untracked files created during the session and now deleted.
    pub removed_untracked: Vec<String>,
    /// Files rollback could not touch (git/IO error, or already gone).
    pub skipped: Vec<String>,
}

/// Run a git command in `project`, returning trimmed stdout. Windows uses
/// CREATE_NO_WINDOW to avoid console popups (same pattern as honesty.rs /
/// git.rs). Errors carry stderr so callers can log a meaningful reason.
fn git_run(project: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args).current_dir(project);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| format!("git spawn failed: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn checkpoint_path(project: &Path, session_id: &str) -> PathBuf {
    project
        .join(".forge")
        .join("checkpoints")
        .join(format!("{session_id}.json"))
}

/// Lines of git stdout as owned Strings (empty lines dropped).
fn lines_of(s: &str) -> Vec<String> {
    s.lines().filter(|l| !l.is_empty()).map(String::from).collect()
}

/// Capture a checkpoint at session start (before the agent touches anything).
/// Returns Err if git is unavailable or the path isn't a repo — checkpoint is
/// an enhancement, not a gate, so callers log a warning and continue.
pub fn create_at_session_start(
    project: &str,
    session_id: &str,
    reason: &str,
) -> Result<Checkpoint, String> {
    let root = Path::new(project);
    let head_sha = git_run(root, &["rev-parse", "HEAD"])?;
    // `stash create -u` prints a commit SHA, or empty when the tree is clean.
    let stash_out = git_run(root, &["stash", "create", "-u"])?;
    let stash_sha = if stash_out.is_empty() {
        None
    } else {
        Some(stash_out)
    };
    let untracked = git_run(root, &["ls-files", "--others", "--exclude-standard"])?;

    let cp = Checkpoint {
        session_id: session_id.to_string(),
        project_path: project.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        head_sha,
        stash_sha,
        untracked_at_checkpoint: lines_of(&untracked),
        reason: reason.to_string(),
    };

    let dir = checkpoint_path(root, session_id).parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| format!("mkdir checkpoints: {e}"))?;
    let json = serde_json::to_string_pretty(&cp).map_err(|e| format!("serialize: {e}"))?;
    std::fs::write(checkpoint_path(root, session_id), json)
        .map_err(|e| format!("write checkpoint: {e}"))?;
    Ok(cp)
}

/// Read a persisted checkpoint. Ok(None) when none exists (session pre-dates the
/// feature, or git was unavailable at spawn).
pub fn read(project: &str, session_id: &str) -> Result<Option<Checkpoint>, String> {
    let path = checkpoint_path(Path::new(project), session_id);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| format!("read checkpoint: {e}"))?;
    let cp: Checkpoint = serde_json::from_str(&raw).map_err(|e| format!("parse checkpoint: {e}"))?;
    Ok(Some(cp))
}

/// Roll the working tree back to the checkpoint state.
///
/// - Restores every currently-modified tracked file via `git checkout -- <f>`
///   (the head_sha guard ensures HEAD still equals the checkpoint HEAD, so this
///   lands the agent's pre-change content).
/// - Deletes untracked files created DURING the session (present now but not at
///   checkpoint time). Pre-existing untracked files are preserved.
///
/// Safety: refuses unless HEAD matches `checkpoint.head_sha` (the user hasn't
/// committed/checked-out since) unless `force` is set. Before applying, captures
/// a `<session_id>.prerollback` checkpoint so a mistaken rollback is itself
/// reversible — mirrors file-sentinel's NEVER-delete philosophy.
pub fn apply_rollback(
    project: &str,
    session_id: &str,
    force: bool,
) -> Result<RollbackResult, String> {
    let root = Path::new(project);
    let cp = read(project, session_id)?
        .ok_or_else(|| format!("no checkpoint for session {session_id}"))?;

    let current_head = git_run(root, &["rev-parse", "HEAD"])?;
    if current_head != cp.head_sha && !force {
        return Err(format!(
            "HEAD moved since checkpoint (was {}, now {}); pass force=true to roll back anyway \
             (this may discard newer commits)",
            cp.head_sha, current_head
        ));
    }

    // Best-effort pre-rollback snapshot (own session_id so read/rollback work
    // on it directly). A failure here is logged but does not block rollback.
    let pre_id = format!("{session_id}.prerollback");
    if let Err(e) = create_at_session_start(project, &pre_id, "prerollback") {
        log::warn!("[checkpoint] prerollback snapshot for {session_id} failed: {e}");
    }

    let mut restored = Vec::new();
    let mut removed = Vec::new();
    let mut skipped = Vec::new();

    // Restore every currently-modified tracked file to HEAD. (Not just the
    // checkpoint's own diff — the agent may have touched files that were clean
    // at checkpoint time. The head guard guarantees HEAD == checkpoint HEAD.)
    let current_tracked = git_run(root, &["diff", "--name-only", "HEAD"])?;
    for f in lines_of(&current_tracked) {
        match git_run(root, &["checkout", "--", &f]) {
            Ok(_) => restored.push(f),
            Err(e) => {
                log::warn!("[checkpoint] restore tracked {f} failed: {e}");
                skipped.push(f);
            }
        }
    }

    // Delete untracked files created during the session = present now but NOT
    // at checkpoint time. Whitelist of one (the diff vs the checkpoint set), so
    // a pre-existing user untracked file is never touched.
    let before: HashSet<String> = cp.untracked_at_checkpoint.iter().cloned().collect();
    let current_untracked = git_run(root, &["ls-files", "--others", "--exclude-standard"])?;
    for f in lines_of(&current_untracked) {
        if before.contains(&f) {
            continue;
        }
        let p = root.join(&f);
        if p.exists() {
            match std::fs::remove_file(&p) {
                Ok(_) => removed.push(f),
                Err(e) => {
                    log::warn!("[checkpoint] remove untracked {f} failed: {e}");
                    skipped.push(f);
                }
            }
        } else {
            skipped.push(f);
        }
    }

    Ok(RollbackResult {
        restored_files: restored,
        removed_untracked: removed,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bootstrap a git repo with one committed file. Mirrors honesty.rs's
    /// audit_assertion_weakening_fails setup.
    fn git_repo(root: &Path) {
        for args in [
            &["init"][..],
            &["config", "user.email", "t@t.t"][..],
            &["config", "user.name", "t"][..],
        ] {
            git_run(root, args).unwrap();
        }
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git_run(root, &["add", "."]).unwrap();
        git_run(root, &["commit", "-m", "base", "--allow-empty"]).unwrap();
    }

    #[test]
    fn create_captures_head_and_untracked() {
        let tmp = tempfile::TempDir::new().unwrap();
        git_repo(tmp.path());
        std::fs::write(tmp.path().join("preexisting.txt"), "x\n").unwrap(); // untracked

        let cp = create_at_session_start(tmp.path().to_str().unwrap(), "s1", "session_start").unwrap();
        assert!(!cp.head_sha.is_empty());
        assert!(
            cp.untracked_at_checkpoint.contains(&"preexisting.txt".to_string()),
            "got: {:?}",
            cp.untracked_at_checkpoint
        );
        // Persisted to disk.
        assert!(read(tmp.path().to_str().unwrap(), "s1").unwrap().is_some());
    }

    #[test]
    fn create_with_clean_tree_has_no_stash() {
        let tmp = tempfile::TempDir::new().unwrap();
        git_repo(tmp.path());
        let cp = create_at_session_start(tmp.path().to_str().unwrap(), "s2", "test").unwrap();
        assert!(cp.stash_sha.is_none(), "clean tree → no stash");
        assert!(cp.untracked_at_checkpoint.is_empty());
    }

    #[test]
    fn rollback_restores_tracked_and_removes_agent_untracked() {
        let tmp = tempfile::TempDir::new().unwrap();
        git_repo(tmp.path());
        // Checkpoint a clean state.
        create_at_session_start(tmp.path().to_str().unwrap(), "s3", "test").unwrap();
        // Simulate the agent's work: modify the tracked file + create a new one.
        std::fs::write(tmp.path().join("base.txt"), "agent-changed\n").unwrap();
        std::fs::write(tmp.path().join("agent_new.txt"), "agent\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("base.txt")).unwrap(),
            "agent-changed\n"
        );

        let res = apply_rollback(tmp.path().to_str().unwrap(), "s3", false).unwrap();
        assert!(res.restored_files.contains(&"base.txt".to_string()), "got: {res:?}");
        assert!(
            res.removed_untracked.contains(&"agent_new.txt".to_string()),
            "got: {res:?}"
        );
        // Tracked file restored to HEAD content. Normalize CRLF: Windows git
        // checkout writes CRLF (core.autocrlf), but the restore is correct.
        let restored = std::fs::read_to_string(tmp.path().join("base.txt")).unwrap();
        assert_eq!(restored.replace("\r\n", "\n"), "base\n");
        // Agent-created untracked file gone.
        assert!(!tmp.path().join("agent_new.txt").exists());
    }

    #[test]
    fn rollback_preserves_preexisting_untracked() {
        let tmp = tempfile::TempDir::new().unwrap();
        git_repo(tmp.path());
        std::fs::write(tmp.path().join("user_notes.txt"), "mine\n").unwrap();
        create_at_session_start(tmp.path().to_str().unwrap(), "s3b", "test").unwrap();
        // Agent adds its own untracked file alongside the user's.
        std::fs::write(tmp.path().join("agent_gen.txt"), "agent\n").unwrap();

        let res = apply_rollback(tmp.path().to_str().unwrap(), "s3b", false).unwrap();
        assert!(res.removed_untracked.contains(&"agent_gen.txt".to_string()));
        // The user's pre-existing untracked file survives.
        assert!(tmp.path().join("user_notes.txt").exists());
    }

    #[test]
    fn rollback_refuses_when_head_moved_unless_forced() {
        let tmp = tempfile::TempDir::new().unwrap();
        git_repo(tmp.path());
        create_at_session_start(tmp.path().to_str().unwrap(), "s4", "test").unwrap();
        // User commits after the agent ran → HEAD moves.
        std::fs::write(tmp.path().join("base.txt"), "user\n").unwrap();
        git_run(tmp.path(), &["add", "."]).unwrap();
        git_run(tmp.path(), &["commit", "-m", "user"]).unwrap();
        // ...then dirties the working tree again.
        std::fs::write(tmp.path().join("base.txt"), "dirty\n").unwrap();

        let err = apply_rollback(tmp.path().to_str().unwrap(), "s4", false).unwrap_err();
        assert!(err.contains("HEAD moved"), "got: {err}");

        // force bypasses the guard; the dirty tracked file is restored to HEAD.
        let res = apply_rollback(tmp.path().to_str().unwrap(), "s4", true).unwrap();
        assert!(res.restored_files.contains(&"base.txt".to_string()));
        let restored = std::fs::read_to_string(tmp.path().join("base.txt")).unwrap();
        assert_eq!(restored.replace("\r\n", "\n"), "user\n");
    }

    #[test]
    fn rollback_errors_when_no_checkpoint() {
        let tmp = tempfile::TempDir::new().unwrap();
        git_repo(tmp.path());
        let err = apply_rollback(tmp.path().to_str().unwrap(), "ghost", false).unwrap_err();
        assert!(err.contains("no checkpoint"));
    }

    #[test]
    fn create_returns_err_outside_git_repo() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = create_at_session_start(tmp.path().to_str().unwrap(), "s5", "test").unwrap_err();
        assert!(err.contains("git"), "got: {err}");
    }
}
