# wtd

A Rust TUI that watches every worktree in a repository and shows "what did this
task change" on a single screen — a worktree-wide version of Claude Code's
`/diff`. It is meant to sit in a tmux pane and watch parallel agents (Claude
Code background jobs, worktrunk) work in real time.

## Responsibility boundaries

- **Read-only viewer.** Never changes staged content, worktree files, branches,
  or worktrees. (Git itself may refresh cached index metadata during read commands.)
- **Diffs and change counts come from the git CLI.** wtd does not recompute
  them; it displays the numbers and diffs exactly as `git` reports them.
- **Never creates worktrees.** Enumeration and watching only; creation belongs
  to Claude Code background jobs, worktrunk, and the like.
- **Never edits files.** Editor integration is delegated fire-and-forget to an
  opener (an external command).

## Design principles

- All git operations shell out to the git CLI (`src/git.rs`). No gitoxide —
  behavior must not drift from real git around worktrees and merge-base.
- Runs with built-in defaults when no config exists. A config that fails to
  load degrades to the defaults instead of aborting.
- The opener is not tied to any particular editor: it fills a command template
  (`{file}` `{line}` `{worktree}` `{repo}`) and runs it.
- Never block the TUI on diff work. Syntax highlighting is built asynchronously
  on a worker thread and swapped in; highlighting is skipped past `HL_MAX_LINES`
  changed lines and display is truncated at `MAX_LINES`.
- The filesystem watcher ignores events under `.git` — wtd's own git calls
  would otherwise feed back into the event loop.

## Quality standards

- Avoid `.unwrap()` / `.expect()` outside tests — a panic leaves the terminal
  in raw mode. Use `anyhow::Result` + `?`.
- Before every commit: `cargo fmt` → `cargo clippy -- -D warnings` →
  `cargo test` → `cargo build`, all with zero errors and warnings.
- Tests currently cover only the line-wrapping logic in `src/ui.rs`. Never
  claim tests passed for code they do not cover.
- Keep the dependency set minimal.
- Everything user-facing and everything in the repository — TUI labels, help,
  status text, code comments, commit messages (Conventional Commits style) —
  is written in English, except language-specific test fixtures (e.g. CJK
  strings exercising wide-character wrapping).

## Module map (`src/`)

| File | Responsibility |
|---|---|
| `main.rs` | CLI parsing, event loop, terminal setup |
| `app.rs` | Application state and key handling |
| `ui.rs` | Rendering (ratatui) |
| `git.rs` | Git CLI shell-out layer (worktree enumeration, base detection, diff retrieval) |
| `diffview.rs` | Unified diff → display lines (syntect highlighting, intra-line diff) |
| `tree.rs` | Building the changed-file tree |
| `watcher.rs` | Filesystem watching |
| `opener.rs` | Opener abstraction (command-template execution) |
| `config.rs` | Loading `~/.config/wtd/config.toml` |
| `status_out.rs` | Text/JSON output for `wtd status` |
| `icons.rs` | Nerd Font icon table |

## Reference

| Document | Contents |
|---|---|
| `examples/config.toml` | Config reference describing every key |

- When a change alters behavior this document or the config reference
  describes, update the document in the same commit.
