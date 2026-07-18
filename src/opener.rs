use crate::config::Opener;
use std::path::Path;
use std::process::{Command, Stdio};

// Opener abstraction: fill in a command template and spawn it fire-and-forget.
// Placeholders: {file} {line} {worktree} {repo}

pub struct OpenCtx<'a> {
    pub file: &'a Path,
    pub line: Option<u32>,
    pub worktree: &'a Path,
    pub repo: &'a Path,
}

fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    s.to_string()
}

pub fn spawn(opener: &Opener, ctx: &OpenCtx) -> Result<(), String> {
    if opener.cmd.is_empty() {
        return Err("opener cmd is empty".into());
    }
    let line = ctx.line.unwrap_or(1).to_string();
    let args: Vec<String> = opener
        .cmd
        .iter()
        .map(|t| {
            expand_tilde(t)
                .replace("{file}", &ctx.file.to_string_lossy())
                .replace("{line}", &line)
                .replace("{worktree}", &ctx.worktree.to_string_lossy())
                .replace("{repo}", &ctx.repo.to_string_lossy())
        })
        .collect();
    Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to spawn {}: {e}", args[0]))
}

/// Open with the OS default application (macOS: open, otherwise: xdg-open)
pub fn spawn_system(file: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(not(target_os = "macos"))]
    let cmd = "xdg-open";
    Command::new(cmd)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to spawn {cmd}: {e}"))
}
