# wtd — worktree-aware diff watcher TUI

A TUI that watches every worktree in a repository and shows "what did this task
change" on a single screen — a worktree-wide version of Claude Code's `/diff`.
Built to sit in the bottom-right tmux pane and watch parallel agents (Claude
Code background jobs and the like) work in real time.

- Auto-discovers and watches all worktrees (`.claude/worktrees/*`, `.worktrees/*`, …)
- The primary view is the **cumulative diff from the merge-base against
  develop/main** (agents commit as they go, so a view of uncommitted changes
  alone empties out as the task progresses)
- `/diff`-style history tabs (All | Current | c4 c3 …) to step back commit by commit
- Fork-style two-column layout: icon file tree on the left, rich diff on the right
- Pluggable editor integration (a command template in the config; no opener is
  configured by default)

## Usage

```sh
wtd                # start the TUI (watch the current repository)
wtd --repo <path>  # watch a specific repository
wtd status         # one-shot text output (non-interactive)
wtd status --json  # machine-readable output
```

## Install

```sh
cargo install --path .
```

Configuration lives at `~/.config/wtd/config.toml` (optional; built-in defaults
apply when it is absent). See [examples/config.toml](examples/config.toml) for
a reference of every key.
