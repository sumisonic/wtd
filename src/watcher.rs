use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

// Filesystem watching via notify's platform backend. Events under `.git` are ignored (prevents falling into an event
// loop from index updates caused by wtd's own git invocations). Commits and worktree
// additions/removals are picked up by the polling fallback.

pub struct Fs {
    watcher: RecommendedWatcher,
    watched: HashSet<PathBuf>,
}

fn is_git_internal(p: &Path) -> bool {
    p.components().any(|c| c.as_os_str() == ".git")
}

impl Fs {
    pub fn new(tx: Sender<()>) -> notify::Result<Self> {
        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(ev) = res {
                if ev.paths.iter().any(|p| !is_git_internal(p)) {
                    let _ = tx.send(());
                }
            }
        })?;
        Ok(Self {
            watcher,
            watched: HashSet::new(),
        })
    }

    /// Sync watched paths to the worktree list (repo top + worktrees outside the top)
    pub fn sync(&mut self, repo_top: &Path, worktrees: &[PathBuf]) {
        let mut want: HashSet<PathBuf> = HashSet::new();
        want.insert(repo_top.to_path_buf());
        for wt in worktrees {
            if !wt.starts_with(repo_top) {
                want.insert(wt.clone());
            }
        }
        let stale: Vec<PathBuf> = self.watched.difference(&want).cloned().collect();
        for p in stale {
            let _ = self.watcher.unwatch(&p);
            self.watched.remove(&p);
        }
        let new: Vec<PathBuf> = want.difference(&self.watched).cloned().collect();
        for p in new {
            if self.watcher.watch(&p, RecursiveMode::Recursive).is_ok() {
                self.watched.insert(p);
            }
        }
    }
}
