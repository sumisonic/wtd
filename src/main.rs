mod app;
mod config;
mod diffview;
mod git;
mod icons;
mod opener;
mod status_out;
mod tree;
mod ui;
mod watcher;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use std::io::stdout;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// worktree-aware diff watcher TUI
#[derive(Parser)]
#[command(name = "wtd", version, about)]
struct Cli {
    /// Target repository (defaults to the current directory)
    #[arg(long, global = true)]
    repo: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// One-shot text output (non-interactive)
    Status {
        /// Machine-readable JSON output
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = config::Config::load();
    let start = cli
        .repo
        .clone()
        .unwrap_or(std::env::current_dir().context("failed to get current directory")?);
    let repo = git::main_checkout(&start).context("not a git repository (use --repo <path>)")?;
    let cfg_base = cfg.general.base.trim().to_string();
    let smart = cfg_base.is_empty() || cfg_base == "auto";
    let base = git::detect_base(&repo, &cfg.general.base);

    match cli.cmd {
        Some(Cmd::Status { json }) => status_out::run(&repo, &base, smart, json),
        None => run_tui(cfg, repo),
    }
}

fn run_tui(cfg: config::Config, repo: PathBuf) -> Result<()> {
    let poll_ms = cfg.general.poll_ms.max(500);
    let mut app = app::App::new(cfg, repo.clone());

    // FS watching
    let (tx, rx) = mpsc::channel::<()>();
    let mut fs = watcher::Fs::new(tx).ok();

    app.refresh();
    if let Some(fs) = fs.as_mut() {
        let wts: Vec<PathBuf> = app.wts.iter().map(|w| w.info.path.clone()).collect();
        fs.sync(&repo, &wts);
    }

    // Terminal setup (restored even on panic)
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
        orig_hook(info);
    }));
    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal = ratatui::Terminal::new(backend)?;

    let mut last_refresh = Instant::now();
    let mut pending_fs: Option<Instant> = None;

    let result = loop {
        if event::poll(Duration::from_millis(80))? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => app.on_key(k),
                _ => {}
            }
        }
        // FS events (250ms debounce)
        let mut dirty = false;
        while rx.try_recv().is_ok() {
            dirty = true;
        }
        if dirty && pending_fs.is_none() {
            pending_fs = Some(Instant::now());
        }
        let fs_due = pending_fs
            .map(|t| t.elapsed() >= Duration::from_millis(250))
            .unwrap_or(false);
        let poll_due = last_refresh.elapsed() >= Duration::from_millis(poll_ms);
        if fs_due || poll_due {
            app.refresh();
            if let Some(fs) = fs.as_mut() {
                let wts: Vec<PathBuf> = app.wts.iter().map(|w| w.info.path.clone()).collect();
                fs.sync(&repo, &wts);
            }
            pending_fs = None;
            last_refresh = Instant::now();
        }
        // Receive finished work from the highlight worker and swap it in (shown on the next draw)
        app.poll_async();
        if app.want_clear {
            let _ = terminal.clear();
            app.want_clear = false;
        }
        if let Err(e) = terminal.draw(|f| ui::render(f, &mut app)) {
            break Err(e.into());
        }
        if app.quit {
            break Ok(());
        }
    };

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    result
}
