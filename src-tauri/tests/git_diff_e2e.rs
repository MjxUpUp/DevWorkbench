//! A2 end-to-end: `list_changed_files` + `get_file_diff` against a REAL git
//! repo. The unit tests cover the pure parsers; this proves the shell-out
//! path (porcelain -z + numstat merge, tracked vs untracked dispatch) works on
//! the actual `git` binary — the layer the unit tests can't reach.
//!
//! Skips when git isn't on PATH so CI without git doesn't go red.

use std::path::Path;
use std::process::Command;

use app_lib::commands::git::{get_file_diff, list_changed_files};

fn git_available() -> bool {
    Command::new("git").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// git with CREATE_NO_WINDOW on Windows + the identity/GPG overrides a fresh
/// repo needs to actually commit (no global user, no keyring GPG).
fn git(workdir: &Path) -> Command {
    let mut c = Command::new("git");
    c.current_dir(workdir)
        .args(["-c", "user.name=Test", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"]);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

#[test]
fn list_changed_files_sees_modified_and_untracked() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    // Base commit with a.txt.
    git(p).args(["init", "-q"]).status().unwrap();
    std::fs::write(p.join("a.txt"), "line1\nline2\nline3\n").unwrap();
    git(p).args(["add", "a.txt"]).status().unwrap();
    git(p).args(["commit", "-q", "-m", "base"]).status().unwrap();

    // Working-tree changes: modify a.txt, add untracked b.txt.
    std::fs::write(p.join("a.txt"), "line1\nCHANGED\nline3\nEXTRA\n").unwrap();
    std::fs::write(p.join("b.txt"), "brand\nnew\nfile\n").unwrap();

    let files = list_changed_files(p.to_string_lossy().to_string()).expect("list_changed_files");
    let by_path: std::collections::HashMap<String, _> =
        files.iter().map(|f| (f.path.clone(), f)).collect();

    let a = by_path.get("a.txt").expect("a.txt in change set");
    // Modified against HEAD: 1 removal (line2), 2 additions (CHANGED, EXTRA).
    assert_eq!(a.status, "M", "a.txt status: {:?}", a);
    assert!(a.removed >= 1, "a.txt removed: {:?}", a);
    assert!(a.added >= 2, "a.txt added: {:?}", a);

    let b = by_path.get("b.txt").expect("b.txt in change set");
    assert_eq!(b.status, "U", "b.txt should be untracked: {:?}", b);
    // Untracked file → its whole content counts as additions (3 lines).
    assert_eq!(b.added, 3, "b.txt added: {:?}", b);
    assert_eq!(b.removed, 0);

    // Untracked sorts after tracked.
    let a_idx = files.iter().position(|f| f.path == "a.txt").unwrap();
    let b_idx = files.iter().position(|f| f.path == "b.txt").unwrap();
    assert!(a_idx < b_idx, "tracked should precede untracked: {:?}", files);
}

#[test]
fn get_file_diff_parses_tracked_modification() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p).args(["init", "-q"]).status().unwrap();
    std::fs::write(p.join("a.txt"), "alpha\nbeta\n").unwrap();
    git(p).args(["add", "a.txt"]).status().unwrap();
    git(p).args(["commit", "-q", "-m", "base"]).status().unwrap();
    std::fs::write(p.join("a.txt"), "alpha\nBETA\ngamma\n").unwrap();

    let diff = get_file_diff(p.to_string_lossy().to_string(), "a.txt".into()).expect("get_file_diff");
    assert!(!diff.is_binary);
    let adds: Vec<_> = diff.hunks.iter().filter(|l| l.kind == "add").cloned().collect();
    let removes: Vec<_> = diff.hunks.iter().filter(|l| l.kind == "remove").cloned().collect();
    assert!(adds.iter().any(|l| l.text == "BETA"), "added lines: {:?}", adds);
    assert!(adds.iter().any(|l| l.text == "gamma"), "added lines: {:?}", adds);
    assert!(removes.iter().any(|l| l.text == "beta"), "removed lines: {:?}", removes);
}

#[test]
fn get_file_diff_synthesizes_untracked_as_all_additions() {
    if !git_available() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git(p).args(["init", "-q"]).status().unwrap();
    // A committed file so HEAD exists, plus an untracked new file.
    std::fs::write(p.join("base.txt"), "x\n").unwrap();
    git(p).args(["add", "base.txt"]).status().unwrap();
    git(p).args(["commit", "-q", "-m", "base"]).status().unwrap();
    std::fs::write(p.join("new.txt"), "one\ntwo\n").unwrap();

    let diff = get_file_diff(p.to_string_lossy().to_string(), "new.txt".into()).expect("get_file_diff");
    assert!(!diff.is_binary);
    let adds: Vec<_> = diff.hunks.iter().filter(|l| l.kind == "add").collect();
    assert_eq!(adds.len(), 2, "untracked file = every line an addition: {:?}", diff.hunks);
    assert_eq!(adds[0].text, "one");
    // No remove lines for a brand-new file.
    assert!(diff.hunks.iter().all(|l| l.kind != "remove"));
}
