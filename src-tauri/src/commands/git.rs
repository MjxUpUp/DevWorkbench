use crate::models::GitStatus;

use serde::Serialize;

/// A2 — one file in the working-tree change set, with per-file line counts.
/// `status` is the porcelain XY code collapsed to a single letter the UI badges
/// on: M(odified) / A(dded) / D(eleted) / R(enamed) / U(ntracked) / C(onflict).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub added: u64,
    pub removed: u64,
}

/// One rendered line of a unified diff. `kind` drives the row color:
/// "context" (unchanged), "add" (+), "remove" (−), "meta" (the `@@ hunk @@`
/// header + the `diff --git`/`index`/`---`/`+++` file headers we keep for
/// context). `oldNo`/`newNo` are 1-based line numbers in the old/new file
/// (None on meta + add has no oldNo, remove has no newNo).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: String,
    pub text: String,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
}

/// A parsed per-file diff for the A2 viewer. `is_binary` short-circuits the UI
/// to a "binary file" badge instead of trying to render gibberish.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<DiffLine>,
    pub is_binary: bool,
}

/// 获取单个项目的 Git 状态
#[tauri::command]
pub fn get_git_status(project_path: String) -> Result<GitStatus, String> {
    let repo = git2::Repository::open(&project_path)
        .map_err(|e| format!("无法打开 Git 仓库: {}", e))?;

    let branch = get_branch_name(&repo);
    let is_dirty = is_repo_dirty(&repo);
    let (ahead, behind) = get_ahead_behind(&repo);
    let last_commit_time = get_last_commit_time(&repo);
    let (insertions, deletions) = count_line_changes(&repo);

    Ok(GitStatus {
        branch,
        is_dirty,
        ahead,
        behind,
        last_commit_time,
        insertions,
        deletions,
    })
}

fn get_branch_name(repo: &git2::Repository) -> String {
    // 先尝试获取当前 HEAD 指向的分支名
    if let Ok(head) = repo.head() {
        if let Some(name) = head.shorthand() {
            return name.to_string();
        }
    }
    // detached HEAD 或其他情况
    "HEAD (detached)".to_string()
}

fn is_repo_dirty(repo: &git2::Repository) -> bool {
    let mut opts = git2::StatusOptions::new();
    // 包含工作区和暂存区的变更
    opts.include_untracked(true)
        .recurse_untracked_dirs(false)
        .exclude_submodules(true);

    match repo.statuses(Some(&mut opts)) {
        Ok(statuses) => statuses.iter().any(|s| {
            let s = s.status();
            // 任何非"当前"状态都算 dirty
            s != git2::Status::CURRENT
                && (s.is_wt_new()
                    || s.is_wt_modified()
                    || s.is_wt_deleted()
                    || s.is_wt_renamed()
                    || s.is_index_new()
                    || s.is_index_modified()
                    || s.is_index_deleted()
                    || s.is_index_renamed()
                    || s.is_conflicted())
        }),
        Err(_) => false,
    }
}

fn get_ahead_behind(repo: &git2::Repository) -> (u32, u32) {
    // 获取当前分支的 HEAD commit
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return (0, 0),
    };

    let branch_name = match head.shorthand() {
        Some(name) => name,
        None => return (0, 0),
    };

    // 查找本地分支
    let branch = match repo.find_branch(branch_name, git2::BranchType::Local) {
        Ok(b) => b,
        Err(_) => return (0, 0),
    };

    // 查找上游分支
    let upstream = match branch.upstream() {
        Ok(u) => u,
        Err(_) => return (0, 0), // 没有上游，不算 ahead/behind
    };

    let local_oid = match branch.get().target() {
        Some(oid) => oid,
        None => return (0, 0),
    };

    let upstream_oid = match upstream.get().target() {
        Some(oid) => oid,
        None => return (0, 0),
    };

    match repo.graph_ahead_behind(local_oid, upstream_oid) {
        Ok((a, b)) => (a as u32, b as u32),
        Err(_) => (0, 0),
    }
}

fn get_last_commit_time(repo: &git2::Repository) -> Option<String> {
    let head = repo.head().ok()?;
    let commit = repo.find_commit(head.target()?).ok()?;
    let time = commit.time();
    let secs = time.seconds();
    let dt = chrono::DateTime::from_timestamp(secs, 0)?;
    Some(dt.to_rfc3339())
}

/// 统计工作区相对 HEAD 的增删行数（含未跟踪文件，遵守 .gitignore）。
///
/// 早期实现用 `git2` 逐个 `read_to_string` 未跟踪文件并递归目录，对一个含
/// `node_modules`/`target` 的项目会读取成千上万个文件，阻塞命令数十秒，导致
/// 前端 IPC 卡死、界面白屏。改为直接调用 `git diff --shortstat` —— 命令行 git
/// 原生遵守 .gitignore、单次扫描完成，且自带 --untracked-files，既快又安全。
fn count_line_changes(repo: &git2::Repository) -> (u64, u64) {
    let workdir = match repo.workdir() {
        Some(p) => p,
        None => return (0, 0),
    };

    // `git diff --shortstat HEAD --untracked-files=normal`
    // 输出形如：" 2 files changed, 42 insertions(+), 7 deletions(-)\n"
    let mut cmd = std::process::Command::new("git");
    cmd.args(["diff", "--shortstat", "--untracked-files=normal", "HEAD"])
        .current_dir(workdir);

    // CREATE_NO_WINDOW — 不弹出控制台窗口（与 editor.rs/pty.rs 一致）。
    // 没有这个标志，Windows 上每次 git 子进程都会闪一个黑框，而此命令在
    // 每次切换项目（GitPanel + TitleBar 各调一次 get_git_status）都会触发。
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let output = cmd.output();

    let stdout = match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
        Err(_) => return (0, 0),
    };

    let parse = |key: &str| -> u64 {
        stdout.lines().next().and_then(|line| {
            let mut tokens = line.split_whitespace();
            let mut prev: Option<&str> = None;
            for tok in tokens.by_ref() {
                if tok.starts_with(key) {
                    if let Some(n) = prev {
                        if let Ok(v) = n.parse::<u64>() {
                            return Some(v);
                        }
                    }
                }
                prev = Some(tok);
            }
            None
        }).unwrap_or(0)
    };

    (parse("insertion"), parse("deletion"))
}

// ── A2: per-file diff viewer ────────────────────────────────────────────────
//
// Two commands back the working-tree change viewer in GitPanel:
//   - `list_changed_files`: the lightweight file list + per-file +/− counts
//     (cheap enough to poll alongside `get_git_status`).
//   - `get_file_diff`: the heavy per-file hunk payload, fetched on expand so a
//     10k-line diff never loads unless the user opens it.
//
// Both shell out to `git` (not git2) because libgit2's diff API is fiddlier to
// get byte-identical to `git diff` output, and the porcelain/-z formats are a
// stable contract. CREATE_NO_WINDOW is set so Windows doesn't flash a console
// on every poll — same reason `count_line_changes` sets it.

/// Build a `git` command in `workdir` with CREATE_NO_WINDOW on Windows.
fn git_cmd(workdir: &std::path::Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(workdir);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd
}

/// Run `git`, returning stdout as a lossy String. Errors surface as a String
/// the frontend's ErrorBoundary already knows how to degrade on.
fn git_output(workdir: &std::path::Path, args: &[&str]) -> Result<String, String> {
    let out = git_cmd(workdir)
        .args(args)
        .output()
        .map_err(|e| format!("git {:?} 启动失败: {e}", args))?;
    if !out.status.success() {
        return Err(format!(
            "git {:?} 失败 (exit {}): {}",
            args,
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The collapsed single-letter status a UI badge shows, from a porcelain XY
/// pair. Untracked (`??`) → "U"; renames/copies collapse to "R"/"C"; conflicts
/// ("DD"/"AU"/"UD"/… → both letters non-space) → "C".
fn status_letter(x: char, y: char) -> &'static str {
    let pair = format!("{x}{y}");
    match pair.as_str() {
        "??" => "U",
        "A " | "A." | " A" => "A",
        "D " | " D" | "DM" | "MD" => "D",
        "R " | " R" | "RM" => "R",
        "C " | " C" | "CM" => "C",
        _ => {
            // Any remaining conflict pair (both letters set) is a conflict.
            if x != ' ' && y != ' ' && x != '?' {
                "C"
            } else {
                "M"
            }
        }
    }
}

/// `list_changed_files` — the per-file change set with line counts.
/// Tracked changes come from `git diff --numstat HEAD` (added/removed/path);
/// untracked files aren't in numstat, so they're merged in from
/// `git status --porcelain -z` with `added = file line count`, `removed = 0`.
#[tauri::command]
pub fn list_changed_files(project_path: String) -> Result<Vec<ChangedFile>, String> {
    let workdir = std::path::PathBuf::from(&project_path);
    // Fail fast + friendly if this isn't a git repo (GitPanel already guards,
    // but a stray call from a future caller shouldn't panic).
    git_cmd(&workdir)
        .args(["rev-parse", "--git-dir"])
        .output()
        .map_err(|e| format!("not a git repo ({e})"))?;

    // Tracked changes vs HEAD: `<added>\t<removed>\t<path>` (binary shows `-`).
    let mut by_path: std::collections::HashMap<String, ChangedFile> = std::collections::HashMap::new();
    let numstat = git_output(&workdir, &["diff", "--numstat", "HEAD"])?;
    for line in numstat.lines() {
        let mut parts = line.splitn(3, '\t');
        let added_s = parts.next().unwrap_or("0");
        let removed_s = parts.next().unwrap_or("0");
        let path = match parts.next() {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };
        let added = if added_s == "-" { 0 } else { added_s.parse::<u64>().unwrap_or(0) };
        let removed = if removed_s == "-" { 0 } else { removed_s.parse::<u64>().unwrap_or(0) };
        by_path.insert(
            path.to_string(),
            ChangedFile { path: path.to_string(), status: "M".into(), added, removed },
        );
    }

    // `git status --porcelain -z` is NUL-separated and unambiguous about
    // filenames-with-spaces. Walk entries; for renames/copies a second NUL
    // record holds the source path — consume it.
    let porcelain = git_cmd(&workdir)
        .args(["status", "--porcelain=v1", "-z"])
        .output()
        .map_err(|e| format!("git status 失败: {e}"))?;
    let raw = String::from_utf8_lossy(&porcelain.stdout);
    let records: Vec<&str> = raw.split('\0').collect();
    let mut i = 0;
    while i < records.len() {
        let rec = records[i];
        i += 1;
        if rec.is_empty() {
            continue;
        }
        if rec.len() < 3 {
            continue;
        }
        let bytes = rec.as_bytes();
        let x = bytes[0] as char;
        let y = bytes[1] as char;
        // Byte 2 is a space; path starts at byte 3.
        let path = rec[3..].to_string();
        // Renames/copies: the source path is the next record — consume + drop it.
        if matches!(status_letter(x, y), "R" | "C") && i < records.len() {
            i += 1;
        }
        if path.is_empty() {
            continue;
        }
        let letter = status_letter(x, y);
        let entry = by_path.entry(path.clone()).or_insert_with(|| ChangedFile {
            path: path.clone(),
            status: "M".into(),
            added: 0,
            removed: 0,
        });
        entry.status = letter.into();
        // Untracked files: numstat gave nothing → count the file's lines as adds.
        if letter == "U" {
            let full = workdir.join(&path);
            entry.added = std::fs::read_to_string(&full)
                .map(|s| s.lines().count() as u64)
                .unwrap_or(0);
            entry.removed = 0;
        }
    }

    // Stable order: untracked last, then path alpha (matches how `git status`
    // groups, so the list doesn't reshuffle between polls).
    let mut files: Vec<ChangedFile> = by_path.into_values().collect();
    files.sort_by(|a, b| {
        let au = a.status == "U";
        let bu = b.status == "U";
        au.cmp(&bu).then_with(|| a.path.cmp(&b.path))
    });
    Ok(files)
}

/// `get_file_diff` — the parsed hunks for one file, fetched on expand.
/// Tracked file → `git diff HEAD -- <path>` parsed into hunks. Untracked file
/// → synthesize an all-additions hunk from the file content (so the user sees
/// what's about to be committed). Binary → `is_binary = true`, empty hunks.
#[tauri::command]
pub fn get_file_diff(project_path: String, file_path: String) -> Result<FileDiff, String> {
    let workdir = std::path::PathBuf::from(&project_path);
    let full = workdir.join(&file_path);

    // Is the file untracked? `git ls-files --error-unmatch` exits non-zero for
    // untracked/ignored files — cheaper + more correct than re-parsing status.
    let tracked = git_cmd(&workdir)
        .args(["ls-files", "--error-unmatch", &file_path])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !tracked {
        // Untracked: synthesize an all-added hunk from current content.
        return Ok(synthesize_untracked_diff(&full, &file_path));
    }

    let raw = git_output(&workdir, &["diff", "HEAD", "--", &file_path])?;
    if raw.contains("Binary files") || raw.contains("binary files") {
        return Ok(FileDiff { path: file_path, hunks: vec![], is_binary: true });
    }
    Ok(parse_unified_diff(&raw, &file_path))
}

/// Parse a `git diff` blob into ordered DiffLines. Keeps the `@@ hunk @@`
/// headers (as "meta") so the viewer can render section breaks; drops the
/// `diff --git`/`index`/`---`/`+++` preamble (the viewer already shows the
/// path). Tracks running old/new line numbers so add/remove rows carry their
/// source/target line numbers.
fn parse_unified_diff(raw: &str, path: &str) -> FileDiff {
    let mut hunks: Vec<DiffLine> = Vec::new();
    let mut old_no: u32 = 0;
    let mut new_no: u32 = 0;
    for line in raw.lines() {
        // Hunk header: `@@ -oldStart,oldCount +newStart,newCount @@ context`
        if let Some(rest) = line.strip_prefix("@@") {
            // Find the closing @@ — the part between is `-a,b +c,d`.
            let header_end = rest.find("@@").unwrap_or(rest.len());
            let spec = &rest[..header_end];
            if let Some((old_start, new_start)) = parse_hunk_ranges(spec) {
                old_no = old_start;
                new_no = new_start;
            }
            let title = rest[header_end..].trim_start_matches("@@").trim();
            hunks.push(DiffLine {
                kind: "meta".into(),
                text: format!("@@ -… {title}").trim().to_string(),
                old_no: None,
                new_no: None,
            });
            continue;
        }
        // Skip the file preamble (diff --git / index / --- / +++).
        if line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("similarity ")
            || line.starts_with("rename ")
            || line.starts_with("copy ")
        {
            continue;
        }
        match line.chars().next() {
            Some('+') => {
                hunks.push(DiffLine {
                    kind: "add".into(),
                    text: line[1..].to_string(),
                    old_no: None,
                    new_no: Some(new_no),
                });
                new_no += 1;
            }
            Some('-') => {
                hunks.push(DiffLine {
                    kind: "remove".into(),
                    text: line[1..].to_string(),
                    old_no: Some(old_no),
                    new_no: None,
                });
                old_no += 1;
            }
            Some(' ') => {
                hunks.push(DiffLine {
                    kind: "context".into(),
                    text: line[1..].to_string(),
                    old_no: Some(old_no),
                    new_no: Some(new_no),
                });
                old_no += 1;
                new_no += 1;
            }
            Some('\\') => {
                // "\ No newline at end of file" — meta, no line advance.
                hunks.push(DiffLine {
                    kind: "meta".into(),
                    text: line.to_string(),
                    old_no: None,
                    new_no: None,
                });
            }
            _ => {
                // Anything else (e.g. an unexpected blank in a combined diff)
                // — surface verbatim as context rather than dropping it.
                hunks.push(DiffLine {
                    kind: "context".into(),
                    text: line.to_string(),
                    old_no: None,
                    new_no: None,
                });
            }
        }
    }
    FileDiff { path: path.to_string(), hunks, is_binary: false }
}

/// Pull `(oldStart, newStart)` out of a `-oldStart,oldCount +newStart,newCount`
/// spec (the inside of a `@@ … @@` header). Returns None on a malformed spec,
/// in which case the caller keeps its running counters.
fn parse_hunk_ranges(spec: &str) -> Option<(u32, u32)> {
    let spec = spec.trim();
    // `-10,7 +12,9` → old side starts with `-`, new side with `+`.
    let old_part = spec.split('+').next()?;
    let old_start = old_part.trim_start_matches('-').split(',').next()?.trim().parse::<u32>().ok()?;
    let plus_idx = spec.find('+')?;
    let new_part = &spec[plus_idx + 1..];
    let new_start = new_part.split(',').next()?.trim().parse::<u32>().ok()?;
    Some((old_start, new_start))
}

/// Untracked file → every line is an addition. Mimics `git diff`'s hunk shape
/// so the viewer renders it the same way as a tracked change. Reads as UTF-8
/// lossy; a genuinely binary untracked file returns is_binary.
fn synthesize_untracked_diff(full: &std::path::Path, path: &str) -> FileDiff {
    let bytes = match std::fs::read(full) {
        Ok(b) => b,
        Err(_) => return FileDiff { path: path.to_string(), hunks: vec![], is_binary: false },
    };
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return FileDiff { path: path.to_string(), hunks: vec![], is_binary: true };
    }
    let content = String::from_utf8_lossy(&bytes);
    let mut hunks = vec![DiffLine {
        kind: "meta".into(),
        text: "@@ -0,0 +1,N @@" .replace('N', &content.lines().count().to_string()),
        old_no: None,
        new_no: None,
    }];
    let mut new_no: u32 = 1;
    for line in content.lines() {
        hunks.push(DiffLine {
            kind: "add".into(),
            text: line.to_string(),
            old_no: None,
            new_no: Some(new_no),
        });
        new_no += 1;
    }
    FileDiff { path: path.to_string(), hunks, is_binary: false }
}

#[cfg(test)]
mod diff_tests {
    use super::*;

    #[test]
    fn status_letter_maps_porcelain_pairs() {
        assert_eq!(status_letter('?', '?'), "U");
        assert_eq!(status_letter('M', ' '), "M");
        assert_eq!(status_letter(' ', 'M'), "M");
        assert_eq!(status_letter('A', ' '), "A");
        assert_eq!(status_letter('D', ' '), "D");
        assert_eq!(status_letter('R', ' '), "R");
        // Conflict pair (both set, not ??): DD / AU / UD …
        assert_eq!(status_letter('D', 'D'), "C");
        assert_eq!(status_letter('A', 'U'), "C");
    }

    #[test]
    fn parse_hunk_ranges_reads_starts() {
        assert_eq!(parse_hunk_ranges("-10,7 +12,9"), Some((10, 12)));
        assert_eq!(parse_hunk_ranges("-1 +1,3"), Some((1, 1)));
        // Missing count on the old side is fine.
        assert_eq!(parse_hunk_ranges("-5 +5,2"), Some((5, 5)));
    }

    #[test]
    fn parse_hunk_ranges_rejects_garbage() {
        assert_eq!(parse_hunk_ranges("not a spec"), None);
        assert_eq!(parse_hunk_ranges("-abc +1"), None);
    }

    #[test]
    fn parse_unified_diff_counts_lines_and_tracks_numbers() {
        // Hand-rolled minimal diff: 2 context, 1 remove, 2 add.
        let raw = "\
diff --git a/f.txt b/f.txt
index 111..222 100644
--- a/f.txt
+++ b/f.txt
@@ -1,3 +1,4 @@
 ctx-a
-old-1
+new-1
+new-2
 ctx-b
";
        let d = parse_unified_diff(raw, "f.txt");
        assert!(!d.is_binary);
        // One meta hunk header, then 5 content lines.
        let meta_count = d.hunks.iter().filter(|l| l.kind == "meta").count();
        assert_eq!(meta_count, 1, "expected one hunk header, got {meta_count}");
        let adds: Vec<&DiffLine> = d.hunks.iter().filter(|l| l.kind == "add").collect();
        let removes: Vec<&DiffLine> = d.hunks.iter().filter(|l| l.kind == "remove").collect();
        assert_eq!(adds.len(), 2);
        assert_eq!(removes.len(), 1);
        // Line numbers advance from the hunk start (1,1). ctx-a consumes new=1,
        // so the first add (new-1) lands on new=2; old-1 sits at old=2 (after ctx-a).
        assert_eq!(adds[0].new_no, Some(2));
        assert_eq!(adds[1].new_no, Some(3));
        assert_eq!(removes[0].old_no, Some(2));
    }

    #[test]
    fn parse_unified_diff_drops_file_preamble() {
        let raw = "diff --git a/x b/x\nindex a..b\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n+hi\n";
        let d = parse_unified_diff(raw, "x");
        // No hunk line carries the preamble text.
        assert!(d.hunks.iter().all(|l| !l.text.contains("diff --git")));
        assert!(d.hunks.iter().all(|l| !l.text.contains("index a..b")));
    }

    #[test]
    fn synthesize_untracked_marks_every_line_added() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("new.txt");
        std::fs::write(&f, "alpha\nbeta\n").unwrap();
        let d = synthesize_untracked_diff(&f, "new.txt");
        assert!(!d.is_binary);
        let adds: Vec<&DiffLine> = d.hunks.iter().filter(|l| l.kind == "add").collect();
        assert_eq!(adds.len(), 2);
        assert_eq!(adds[0].text, "alpha");
        assert_eq!(adds[0].new_no, Some(1));
        assert_eq!(adds[1].text, "beta");
    }

    #[test]
    fn synthesize_untracked_detects_binary() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("blob.bin");
        let mut bytes = b"hello".to_vec();
        bytes.push(0u8);
        bytes.extend_from_slice(b"world");
        std::fs::write(&f, &bytes).unwrap();
        let d = synthesize_untracked_diff(&f, "blob.bin");
        assert!(d.is_binary);
        assert!(d.hunks.is_empty());
    }
}
