use crate::git::{self, DiffSource};
use anyhow::Result;
use std::path::Path;

// `wtd status` — one-shot text/JSON output (non-interactive)

pub fn run(repo: &Path, base: &str, smart: bool, json: bool) -> Result<()> {
    let wts = git::list_worktrees(repo)?;
    let mut entries = Vec::new();
    for wt in &wts {
        let mb = git::anchor(&wt.path, base, smart).unwrap_or_else(|| "HEAD".into());
        let files = git::changed_files(&wt.path, &DiffSource::All, &mb);
        let (a, d) = git::totals(&files);
        entries.push((wt, files, a, d));
    }

    if json {
        let val = serde_json::json!({
            "repo": repo.to_string_lossy(),
            "base": base,
            "worktrees": entries.iter().map(|(wt, files, a, d)| {
                serde_json::json!({
                    "branch": wt.branch,
                    "path": wt.path.to_string_lossy(),
                    "is_main": wt.is_main,
                    "added": a,
                    "deleted": d,
                    "files": files.iter().map(|f| serde_json::json!({
                        "path": f.path,
                        "status": f.status.badge().to_string(),
                        "added": f.added,
                        "deleted": f.deleted,
                    })).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&val)?);
        return Ok(());
    }

    for (wt, files, a, d) in &entries {
        let name = if wt.is_main {
            format!("{} (main checkout)", wt.branch)
        } else {
            wt.branch.clone()
        };
        if files.is_empty() {
            println!("{name}  clean");
            continue;
        }
        println!("{name}  +{a} -{d}  [{}]", wt.path.display());
        for f in files {
            let stats = match (f.added, f.deleted) {
                (Some(a), Some(d)) => format!("+{a} -{d}"),
                _ => "Bin".into(),
            };
            println!("  {} {:<50} {}", f.status.badge(), f.path, stats);
        }
        println!();
    }
    Ok(())
}
