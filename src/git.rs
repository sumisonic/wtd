use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

// Shell-out layer to the git CLI. gitoxide is not used (to avoid diverging from
// real git behavior around worktrees and merge-base).

pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {:?}", args))?;
    if !out.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Return stdout regardless of exit code (git diff --no-index exits 1 when there are differences)
fn git_lenient(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("failed to run git {:?}", args))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
    pub is_main: bool,
}

/// List the main checkout plus all worktrees (main first)
pub fn list_worktrees(repo: &Path) -> Result<Vec<Worktree>> {
    let out = git(repo, &["worktree", "list", "--porcelain"])?;
    let mut result = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch = String::new();
    let mut head = String::new();
    let mut entries: Vec<(PathBuf, String, String)> = Vec::new();
    for line in out.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            if let Some(prev) = path.take() {
                entries.push((prev, branch.clone(), head.clone()));
            }
            path = Some(PathBuf::from(p));
            branch.clear();
            head.clear();
        } else if let Some(h) = line.strip_prefix("HEAD ") {
            head = h.to_string();
        } else if let Some(b) = line.strip_prefix("branch ") {
            branch = b.strip_prefix("refs/heads/").unwrap_or(b).to_string();
        } else if line == "detached" {
            branch = format!("({})", head.chars().take(8).collect::<String>());
        }
    }
    if let Some(prev) = path.take() {
        entries.push((prev, branch, head));
    }
    for (i, (p, b, _h)) in entries.into_iter().enumerate() {
        result.push(Worktree {
            path: p,
            branch: b,
            is_main: i == 0,
        });
    }
    Ok(result)
}

/// Top of the repository (resolves to the main checkout when started inside a worktree)
pub fn main_checkout(start: &Path) -> Result<PathBuf> {
    let top = git(start, &["rev-parse", "--show-toplevel"])?;
    let top = PathBuf::from(top.trim());
    let wts = list_worktrees(&top)?;
    wts.first()
        .map(|w| w.path.clone())
        .context("no worktree found")
}

/// Determine the base branch. "auto" picks develop if it exists, otherwise origin/HEAD,
/// otherwise whichever of main/master exists.
pub fn detect_base(repo: &Path, configured: &str) -> String {
    if configured != "auto" && !configured.is_empty() {
        return configured.to_string();
    }
    if git(
        repo,
        &["show-ref", "--verify", "--quiet", "refs/heads/develop"],
    )
    .is_ok()
    {
        return "develop".to_string();
    }
    if let Ok(sym) = git(repo, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        if let Some(name) = sym.trim().strip_prefix("refs/remotes/origin/") {
            return name.to_string();
        }
    }
    for cand in ["main", "master"] {
        let r = format!("refs/heads/{cand}");
        if git(repo, &["show-ref", "--verify", "--quiet", &r]).is_ok() {
            return cand.to_string();
        }
    }
    "HEAD".to_string()
}

pub fn merge_base(wt: &Path, base: &str) -> Option<String> {
    git(wt, &["merge-base", base, "HEAD"])
        .ok()
        .map(|s| s.trim().to_string())
}

/// Current branch name of the worktree (None when detached)
fn current_branch(wt: &Path) -> Option<String> {
    let out = git(wt, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok()?;
    let s = out.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Upstream of the current branch (e.g. origin/develop); None if unset.
fn upstream_ref(wt: &Path) -> Option<String> {
    let out = git(
        wt,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .ok()?;
    let s = out.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Determine the anchor commit that diffs are computed against.
///
/// When `smart` (config base is "auto") and HEAD sits on the base branch itself,
/// the anchor switches to the upstream (origin/<base>). Even in a main-only workflow
/// this makes commits stacked since the last push visible as a cumulative diff. Without
/// an upstream it naturally degrades to merge-base(base, HEAD) (= HEAD, uncommitted only).
///
/// When diverged from base (feature branch / worktree), it is always merge-base(base).
pub fn anchor(wt: &Path, base: &str, smart: bool) -> Option<String> {
    if smart {
        if let Some(cur) = current_branch(wt) {
            if cur == base {
                if let Some(up) = upstream_ref(wt) {
                    return merge_base(wt, &up);
                }
            }
        }
    }
    merge_base(wt, base)
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub short: String,
    pub time: i64,
    pub subject: String,
}

/// Commits in merge-base..HEAD (newest first)
pub fn commits_since(wt: &Path, mb: &str) -> Vec<CommitInfo> {
    let range = format!("{mb}..HEAD");
    let Ok(out) = git(wt, &["log", "--format=%H\u{1f}%h\u{1f}%ct\u{1f}%s", &range]) else {
        return Vec::new();
    };
    out.lines()
        .filter_map(|l| {
            let mut it = l.split('\u{1f}');
            Some(CommitInfo {
                sha: it.next()?.to_string(),
                short: it.next()?.to_string(),
                time: it.next()?.parse().ok()?,
                subject: it
                    .next()
                    .unwrap_or("")
                    .chars()
                    .map(|c| if (c as u32) < 0x20 { ' ' } else { c })
                    .collect(),
            })
        })
        .collect()
}

/// Which snapshot of changes to view
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiffSource {
    /// Cumulative diff from the merge-base (committed + uncommitted)
    All,
    /// Uncommitted diff from HEAD
    Current,
    /// Diff of a single commit
    Commit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Other,
}

impl FileStatus {
    pub fn badge(self) -> char {
        match self {
            FileStatus::Modified => 'M',
            FileStatus::Added => 'A',
            FileStatus::Deleted => 'D',
            FileStatus::Renamed => 'R',
            FileStatus::Untracked => '?',
            FileStatus::Other => '·',
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileChange {
    /// Path relative to the worktree (new path for renames)
    pub path: String,
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// None means binary
    pub added: Option<u64>,
    pub deleted: Option<u64>,
}

fn parse_status_char(s: &str) -> FileStatus {
    match s.chars().next() {
        Some('M') => FileStatus::Modified,
        Some('A') => FileStatus::Added,
        Some('D') => FileStatus::Deleted,
        Some('R') => FileStatus::Renamed,
        Some('C') => FileStatus::Renamed,
        _ => FileStatus::Other,
    }
}

/// Parse name-status -z output
fn parse_name_status(out: &str) -> Vec<(FileStatus, Option<String>, String)> {
    let mut result = Vec::new();
    let mut toks = out.split('\0').filter(|t| !t.is_empty());
    while let Some(status_tok) = toks.next() {
        let status = parse_status_char(status_tok);
        if status_tok.starts_with('R') || status_tok.starts_with('C') {
            let (Some(old), Some(new)) = (toks.next(), toks.next()) else {
                break;
            };
            result.push((status, Some(old.to_string()), new.to_string()));
        } else {
            let Some(path) = toks.next() else { break };
            result.push((status, None, path.to_string()));
        }
    }
    result
}

/// Parse numstat -z output -> (path, added, deleted); None for binary.
fn parse_numstat(out: &str) -> Vec<(String, Option<u64>, Option<u64>)> {
    let mut result = Vec::new();
    let mut toks = out.split('\0').peekable();
    while let Some(tok) = toks.next() {
        if tok.is_empty() {
            continue;
        }
        let mut fields = tok.splitn(3, '\t');
        let (Some(a), Some(d), Some(rest)) = (fields.next(), fields.next(), fields.next()) else {
            continue;
        };
        let added = a.parse::<u64>().ok();
        let deleted = d.parse::<u64>().ok();
        let path = if rest.is_empty() {
            // Rename: the next two tokens are old, new
            let _old = toks.next();
            match toks.next() {
                Some(new) => new.to_string(),
                None => continue,
            }
        } else {
            rest.to_string()
        };
        result.push((path, added, deleted));
    }
    result
}

/// Count lines of an untracked file (None if it looks binary)
fn count_lines(path: &Path) -> Option<u64> {
    let data = std::fs::read(path).ok()?;
    let head = &data[..data.len().min(8192)];
    if head.contains(&0) {
        return None;
    }
    Some(data.iter().filter(|&&b| b == b'\n').count() as u64)
}

/// Changed files for the given source
pub fn changed_files(wt: &Path, source: &DiffSource, mb: &str) -> Vec<FileChange> {
    let (ns_args, num_args): (Vec<String>, Vec<String>) = match source {
        DiffSource::All => (
            vec![
                "diff".into(),
                "--name-status".into(),
                "-M".into(),
                "-z".into(),
                mb.into(),
            ],
            vec![
                "diff".into(),
                "--numstat".into(),
                "-M".into(),
                "-z".into(),
                mb.into(),
            ],
        ),
        DiffSource::Current => (
            vec![
                "diff".into(),
                "--name-status".into(),
                "-M".into(),
                "-z".into(),
                "HEAD".into(),
            ],
            vec![
                "diff".into(),
                "--numstat".into(),
                "-M".into(),
                "-z".into(),
                "HEAD".into(),
            ],
        ),
        DiffSource::Commit(sha) => (
            vec![
                "show".into(),
                "--format=".into(),
                "--name-status".into(),
                "-M".into(),
                "-z".into(),
                sha.clone(),
            ],
            vec![
                "show".into(),
                "--format=".into(),
                "--numstat".into(),
                "-M".into(),
                "-z".into(),
                sha.clone(),
            ],
        ),
    };
    let ns_ref: Vec<&str> = ns_args.iter().map(|s| s.as_str()).collect();
    let num_ref: Vec<&str> = num_args.iter().map(|s| s.as_str()).collect();
    let ns = git_lenient(wt, &ns_ref).unwrap_or_default();
    let num = git_lenient(wt, &num_ref).unwrap_or_default();
    let stats: std::collections::HashMap<String, (Option<u64>, Option<u64>)> = parse_numstat(&num)
        .into_iter()
        .map(|(p, a, d)| (p, (a, d)))
        .collect();

    let mut files: Vec<FileChange> = parse_name_status(&ns)
        .into_iter()
        .map(|(status, old, path)| {
            let (added, deleted) = stats.get(&path).copied().unwrap_or((None, None));
            FileChange {
                path,
                old_path: old,
                status,
                added,
                deleted,
            }
        })
        .collect();

    // Untracked files (All / Current only)
    if matches!(source, DiffSource::All | DiffSource::Current) {
        if let Ok(out) = git(wt, &["ls-files", "--others", "--exclude-standard", "-z"]) {
            // A trailing "/" marks a directory entry for a nested git repo (worktree etc.); skip it
            for p in out
                .split('\0')
                .filter(|p| !p.is_empty() && !p.ends_with('/'))
            {
                let lines = count_lines(&wt.join(p));
                files.push(FileChange {
                    path: p.to_string(),
                    old_path: None,
                    status: FileStatus::Untracked,
                    added: lines,
                    deleted: lines.map(|_| 0),
                });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// Unified diff text for a single file
pub fn diff_text(wt: &Path, source: &DiffSource, file: &FileChange, mb: &str) -> String {
    if file.status == FileStatus::Untracked {
        let dev_null = "/dev/null";
        return git_lenient(wt, &["diff", "--no-index", "--", dev_null, &file.path])
            .unwrap_or_default();
    }
    let mut args: Vec<String> = match source {
        DiffSource::All => vec!["diff".into(), "-M".into(), mb.into()],
        DiffSource::Current => vec!["diff".into(), "-M".into(), "HEAD".into()],
        DiffSource::Commit(sha) => {
            vec!["show".into(), "--format=".into(), "-M".into(), sha.clone()]
        }
    };
    args.push("--".into());
    if let Some(old) = &file.old_path {
        args.push(old.clone());
    }
    args.push(file.path.clone());
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    git_lenient(wt, &refs).unwrap_or_default()
}

/// Cumulative totals for the worktree bar (+a, -d)
pub fn totals(files: &[FileChange]) -> (u64, u64) {
    files.iter().fold((0, 0), |(a, d), f| {
        (a + f.added.unwrap_or(0), d + f.deleted.unwrap_or(0))
    })
}
