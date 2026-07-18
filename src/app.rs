use crate::config::Config;
use crate::diffview::{build_doc, DiffDoc, Highlighter, Hl};
use crate::git::{self, DiffSource, FileChange, FileStatus, Worktree};
use crate::opener::{self, OpenCtx};
use crate::tree::{flat_rows, tree_rows, Row};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Instant;

/// Skip syntax highlighting entirely when a diff's changed line count (sum of + and -) exceeds this.
/// Highlighting runs on a worker thread; this cap limits wasted CPU and swap-in latency.
const HL_MAX_LINES: u64 = 1200;

/// Number of DiffDoc entries kept in the LRU cache, so moving between files avoids rebuilds.
const DOC_CACHE: usize = 8;

/// Key for the doc cache (worktree, source, path, merge-base, edit fingerprint).
/// merge-base is an actual input to git diff, so it is part of the key to avoid stale output on base switches.
type DocKey = (PathBuf, DiffSource, String, String, u128);

/// Request and response for the highlight worker
struct HlReq {
    key: DocKey,
    text: String,
    path: String,
}
struct HlResp {
    key: DocKey,
    doc: DiffDoc,
}

/// Hand-off slot for highlight requests. Holds only the latest request,
/// immediately dropping the old one (intermediate files while scrubbing) on overwrite.
/// Unlike a channel, diff texts never pile up, so memory stays bounded.
struct HlSlot {
    req: std::sync::Mutex<Option<HlReq>>,
    cv: std::sync::Condvar,
}

impl HlSlot {
    fn put(&self, r: HlReq) {
        if let Ok(mut g) = self.req.lock() {
            *g = Some(r);
            self.cv.notify_one();
        }
    }

    /// Block until a request arrives and take it (worker side). Returns None on lock poisoning.
    fn take_blocking(&self) -> Option<HlReq> {
        let mut g = self.req.lock().ok()?;
        loop {
            if let Some(r) = g.take() {
                return Some(r);
            }
            g = self.cv.wait(g).ok()?;
        }
    }
}

/// Entry in the doc LRU cache. hl_done marks it as highlighted (or exempt).
/// The worker drops stale requests (latest-wins), so an entry may stay unfinished.
/// gen is the generation of the document content (invalidation key for the render-side
/// layout cache; Rc pointer values can falsely match via address reuse (ABA), so they are not used).
struct DocEntry {
    key: DocKey,
    doc: Rc<DiffDoc>,
    hl_done: bool,
    gen: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    Worktree,
    History,
    Tree,
    Diff,
}

/// Per-worktree UI state (remembered even after leaving the worktree)
#[derive(Default)]
pub struct WtUi {
    /// 0=All, 1=Current, 2+i = commit i (newest first)
    pub hist_sel: usize,
    pub collapsed: HashSet<String>,
    pub cursor: usize,
    pub scroll: usize,
}

pub struct WtData {
    pub info: Worktree,
    pub merge_base: Option<String>,
    pub commits: Vec<git::CommitInfo>,
    pub files: Vec<FileChange>,
    /// Cumulative totals for the worktree bar (relative to All)
    pub all_totals: (u64, u64),
}

pub struct App {
    pub cfg: Config,
    pub repo: PathBuf,
    pub repo_name: String,
    pub base: String,
    /// True when the configured base is "auto"; enables the smart rule of anchoring to the upstream while on the base branch.
    pub smart_base: bool,
    pub wts: Vec<WtData>,
    pub ui_state: HashMap<PathBuf, WtUi>,
    pub sel_wt: usize,
    pub focus: Area,
    pub flat: bool,
    pub filter: String,
    pub filter_input: bool,
    pub follow: bool,
    last_followed: Option<(PathBuf, std::time::SystemTime)>,
    pub sxs: bool,
    /// Whether to wrap long diff lines (toggled with w)
    pub wrap: bool,
    pub split_pct: u16,
    /// Diff scroll position (in physical lines, after wrapping)
    pub diff_scroll: usize,
    pub status: Option<(String, Instant)>,
    pub quit: bool,
    pub picker: Option<usize>,
    /// Whether the diff is shown full-screen in single-column mode
    pub narrow_diff: bool,
    /// Full redraw requested via Ctrl+l
    pub want_clear: bool,
    // Metrics recorded by the renderer (used by key handling)
    pub tree_height: usize,
    pub diff_height: usize,
    /// Max diff scroll computed by the renderer (physical lines)
    pub diff_max_scroll: usize,
    /// Prefix-sum from logical row index to physical start line (length rows+1; last entry is the physical total).
    /// Recomputed by the renderer when width, wrap, or doc changes. Used to translate hunk-jump coordinates.
    pub row_offsets: Vec<usize>,
    /// Key for row_offsets (doc generation + width + wrap + layout)
    pub layout_key: Option<(u64, u16, bool, bool)>,
    /// Generation of the document last returned by doc() (for layout_key)
    pub cur_doc_gen: u64,
    pub effective_sxs: bool,
    pub narrow: bool,
    // Diff cache (LRU with MRU at the front) and the key of the selected document
    docs: Vec<DocEntry>,
    cur_doc_key: Option<DocKey>,
    next_doc_gen: u64,
    // Highlight worker (via the latest-wins slot; intermediate files while scrubbing are skipped)
    hl_slot: std::sync::Arc<HlSlot>,
    hl_rx: mpsc::Receiver<HlResp>,
    initial_picked: bool,
}

impl App {
    pub fn new(cfg: Config, repo: PathBuf) -> Self {
        let repo_name = repo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "?".into());
        let cfg_base = cfg.general.base.trim().to_string();
        let smart_base = cfg_base.is_empty() || cfg_base == "auto";
        let base = git::detect_base(&repo, &cfg.general.base);
        let split_pct = cfg.ui.split_pct.clamp(15, 85);
        let wrap = cfg.ui.wrap;
        // Highlight worker: syntect is slow relative to line count (~1s for 4000 lines), so it
        // runs on a separate thread and the finished DiffDoc is swapped in via poll_async.
        // Building the Highlighter (SyntaxSet) is also moved to the thread so startup isn't blocked.
        // If the worker dies, put is ignored and we degrade to plain rendering (never crash).
        let hl_slot = std::sync::Arc::new(HlSlot {
            req: std::sync::Mutex::new(None),
            cv: std::sync::Condvar::new(),
        });
        let (resp_tx, hl_rx) = mpsc::channel::<HlResp>();
        let theme = cfg.ui.theme.clone();
        let slot = hl_slot.clone();
        std::thread::spawn(move || {
            let hi = Highlighter::new(&theme);
            while let Some(req) = slot.take_blocking() {
                let doc = build_doc(&req.text, &req.path, Hl::On(&hi));
                if resp_tx.send(HlResp { key: req.key, doc }).is_err() {
                    break;
                }
            }
        });
        Self {
            cfg,
            repo,
            repo_name,
            base,
            smart_base,
            wts: Vec::new(),
            ui_state: HashMap::new(),
            sel_wt: 0,
            focus: Area::Tree,
            flat: false,
            filter: String::new(),
            filter_input: false,
            follow: false,
            last_followed: None,
            sxs: true,
            wrap,
            split_pct,
            diff_scroll: 0,
            status: None,
            quit: false,
            picker: None,
            narrow_diff: false,
            want_clear: false,
            tree_height: 0,
            diff_height: 0,
            diff_max_scroll: 0,
            row_offsets: Vec::new(),
            layout_key: None,
            cur_doc_gen: 0,
            effective_sxs: false,
            narrow: false,
            docs: Vec::new(),
            cur_doc_key: None,
            next_doc_gen: 1,
            hl_slot,
            hl_rx,
            initial_picked: false,
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    // ---- Data refresh -----------------------------------------------------

    pub fn refresh(&mut self) {
        let Ok(wts) = git::list_worktrees(&self.repo) else {
            self.set_status("git worktree list failed");
            return;
        };
        // Track the selected worktree by path
        let sel_path = self.wts.get(self.sel_wt).map(|w| w.info.path.clone());
        // Track the selected file by path
        let sel_file = self.selected_file().map(|f| f.path.clone());

        let mut new_wts = Vec::new();
        for info in wts {
            let mb = git::anchor(&info.path, &self.base, self.smart_base);
            let commits = mb
                .as_deref()
                .map(|m| git::commits_since(&info.path, m))
                .unwrap_or_default();
            let ui = self.ui_state.entry(info.path.clone()).or_default();
            if ui.hist_sel >= 2 + commits.len() {
                ui.hist_sel = 0;
            }
            let mb_str = mb.clone().unwrap_or_else(|| "HEAD".into());
            let source = source_of(ui.hist_sel, &commits);
            let files = git::changed_files(&info.path, &source, &mb_str);
            let all_totals = if matches!(source, DiffSource::All) {
                git::totals(&files)
            } else {
                git::totals(&git::changed_files(&info.path, &DiffSource::All, &mb_str))
            };
            new_wts.push(WtData {
                info,
                merge_base: mb,
                commits,
                files,
                all_totals,
            });
        }
        self.wts = new_wts;
        if let Some(p) = sel_path {
            if let Some(i) = self.wts.iter().position(|w| w.info.path == p) {
                self.sel_wt = i;
            }
        }
        if self.sel_wt >= self.wts.len() {
            self.sel_wt = 0;
        }
        // Right after startup: if the main checkout is clean, pick the first worktree with changes
        if !self.initial_picked {
            self.initial_picked = true;
            let main_clean = self
                .wts
                .first()
                .map(|w| w.files.is_empty())
                .unwrap_or(false);
            if self.sel_wt == 0 && main_clean {
                if let Some(i) = self
                    .wts
                    .iter()
                    .position(|w| !w.info.is_main && !w.files.is_empty())
                {
                    self.sel_wt = i;
                }
            }
        }
        // Keep the cursor stable (return to the file with the same path)
        if let Some(path) = sel_file {
            self.select_file_by_path(&path);
        }
        self.clamp_cursor();
        if self.follow {
            self.follow_latest();
        }
    }

    fn select_file_by_path(&mut self, path: &str) {
        let rows = self.rows();
        if let Some(i) = rows.iter().position(|r| {
            r.file_idx
                .map(|fi| {
                    self.cur_wt()
                        .map(|w| w.files[fi].path == path)
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        }) {
            if let Some(ui) = self.cur_ui_mut() {
                ui.cursor = i;
            }
        }
    }

    /// Follow mode: move the cursor to the most recently modified file and push it to the opener
    fn follow_latest(&mut self) {
        // Move the history selection to Current (or All when empty)
        let (cur_nonempty, hist) = {
            let Some(wt) = self.cur_wt() else { return };
            let mb = wt.merge_base.clone().unwrap_or_else(|| "HEAD".into());
            let cur = git::changed_files(&wt.info.path, &DiffSource::Current, &mb);
            (
                !cur.is_empty(),
                self.cur_ui().map(|u| u.hist_sel).unwrap_or(0),
            )
        };
        let want = if cur_nonempty { 1 } else { 0 };
        if hist != want {
            self.set_hist(want);
        }
        // Find the file with the newest mtime
        let Some(wt) = self.cur_wt() else { return };
        let wt_path = wt.info.path.clone();
        let mut latest: Option<(String, std::time::SystemTime)> = None;
        for f in &wt.files {
            if f.status == FileStatus::Deleted {
                continue;
            }
            if let Ok(meta) = std::fs::metadata(wt_path.join(&f.path)) {
                if let Ok(m) = meta.modified() {
                    if latest.as_ref().map(|(_, t)| m > *t).unwrap_or(true) {
                        latest = Some((f.path.clone(), m));
                    }
                }
            }
        }
        let Some((path, mtime)) = latest else { return };
        self.select_file_by_path(&path);
        self.clamp_cursor();
        let abs = wt_path.join(&path);
        let changed = self
            .last_followed
            .as_ref()
            .map(|(p, t)| *p != abs || mtime > *t)
            .unwrap_or(true);
        if changed {
            self.last_followed = Some((abs, mtime));
            self.open_selected(None, false);
        }
    }

    // ---- Accessors --------------------------------------------------------

    pub fn cur_wt(&self) -> Option<&WtData> {
        self.wts.get(self.sel_wt)
    }

    pub fn cur_ui(&self) -> Option<&WtUi> {
        self.cur_wt().and_then(|w| self.ui_state.get(&w.info.path))
    }

    pub fn cur_ui_mut(&mut self) -> Option<&mut WtUi> {
        let path = self.cur_wt()?.info.path.clone();
        self.ui_state.get_mut(&path)
    }

    pub fn hist_len(&self) -> usize {
        2 + self.cur_wt().map(|w| w.commits.len()).unwrap_or(0)
    }

    pub fn source(&self) -> DiffSource {
        let hist = self.cur_ui().map(|u| u.hist_sel).unwrap_or(0);
        source_of(
            hist,
            self.cur_wt().map(|w| w.commits.as_slice()).unwrap_or(&[]),
        )
    }

    pub fn rows(&self) -> Vec<Row> {
        let Some(wt) = self.cur_wt() else {
            return Vec::new();
        };
        let empty = HashSet::new();
        let collapsed = self.cur_ui().map(|u| &u.collapsed).unwrap_or(&empty);
        if self.flat {
            flat_rows(&wt.files, &self.filter)
        } else {
            tree_rows(&wt.files, collapsed, &self.filter)
        }
    }

    pub fn selected_file(&self) -> Option<&FileChange> {
        let rows = self.rows();
        let cursor = self.cur_ui()?.cursor;
        let idx = rows.get(cursor)?.file_idx?;
        self.cur_wt().map(|w| &w.files[idx])
    }

    /// DiffDoc for the selected file (LRU-cached).
    /// On a cache miss, a plain version is built and returned immediately (a few ms); the
    /// highlighted version is requested from the worker and swapped in via poll_async
    /// once done. Never blocks the render path.
    pub fn doc(&mut self) -> Option<Rc<DiffDoc>> {
        let wt_path = self.cur_wt()?.info.path.clone();
        let source = self.source();
        let file = self.selected_file()?.clone();
        // Cache freshness is judged by things an edit can change. Keying on gen (which grows
        // with every refresh) would rebuild every 2s even at rest, causing periodic freezes on
        // huge files. Instead the key uses the file's mtime and changed line counts, so it
        // only rebuilds when the file was actually edited.
        let mb = self
            .cur_wt()
            .and_then(|w| w.merge_base.clone())
            .unwrap_or_else(|| "HEAD".into());
        let fp = self.file_fingerprint(&wt_path, &file);
        let key: DocKey = (
            wt_path.clone(),
            source.clone(),
            file.path.clone(),
            mb.clone(),
            fp,
        );
        // Reset scroll when the displayed target changes (file switch or edit)
        let selection_changed = self.cur_doc_key.as_ref() != Some(&key);
        if selection_changed {
            self.cur_doc_key = Some(key.clone());
            self.diff_scroll = 0;
        }
        if let Some(i) = self.docs.iter().position(|e| e.key == key) {
            let entry = self.docs.remove(i);
            self.docs.insert(0, entry);
            let doc = self.docs[0].doc.clone();
            self.cur_doc_gen = self.docs[0].gen;
            // If highlighting is unfinished (the request was overwritten in the latest-wins
            // slot), re-request only when the file is selected again (not every frame)
            if selection_changed && !self.docs[0].hl_done {
                let text = git::diff_text(&wt_path, &source, &file, &mb);
                self.hl_slot.put(HlReq {
                    key,
                    text,
                    path: file.path.clone(),
                });
            }
            return Some(doc);
        }
        let changed = file.added.unwrap_or(0) + file.deleted.unwrap_or(0);
        let want_hl = changed <= HL_MAX_LINES;
        let text = git::diff_text(&wt_path, &source, &file, &mb);
        let doc = build_doc(
            &text,
            &file.path,
            if want_hl { Hl::Pending } else { Hl::Off },
        );
        if want_hl {
            self.hl_slot.put(HlReq {
                key: key.clone(),
                text,
                path: file.path.clone(),
            });
        }
        let rc = Rc::new(doc);
        let gen = self.bump_doc_gen();
        self.cur_doc_gen = gen;
        self.docs.insert(
            0,
            DocEntry {
                key,
                doc: rc.clone(),
                hl_done: !want_hl,
                gen,
            },
        );
        self.docs.truncate(DOC_CACHE);
        Some(rc)
    }

    fn bump_doc_gen(&mut self) -> u64 {
        let g = self.next_doc_gen;
        self.next_doc_gen += 1;
        g
    }

    /// Receive highlighted documents (called on every main-loop iteration).
    /// Plain and highlighted versions have identical row heights, so the scroll position is
    /// preserved (the generation advances, so the renderer's layout table is recomputed,
    /// but with identical values).
    pub fn poll_async(&mut self) {
        while let Ok(resp) = self.hl_rx.try_recv() {
            let gen = self.bump_doc_gen();
            if let Some(entry) = self.docs.iter_mut().find(|e| e.key == resp.key) {
                entry.doc = Rc::new(resp.doc);
                entry.hl_done = true;
                entry.gen = gen;
            }
        }
    }

    /// Fingerprint for cache freshness (mtime in ns, +/- line counts, status, rename source)
    fn file_fingerprint(&self, wt_path: &std::path::Path, file: &FileChange) -> u128 {
        use std::hash::{Hash, Hasher};
        let mtime = std::fs::metadata(wt_path.join(&file.path))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let counts = (file.added.unwrap_or(0) as u128) << 20 | file.deleted.unwrap_or(0) as u128;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        file.status.badge().hash(&mut h);
        file.old_path.hash(&mut h);
        mtime ^ (counts << 96) ^ ((h.finish() as u128) << 32)
    }

    // ---- Selection changes ------------------------------------------------

    pub fn set_wt(&mut self, i: usize) {
        if i < self.wts.len() && i != self.sel_wt {
            self.sel_wt = i;
            self.diff_scroll = 0;
            self.clamp_cursor();
        }
    }

    pub fn set_hist(&mut self, idx: usize) {
        let len = self.hist_len();
        let idx = idx.min(len.saturating_sub(1));
        let (wt_path, mb, commits_snapshot) = {
            let Some(wt) = self.cur_wt() else { return };
            (
                wt.info.path.clone(),
                wt.merge_base.clone().unwrap_or_else(|| "HEAD".into()),
                wt.commits.clone(),
            )
        };
        if let Some(ui) = self.ui_state.get_mut(&wt_path) {
            if ui.hist_sel == idx {
                return;
            }
            ui.hist_sel = idx;
        }
        let source = source_of(idx, &commits_snapshot);
        let files = git::changed_files(&wt_path, &source, &mb);
        if let Some(wt) = self.wts.get_mut(self.sel_wt) {
            wt.files = files;
        }
        self.diff_scroll = 0;
        self.clamp_cursor();
    }

    fn clamp_cursor(&mut self) {
        let len = self.rows().len();
        if let Some(ui) = self.cur_ui_mut() {
            if len == 0 {
                ui.cursor = 0;
            } else if ui.cursor >= len {
                ui.cursor = len - 1;
            }
        }
    }

    fn move_cursor(&mut self, delta: i64) {
        let len = self.rows().len() as i64;
        if len == 0 {
            return;
        }
        if let Some(ui) = self.cur_ui_mut() {
            let cur = ui.cursor as i64 + delta;
            ui.cursor = cur.clamp(0, len - 1) as usize;
        }
    }

    /// Step to the next/previous file from the diff (skim without going back to the tree)
    fn step_file(&mut self, forward: bool) {
        let rows = self.rows();
        let Some(ui) = self.cur_ui() else { return };
        let mut i = ui.cursor as i64;
        let len = rows.len() as i64;
        loop {
            i += if forward { 1 } else { -1 };
            if i < 0 || i >= len {
                return;
            }
            if rows[i as usize].file_idx.is_some() {
                break;
            }
        }
        if let Some(ui) = self.cur_ui_mut() {
            ui.cursor = i as usize;
        }
        self.diff_scroll = 0;
    }

    // ---- opener -----------------------------------------------------------

    pub fn opener_names(&self) -> Vec<String> {
        self.cfg.open.openers.keys().cloned().collect()
    }

    pub fn open_selected(&mut self, name: Option<&str>, report_ok: bool) {
        let Some(wt) = self.cur_wt() else { return };
        let wt_path = wt.info.path.clone();
        let Some(file) = self.selected_file().cloned() else {
            self.set_status("no file selected");
            return;
        };
        if file.status == FileStatus::Deleted {
            self.set_status("cannot open a deleted file");
            return;
        }
        let line = self.doc().and_then(|d| d.first_new_line);
        let opener = match name {
            Some(n) => self.cfg.open.openers.get(n).cloned(),
            None => self.cfg.default_opener().cloned(),
        };
        let Some(op) = opener else {
            self.set_status("no opener configured (set [open] in ~/.config/wtd/config.toml)");
            return;
        };
        let abs = wt_path.join(&file.path);
        let ctx = OpenCtx {
            file: &abs,
            line,
            worktree: &wt_path,
            repo: &self.repo,
        };
        match opener::spawn(&op, &ctx) {
            Ok(()) => {
                if report_ok {
                    self.set_status(format!("opened: {}", file.path));
                }
            }
            Err(e) => self.set_status(e),
        }
    }

    /// Open with the OS default application (O key)
    pub fn open_system(&mut self) {
        let Some(wt) = self.cur_wt() else { return };
        let wt_path = wt.info.path.clone();
        let Some(file) = self.selected_file().cloned() else {
            self.set_status("no file selected");
            return;
        };
        if file.status == FileStatus::Deleted {
            self.set_status("cannot open a deleted file");
            return;
        }
        let abs = wt_path.join(&file.path);
        match opener::spawn_system(&abs) {
            Ok(()) => self.set_status(format!("opened with system app: {}", file.path)),
            Err(e) => self.set_status(e),
        }
    }

    // ---- Key handling -----------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        // Clear any stale status message on new input
        self.status = None;
        // Filter input mode
        if self.filter_input {
            match key.code {
                KeyCode::Enter => self.filter_input = false,
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filter_input = false;
                    self.clamp_cursor();
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                    self.clamp_cursor();
                }
                KeyCode::Char(c) => {
                    self.filter.push(c);
                    self.clamp_cursor();
                }
                _ => {}
            }
            return;
        }
        // Opener picker
        if let Some(idx) = self.picker {
            let names = self.opener_names();
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.picker = Some((idx + 1).min(names.len().saturating_sub(1)));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.picker = Some(idx.saturating_sub(1));
                }
                KeyCode::Enter => {
                    self.picker = None;
                    if let Some(n) = names.get(idx).cloned() {
                        self.open_selected(Some(&n), true);
                    }
                }
                KeyCode::Esc | KeyCode::Char('E') | KeyCode::Char('q') => self.picker = None,
                _ => {}
            }
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('c'), true) | (KeyCode::Char('q'), false) => {
                self.quit = true;
                return;
            }
            (KeyCode::Char('j'), true) => {
                self.focus = match self.focus {
                    Area::Worktree => Area::History,
                    Area::History => Area::Tree,
                    Area::Tree => Area::Diff,
                    Area::Diff => Area::Diff,
                };
                self.sync_narrow_diff();
                return;
            }
            (KeyCode::Char('k'), true) => {
                self.focus = match self.focus {
                    Area::Worktree => Area::Worktree,
                    Area::History => Area::Worktree,
                    Area::Tree => Area::History,
                    Area::Diff => Area::Tree,
                };
                self.sync_narrow_diff();
                return;
            }
            (KeyCode::Char('l'), true) => {
                self.want_clear = true;
                return;
            }
            _ => {}
        }

        match key.code {
            KeyCode::Char(c @ '1'..='9') => {
                let i = c as usize - '1' as usize;
                self.set_wt(i);
                return;
            }
            // {} cycles worktrees, [] cycles history; neither moves focus
            // (for peeking at a neighboring worktree while reading a diff; unlike 1-9, reaches the 10th and beyond)
            KeyCode::Char('{') => {
                self.set_wt(self.sel_wt.saturating_sub(1));
                return;
            }
            KeyCode::Char('}') => {
                self.set_wt((self.sel_wt + 1).min(self.wts.len().saturating_sub(1)));
                return;
            }
            KeyCode::Char('[') => {
                let cur = self.cur_ui().map(|u| u.hist_sel).unwrap_or(0);
                self.set_hist(cur.saturating_sub(1));
                return;
            }
            KeyCode::Char(']') => {
                let cur = self.cur_ui().map(|u| u.hist_sel).unwrap_or(0);
                self.set_hist(cur + 1);
                return;
            }
            KeyCode::Char('s') => {
                self.sxs = !self.sxs;
                return;
            }
            KeyCode::Char('w') => {
                self.wrap = !self.wrap;
                self.set_status(if self.wrap { "wrap: ON" } else { "wrap: OFF" });
                return;
            }
            KeyCode::Char('t') => {
                self.flat = !self.flat;
                self.clamp_cursor();
                return;
            }
            KeyCode::Char('/') => {
                self.filter_input = true;
                return;
            }
            KeyCode::Char('o') => {
                self.open_selected(None, true);
                return;
            }
            KeyCode::Char('O') => {
                self.open_system();
                return;
            }
            KeyCode::Char('E') => {
                if self.opener_names().is_empty() {
                    self.set_status(
                        "no opener configured (set [open] in ~/.config/wtd/config.toml)",
                    );
                } else {
                    self.picker = Some(0);
                }
                return;
            }
            KeyCode::Char('f') => {
                self.follow = !self.follow;
                if self.follow {
                    self.set_status("follow: ON");
                    self.follow_latest();
                } else {
                    self.set_status("follow: OFF");
                }
                return;
            }
            KeyCode::Char('<') => {
                self.split_pct = self.split_pct.saturating_sub(4).max(15);
                return;
            }
            KeyCode::Char('>') => {
                self.split_pct = (self.split_pct + 4).min(85);
                return;
            }
            _ => {}
        }

        match self.focus {
            Area::Worktree => self.key_worktree(key),
            Area::History => self.key_history(key),
            Area::Tree => self.key_tree(key),
            Area::Diff => self.key_diff(key),
        }
    }

    fn sync_narrow_diff(&mut self) {
        self.narrow_diff = self.narrow && self.focus == Area::Diff;
    }

    fn key_worktree(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                self.set_wt(self.sel_wt.saturating_sub(1));
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.set_wt((self.sel_wt + 1).min(self.wts.len().saturating_sub(1)));
            }
            KeyCode::Enter => self.focus = Area::History,
            _ => {}
        }
    }

    fn key_history(&mut self, key: KeyEvent) {
        let cur = self.cur_ui().map(|u| u.hist_sel).unwrap_or(0);
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.set_hist(cur.saturating_sub(1)),
            KeyCode::Char('l') | KeyCode::Right => self.set_hist(cur + 1),
            KeyCode::Enter => self.focus = Area::Tree,
            KeyCode::Esc => self.focus = Area::Worktree,
            _ => {}
        }
    }

    fn key_tree(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_cursor(1);
                self.diff_scroll = 0;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_cursor(-1);
                self.diff_scroll = 0;
            }
            KeyCode::Char('h') | KeyCode::Left => {
                let rows = self.rows();
                let Some(ui) = self.cur_ui() else { return };
                let cursor = ui.cursor;
                let Some(row) = rows.get(cursor) else { return };
                if let Some(dir) = row.dir_path.clone() {
                    if row.expanded {
                        if let Some(ui) = self.cur_ui_mut() {
                            ui.collapsed.insert(dir);
                        }
                        self.clamp_cursor();
                        return;
                    }
                }
                // Jump to the parent directory row
                let depth = row.depth;
                if depth > 0 {
                    for i in (0..cursor).rev() {
                        if rows[i].depth < depth {
                            if let Some(ui) = self.cur_ui_mut() {
                                ui.cursor = i;
                            }
                            return;
                        }
                    }
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let rows = self.rows();
                let Some(ui) = self.cur_ui() else { return };
                let Some(row) = rows.get(ui.cursor) else {
                    return;
                };
                if let Some(dir) = row.dir_path.clone() {
                    if !row.expanded {
                        if let Some(ui) = self.cur_ui_mut() {
                            ui.collapsed.remove(&dir);
                        }
                    }
                } else if row.file_idx.is_some() {
                    self.focus = Area::Diff;
                    self.sync_narrow_diff();
                }
            }
            KeyCode::Enter => {
                let rows = self.rows();
                let Some(ui) = self.cur_ui() else { return };
                let Some(row) = rows.get(ui.cursor) else {
                    return;
                };
                if let Some(dir) = row.dir_path.clone() {
                    let expanded = row.expanded;
                    if let Some(ui) = self.cur_ui_mut() {
                        if expanded {
                            ui.collapsed.insert(dir);
                        } else {
                            ui.collapsed.remove(&dir);
                        }
                    }
                    self.clamp_cursor();
                } else if row.file_idx.is_some() {
                    self.focus = Area::Diff;
                    self.sync_narrow_diff();
                }
            }
            KeyCode::Esc => self.focus = Area::History,
            _ => {}
        }
    }

    fn key_diff(&mut self, key: KeyEvent) {
        // In physical lines (screen rows after wrapping); the limit comes from the renderer.
        // If past the limit (e.g. right after a hunk jump), take max with the current position so forward keys don't move backward
        let max_scroll = self.diff_max_scroll.max(self.diff_scroll);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.diff_scroll = (self.diff_scroll + 1).min(max_scroll);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.diff_scroll = self.diff_scroll.saturating_sub(1);
            }
            KeyCode::PageDown | KeyCode::Char('d') => {
                self.diff_scroll = (self.diff_scroll + self.diff_height / 2).min(max_scroll);
            }
            KeyCode::PageUp | KeyCode::Char('u') => {
                self.diff_scroll = self.diff_scroll.saturating_sub(self.diff_height / 2);
            }
            KeyCode::Char('n') => self.jump_hunk(true),
            KeyCode::Char('p') => self.jump_hunk(false),
            KeyCode::Char('J') => self.step_file(true),
            KeyCode::Char('K') => self.step_file(false),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Esc => {
                self.focus = Area::Tree;
                self.sync_narrow_diff();
            }
            _ => {}
        }
    }

    /// Logical row index -> physical line (via renderer-maintained row_offsets; identity if not built yet)
    fn phys_of(&self, row: usize) -> usize {
        self.row_offsets.get(row).copied().unwrap_or(row)
    }

    fn jump_hunk(&mut self, forward: bool) {
        let sxs = self.effective_sxs;
        let scroll = self.diff_scroll;
        let Some(doc) = self.doc() else { return };
        let hunks = if sxs { &doc.hunks_s } else { &doc.hunks_u };
        if hunks.is_empty() {
            return;
        }
        let next = if forward {
            hunks.iter().map(|&h| self.phys_of(h)).find(|&p| p > scroll)
        } else {
            hunks
                .iter()
                .rev()
                .map(|&h| self.phys_of(h))
                .find(|&p| p < scroll)
        };
        if let Some(p) = next {
            self.diff_scroll = p;
        }
    }

    /// Current hunk position (1-based, total)
    pub fn hunk_pos(&mut self) -> Option<(usize, usize)> {
        let sxs = self.effective_sxs;
        let scroll = self.diff_scroll;
        let doc = self.doc()?;
        let hunks = if sxs { &doc.hunks_s } else { &doc.hunks_u };
        if hunks.is_empty() {
            return None;
        }
        let cur = hunks
            .iter()
            .filter(|&&h| self.phys_of(h) <= scroll)
            .count()
            .max(1);
        Some((cur, hunks.len()))
    }
}

pub fn source_of(hist_sel: usize, commits: &[git::CommitInfo]) -> DiffSource {
    match hist_sel {
        0 => DiffSource::All,
        1 => DiffSource::Current,
        n => commits
            .get(n - 2)
            .map(|c| DiffSource::Commit(c.sha.clone()))
            .unwrap_or(DiffSource::All),
    }
}

/// Elapsed time in "2m ago" form
pub fn fmt_age(unix: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = (now - unix).max(0);
    if d < 60 {
        format!("{d}s ago")
    } else if d < 3600 {
        format!("{}m ago", d / 60)
    } else if d < 86400 {
        format!("{}h ago", d / 3600)
    } else {
        format!("{}d ago", d / 86400)
    }
}
