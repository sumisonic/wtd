use crate::app::{fmt_age, App, Area};
use crate::diffview::{Half, SRow, Seg, URow, FG_HUNK};
use crate::git::FileStatus;
use crate::icons;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Dracula-leaning palette (kept minimal so it blends with terminal themes)
const ACCENT: Color = Color::Rgb(0xbd, 0x93, 0xf9);
const ACCENT_FG: Color = Color::Rgb(0x28, 0x2a, 0x36);
const INACTIVE_SEL: Color = Color::Rgb(0x62, 0x72, 0xa4);
const FG_MAIN: Color = Color::Rgb(0xf8, 0xf8, 0xf2);
const FG_DIM: Color = Color::Rgb(0x9a, 0x9d, 0xb2);
const CURSOR_BG_UNFOCUS: Color = Color::Rgb(0x44, 0x47, 0x5a);
const GREEN: Color = Color::Rgb(0x50, 0xfa, 0x7b);
const RED: Color = Color::Rgb(0xff, 0x55, 0x55);
const YELLOW: Color = Color::Rgb(0xf1, 0xfa, 0x8c);
const MAGENTA: Color = Color::Rgb(0xff, 0x79, 0xc6);
const CYAN: Color = Color::Rgb(0x8b, 0xe9, 0xfd);

fn badge_style(status: FileStatus) -> (char, Style) {
    let c = status.badge();
    let color = match status {
        FileStatus::Modified => YELLOW,
        FileStatus::Added => GREEN,
        FileStatus::Deleted => RED,
        FileStatus::Renamed => MAGENTA,
        FileStatus::Untracked => CYAN,
        FileStatus::Other => FG_DIM,
    };
    (c, Style::default().fg(color))
}

pub fn render(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(1), // worktree bar
        Constraint::Length(1), // history bar
        Constraint::Min(3),    // body
        Constraint::Length(1), // help
    ])
    .split(area);

    app.narrow = area.width < app.cfg.general.narrow_cols;
    if !app.narrow {
        app.narrow_diff = false;
    }

    render_header(f, chunks[0], app);
    render_wtbar(f, chunks[1], app);
    render_histbar(f, chunks[2], app);
    render_body(f, chunks[3], app);
    render_help(f, chunks[4], app);
    render_picker(f, area, app);
}

fn render_header(f: &mut Frame, rect: Rect, app: &App) {
    let mut right = format!("base: {}", app.base);
    if app.follow {
        right = format!("● follow  {right}");
    }
    let rw = right.width() as u16 + 1;
    let parts = Layout::horizontal([Constraint::Min(0), Constraint::Length(rw)]).split(rect);
    let title = Line::from(vec![
        Span::styled(
            " wtd ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("— {}", app.repo_name), Style::default().fg(FG_MAIN)),
    ]);
    f.render_widget(Paragraph::new(title), parts[0]);
    let style = if app.follow {
        Style::default().fg(MAGENTA)
    } else {
        Style::default().fg(FG_DIM)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(right, style))).alignment(Alignment::Right),
        parts[1],
    );
}

/// Tab bar (shared by worktree / history). Trims items from the front so the selection stays visible.
fn render_bar(
    f: &mut Frame,
    rect: Rect,
    label: &str,
    items: Vec<Line<'static>>,
    sel: usize,
    focused: bool,
) {
    let label_style = if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(FG_DIM)
    };
    let label_span = Span::styled(format!(" {label} "), label_style);
    let avail = rect.width.saturating_sub(label_span.width() as u16) as usize;
    // Drop leading items until the selection fits
    let widths: Vec<usize> = items.iter().map(|l| l.width() + 1).collect();
    let mut start = 0usize;
    loop {
        let w: usize = widths[start..=sel.min(widths.len().saturating_sub(1))]
            .iter()
            .sum();
        if w <= avail || start >= sel {
            break;
        }
        start += 1;
    }
    let mut spans: Vec<Span> = vec![label_span];
    if start > 0 {
        spans.push(Span::styled("… ", Style::default().fg(FG_DIM)));
    }
    for line in items.into_iter().skip(start) {
        spans.extend(line.spans);
        spans.push(Span::raw(" "));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), rect);
}

fn tab_style(selected: bool, focused: bool) -> Style {
    if selected && focused {
        Style::default()
            .bg(ACCENT)
            .fg(ACCENT_FG)
            .add_modifier(Modifier::BOLD)
    } else if selected {
        Style::default().bg(INACTIVE_SEL).fg(FG_MAIN)
    } else {
        Style::default().fg(FG_DIM)
    }
}

fn render_wtbar(f: &mut Frame, rect: Rect, app: &App) {
    let focused = app.focus == Area::Worktree;
    let items: Vec<Line<'static>> = app
        .wts
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            let selected = i == app.sel_wt;
            let st = tab_style(selected, focused);
            let (a, d) = wt.all_totals;
            let mut spans = vec![
                Span::styled(
                    format!(" {} ", i + 1),
                    st.fg(if selected {
                        st.fg.unwrap_or(FG_DIM)
                    } else {
                        FG_DIM
                    }),
                ),
                Span::styled(wt.info.branch.clone(), st),
            ];
            if a == 0 && d == 0 {
                spans.push(Span::styled(
                    " ✓ ",
                    st.fg(if selected {
                        st.fg.unwrap_or(GREEN)
                    } else {
                        GREEN
                    }),
                ));
            } else {
                spans.push(Span::styled(
                    format!(" +{a}"),
                    if selected { st } else { st.fg(GREEN) },
                ));
                spans.push(Span::styled(
                    format!(" -{d} "),
                    if selected { st } else { st.fg(RED) },
                ));
            }
            Line::from(spans)
        })
        .collect();
    render_bar(f, rect, "worktree:", items, app.sel_wt, focused);
}

fn render_histbar(f: &mut Frame, rect: Rect, app: &App) {
    let focused = app.focus == Area::History;
    let hist_sel = app.cur_ui().map(|u| u.hist_sel).unwrap_or(0);
    let commits = app.cur_wt().map(|w| w.commits.clone()).unwrap_or_default();
    let n = commits.len();
    let mut labels: Vec<String> = vec!["All".into(), "Current".into()];
    for i in 0..n {
        labels.push(format!("c{}", n - i));
    }
    let items: Vec<Line<'static>> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| {
            Line::from(Span::styled(
                format!(" {l} "),
                tab_style(i == hist_sel, focused),
            ))
        })
        .collect();

    // Right-hand summary
    let files = app.cur_wt().map(|w| w.files.as_slice()).unwrap_or(&[]);
    let (a, d) = crate::git::totals(files);
    let mut summary = format!("{} files +{a} -{d}", files.len());
    if hist_sel >= 2 {
        if let Some(c) = commits.get(hist_sel - 2) {
            let subj: String = c.subject.chars().take(40).collect();
            summary = format!("{} \"{}\" {} · {}", c.short, subj, summary, fmt_age(c.time));
        }
    } else if let Some(c) = commits.first() {
        summary = format!("{summary} · commit {}", fmt_age(c.time));
    }
    let sw = (summary.width() as u16 + 1).min(rect.width / 2);
    let parts = Layout::horizontal([Constraint::Min(0), Constraint::Length(sw)]).split(rect);
    render_bar(f, parts[0], "history:", items, hist_sel, focused);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            summary,
            Style::default().fg(FG_DIM),
        )))
        .alignment(Alignment::Right),
        parts[1],
    );
}

fn render_body(f: &mut Frame, rect: Rect, app: &mut App) {
    if app.narrow {
        if app.narrow_diff {
            render_diff_pane(f, rect, app);
        } else {
            app.diff_height = rect.height.saturating_sub(3) as usize;
            render_tree_pane(f, rect, app);
        }
        return;
    }
    let parts = Layout::horizontal([Constraint::Percentage(app.split_pct), Constraint::Min(10)])
        .split(rect);
    render_tree_pane(f, parts[0], app);
    render_diff_pane(f, parts[1], app);
}

fn render_tree_pane(f: &mut Frame, rect: Rect, app: &mut App) {
    let focused = app.focus == Area::Tree;
    let border = if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(INACTIVE_SEL)
    };
    let title = if app.flat {
        " files (flat) "
    } else {
        " files "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(title);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let rows = app.rows();
    let cursor = app.cur_ui().map(|u| u.cursor).unwrap_or(0);
    let h = inner.height as usize;
    app.tree_height = h;
    // Scroll adjustment
    let scroll = {
        let ui = match app.cur_ui_mut() {
            Some(u) => u,
            None => return,
        };
        if ui.cursor < ui.scroll {
            ui.scroll = ui.cursor;
        }
        if h > 0 && ui.cursor >= ui.scroll + h {
            ui.scroll = ui.cursor - h + 1;
        }
        ui.scroll
    };

    if rows.is_empty() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "(no changes)",
                Style::default().fg(FG_DIM),
            ))),
            inner,
        );
        app.diff_height = app.diff_height.max(1);
        return;
    }

    let files = app.cur_wt().map(|w| w.files.clone()).unwrap_or_default();
    let followed = app.follow;
    let icons_on = app.cfg.ui.icons;
    let w = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(scroll).take(h) {
        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::raw(" ".repeat(row.depth * 2 + 1)));
        let is_cursor = i == cursor;
        if let Some(_dir) = &row.dir_path {
            let arrow = if row.expanded { "▾ " } else { "▸ " };
            spans.push(Span::styled(arrow, Style::default().fg(FG_DIM)));
            if icons_on {
                let icon = if row.expanded {
                    icons::DIR_OPEN
                } else {
                    icons::DIR_CLOSED
                };
                spans.push(Span::styled(
                    format!("{icon} "),
                    Style::default().fg(icons::DIR_COLOR),
                ));
            }
            spans.push(Span::styled(
                row.label.clone(),
                Style::default().fg(icons::DIR_COLOR),
            ));
        } else if let Some(fi) = row.file_idx {
            let file = &files[fi];
            let (badge, bstyle) = badge_style(file.status);
            spans.push(Span::styled(format!("{badge} "), bstyle));
            if icons_on {
                let name = row.label.rsplit('/').next().unwrap_or(&row.label);
                let (icon, color) = icons::file_icon(name);
                spans.push(Span::styled(format!("{icon} "), Style::default().fg(color)));
            }
            spans.push(Span::styled(
                row.label.clone(),
                Style::default().fg(FG_MAIN),
            ));
            // Right-aligned +a -d
            let stats = match (file.added, file.deleted) {
                (Some(a), Some(d)) => format!("+{a} -{d}"),
                _ => "Bin".to_string(),
            };
            let used: usize = spans.iter().map(|s| s.width()).sum();
            let marker = if followed && is_cursor {
                " ← now"
            } else {
                ""
            };
            let pad = w.saturating_sub(used + stats.width() + marker.width() + 1);
            if pad > 0 {
                spans.push(Span::raw(" ".repeat(pad)));
                let (a_part, d_part) = stats.split_once(' ').unwrap_or((stats.as_str(), ""));
                spans.push(Span::styled(a_part.to_string(), Style::default().fg(GREEN)));
                if !d_part.is_empty() {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(d_part.to_string(), Style::default().fg(RED)));
                }
                if !marker.is_empty() {
                    spans.push(Span::styled(
                        marker.to_string(),
                        Style::default().fg(MAGENTA),
                    ));
                }
            }
        }
        let mut line = Line::from(spans);
        if is_cursor {
            let bg = if focused { ACCENT } else { CURSOR_BG_UNFOCUS };
            let fg = if focused { ACCENT_FG } else { FG_MAIN };
            line = line.style(Style::default().bg(bg));
            if focused {
                // When focused, shift the cursor row's foreground to the inverted side too
                line.spans = line
                    .spans
                    .into_iter()
                    .map(|mut s| {
                        s.style = s.style.fg(fg);
                        s
                    })
                    .collect();
            }
        }
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn clip_segs(segs: &[Seg], max_w: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for seg in segs {
        if used >= max_w {
            break;
        }
        let sw = seg.text.width();
        if used + sw <= max_w {
            out.push(Span::styled(seg.text.clone(), seg.style));
            used += sw;
        } else {
            let mut acc = String::new();
            for ch in seg.text.chars() {
                let cw = UnicodeWidthStr::width(ch.to_string().as_str());
                if used + cw > max_w {
                    break;
                }
                acc.push(ch);
                used += cw;
            }
            out.push(Span::styled(acc, seg.style));
            break;
        }
    }
    out
}

/// Wrap segs at display width avail, skipping `skip` lines and emitting at most `max` lines of Spans.
/// A 2-cell character that doesn't fit at the end of a line is moved whole to the next line.
/// Skipped regions and lines beyond max are only scanned, never allocated, so even very long
/// lines (minified files etc.) cost one frame only the visible window. Line-break logic must match wrap_height.
fn wrap_segs_window(
    segs: &[Seg],
    avail: usize,
    skip: usize,
    max: usize,
) -> Vec<Vec<Span<'static>>> {
    let avail = avail.max(1);
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    if max == 0 {
        return lines;
    }
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    let mut line_no = 0usize; // physical line currently being built
    for seg in segs {
        let mut acc = String::new();
        for ch in seg.text.chars() {
            let cw = ch.width().unwrap_or(0);
            if used > 0 && used + cw > avail {
                // Finalize the line
                if line_no >= skip {
                    if !acc.is_empty() {
                        cur.push(Span::styled(std::mem::take(&mut acc), seg.style));
                    }
                    lines.push(std::mem::take(&mut cur));
                    if lines.len() >= max {
                        return lines;
                    }
                }
                line_no += 1;
                used = 0;
            }
            if line_no >= skip {
                acc.push(ch);
            }
            used += cw;
        }
        if line_no >= skip && !acc.is_empty() {
            cur.push(Span::styled(acc, seg.style));
        }
    }
    if line_no >= skip {
        lines.push(cur);
    }
    lines
}

/// Count only the total lines wrap_segs_window would emit (for building the layout table; no allocation).
/// Wrapping logic must be identical to wrap_segs_window.
fn wrap_height(segs: &[Seg], avail: usize) -> usize {
    let avail = avail.max(1);
    let mut lines = 1usize;
    let mut used = 0usize;
    for seg in segs {
        for ch in seg.text.chars() {
            let cw = ch.width().unwrap_or(0);
            if used > 0 && used + cw > avail {
                lines += 1;
                used = 0;
            }
            used += cw;
        }
    }
    lines
}

/// Prefix-sum from unified logical row index to physical start line (length rows+1; last entry is the physical total).
/// Per-row height logic must match the renderer's wrap_segs / clip_segs.
fn build_offsets_uni(rows: &[URow], wrap: bool, content_w: usize) -> Vec<usize> {
    let mut offs = Vec::with_capacity(rows.len() + 1);
    let mut acc = 0usize;
    for row in rows {
        offs.push(acc);
        acc += if row.is_hunk || !wrap {
            1
        } else {
            wrap_height(&row.segs, content_w)
        };
    }
    offs.push(acc);
    offs
}

/// Side-by-side variant. A logical row's height is the larger of the two halves' wrapped heights
fn build_offsets_sxs(rows: &[SRow], wrap: bool, half: usize) -> Vec<usize> {
    let h = |hd: &Option<Half>| -> usize {
        match hd {
            Some(hd) if wrap => wrap_height(&hd.segs, half),
            _ => 1,
        }
    };
    let mut offs = Vec::with_capacity(rows.len() + 1);
    let mut acc = 0usize;
    for row in rows {
        offs.push(acc);
        acc += if row.header.is_some() || !wrap {
            1
        } else {
            h(&row.left).max(h(&row.right))
        };
    }
    offs.push(acc);
    offs
}

fn pad_to(spans: &mut Vec<Span<'static>>, width: usize, bg: Option<Color>) {
    let used: usize = spans.iter().map(|s| s.width()).sum();
    if used < width {
        let style = bg.map(|b| Style::default().bg(b)).unwrap_or_default();
        spans.push(Span::styled(" ".repeat(width - used), style));
    }
}

fn render_diff_pane(f: &mut Frame, rect: Rect, app: &mut App) {
    let focused = app.focus == Area::Diff;
    let border = if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(INACTIVE_SEL)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .title(" diff ");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    if inner.height < 2 {
        return;
    }

    let w = inner.width as usize;
    app.effective_sxs = app.sxs && w >= 90;
    let eff_sxs = app.effective_sxs;
    // First line is the file header; doc.note takes one more line (adjusted after fetching the doc)
    let mut body_h = inner.height as usize - 1;
    app.diff_height = body_h;

    // With no file selected (e.g. on a directory), show the worktree change summary
    if app.selected_file().is_none() {
        let mut lines: Vec<Line> = Vec::new();
        if let Some(wt) = app.cur_wt() {
            let (a, d) = crate::git::totals(&wt.files);
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {} ", wt.info.branch),
                    Style::default().fg(FG_MAIN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} files  +{a} -{d}", wt.files.len()),
                    Style::default().fg(FG_DIM),
                ),
            ]));
            lines.push(Line::default());
            for file in wt.files.iter().take(body_h.saturating_sub(2)) {
                let (badge, bstyle) = badge_style(file.status);
                let stats = match (file.added, file.deleted) {
                    (Some(a), Some(d)) => format!("  +{a} -{d}"),
                    _ => "  Bin".into(),
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {badge} "), bstyle),
                    Span::styled(file.path.clone(), Style::default().fg(FG_MAIN)),
                    Span::styled(stats, Style::default().fg(FG_DIM)),
                ]));
            }
            if wt.files.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  no changes",
                    Style::default().fg(FG_DIM),
                )));
            }
        }
        app.diff_max_scroll = 0;
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    // Header line contents
    let header_line: Line = match app.selected_file() {
        Some(file) => {
            let (badge, bstyle) = badge_style(file.status);
            let stats = match (file.added, file.deleted) {
                (Some(a), Some(d)) => format!("  +{a} -{d}"),
                _ => "  Bin".into(),
            };
            let mut spans = vec![
                Span::styled(format!(" {badge} "), bstyle.add_modifier(Modifier::BOLD)),
                Span::styled(
                    file.path.clone(),
                    Style::default().fg(FG_MAIN).add_modifier(Modifier::BOLD),
                ),
                Span::styled(stats, Style::default().fg(FG_DIM)),
            ];
            if let Some(old) = &file.old_path {
                spans.insert(
                    2,
                    Span::styled(format!(" (← {old})"), Style::default().fg(FG_DIM)),
                );
            }
            Line::from(spans)
        }
        None => Line::from(Span::styled(
            " (no file selected)",
            Style::default().fg(FG_DIM),
        )),
    };

    let mut lines: Vec<Line> = vec![header_line];

    // Build the DiffDoc (cached) and turn only the visible part into Lines
    let wrap = app.wrap;
    let mut phys_total = 0usize;
    let mut hunk_note: Option<String> = None;
    if app.selected_file().is_some() {
        if let Some(doc) = app.doc() {
            if let Some(note) = &doc.note {
                lines.push(Line::from(Span::styled(
                    format!(" {note}"),
                    Style::default().fg(FG_DIM),
                )));
                // The note consumes one body line, so shrink the limit and visible line count by one
                body_h = body_h.saturating_sub(1);
                app.diff_height = body_h;
            }
            let half = (w.saturating_sub(7 * 2 + 1)) / 2;
            let content_w = w.saturating_sub(12);
            // Recompute the logical->physical line table only when doc, width, wrap, or layout changes.
            // Doc identity is judged by generation (Rc pointer values can falsely match via address reuse)
            let lkey = (app.cur_doc_gen, w as u16, wrap, eff_sxs);
            if app.layout_key != Some(lkey) {
                let offsets = if eff_sxs {
                    build_offsets_sxs(&doc.sxs, wrap, half)
                } else {
                    build_offsets_uni(&doc.unified, wrap, content_w)
                };
                // If only width or wrap changed for the same doc, map the scroll position
                // onto the new layout, keeping the visible logical row. Unified and
                // side-by-side have different logical row spaces (del/add pairing or not),
                // so when eff_sxs flips, skip the mapping and let clamping handle it
                if let Some(old) = app.layout_key {
                    if old.0 == lkey.0 && old.3 == lkey.3 && app.row_offsets.len() > 1 {
                        let starts = &app.row_offsets[..app.row_offsets.len() - 1];
                        let r = starts
                            .partition_point(|&o| o <= app.diff_scroll)
                            .saturating_sub(1);
                        app.diff_scroll = offsets.get(r).copied().unwrap_or(0);
                    }
                }
                app.row_offsets = offsets;
                app.layout_key = Some(lkey);
            }
            phys_total = app.row_offsets.last().copied().unwrap_or(0);
            // Physical scroll position -> starting logical row and intra-row skip
            let scroll = app.diff_scroll.min(phys_total.saturating_sub(1));
            let starts = &app.row_offsets[..app.row_offsets.len().saturating_sub(1)];
            let start_row = starts.partition_point(|&o| o <= scroll).saturating_sub(1);
            let mut skip = scroll.saturating_sub(starts.get(start_row).copied().unwrap_or(0));
            let mut emitted = 0usize;
            if eff_sxs {
                'sxs: for row in doc.sxs.iter().skip(start_row) {
                    if emitted >= body_h {
                        break;
                    }
                    // Intra-row skip applies only to the first logical row (0 afterwards)
                    let first = skip;
                    skip = 0;
                    if let Some(h) = &row.header {
                        lines.push(Line::from(Span::styled(
                            h.clone(),
                            Style::default().fg(FG_HUNK),
                        )));
                        emitted += 1;
                        continue;
                    }
                    // Screen lines of the visible window for each half (one clipped line when wrap is off).
                    // Nothing outside the window is generated, so even huge lines have bounded per-frame cost
                    let take = body_h - emitted;
                    let half_lines = |hd: &Option<Half>| -> Vec<Vec<Span<'static>>> {
                        match hd {
                            Some(hd) if wrap => wrap_segs_window(&hd.segs, half, first, take),
                            Some(hd) => vec![clip_segs(&hd.segs, half)],
                            None => Vec::new(),
                        }
                    };
                    let lw = half_lines(&row.left);
                    let rw = half_lines(&row.right);
                    let n = lw.len().max(rw.len()).max(1);
                    for j in 0..n {
                        let mut spans: Vec<Span> = Vec::new();
                        for (half_data, wl) in [(&row.left, &lw), (&row.right, &rw)] {
                            match half_data {
                                Some(hd) if j < wl.len() => {
                                    let gstyle = Style::default()
                                        .fg(FG_DIM)
                                        .bg(hd.bg.unwrap_or(Color::Reset));
                                    // Line number only on the first wrapped line; continuations get gutter spaces (background continues)
                                    let gut = if j == 0 && first == 0 {
                                        format!("{:>5} ", hd.no)
                                    } else {
                                        " ".repeat(6)
                                    };
                                    spans.push(Span::styled(gut, gstyle));
                                    let mut segs = wl[j].clone();
                                    pad_to(&mut segs, half, hd.bg);
                                    spans.extend(segs);
                                }
                                Some(hd) => {
                                    // Shorter half after wrapping: pad with +/- background spaces to equalize block height
                                    let style =
                                        hd.bg.map(|b| Style::default().bg(b)).unwrap_or_default();
                                    spans.push(Span::styled(" ".repeat(6 + half), style));
                                }
                                None => {
                                    spans.push(Span::raw(" ".repeat(6 + half)));
                                }
                            }
                            spans.push(Span::styled("│", Style::default().fg(INACTIVE_SEL)));
                        }
                        spans.pop();
                        lines.push(Line::from(spans));
                        emitted += 1;
                        if emitted >= body_h {
                            break 'sxs;
                        }
                    }
                }
            } else {
                'uni: for row in doc.unified.iter().skip(start_row) {
                    if emitted >= body_h {
                        break;
                    }
                    let first = skip;
                    skip = 0;
                    if row.is_hunk {
                        let spans: Vec<Span> = row
                            .segs
                            .iter()
                            .map(|s| Span::styled(s.text.clone(), s.style))
                            .collect();
                        lines.push(Line::from(spans));
                        emitted += 1;
                        continue;
                    }
                    let gstyle = Style::default()
                        .fg(FG_DIM)
                        .bg(row.bg.unwrap_or(Color::Reset));
                    let row_lines = if wrap {
                        wrap_segs_window(&row.segs, content_w, first, body_h - emitted)
                    } else {
                        vec![clip_segs(&row.segs, content_w)]
                    };
                    for (j, mut wl) in row_lines.into_iter().enumerate() {
                        // Line number only on the first wrapped line; continuations get gutter spaces (background continues)
                        let gut = if j == 0 && first == 0 {
                            format!(
                                "{:>5} {:>5} ",
                                row.old_no.map(|n| n.to_string()).unwrap_or_default(),
                                row.new_no.map(|n| n.to_string()).unwrap_or_default(),
                            )
                        } else {
                            " ".repeat(12)
                        };
                        pad_to(&mut wl, content_w, row.bg);
                        let mut spans = vec![Span::styled(gut, gstyle)];
                        spans.append(&mut wl);
                        lines.push(Line::from(spans));
                        emitted += 1;
                        if emitted >= body_h {
                            break 'uni;
                        }
                    }
                }
            }
        }
        // Clamp before counting hunk_pos (avoids a one-frame glitch from out-of-range scroll)
        app.diff_scroll = app.diff_scroll.min(phys_total.saturating_sub(1));
        if let Some((cur, total)) = app.hunk_pos() {
            hunk_note = Some(format!("hunk {cur}/{total}"));
        }
    }
    app.diff_max_scroll = phys_total.saturating_sub(body_h);

    f.render_widget(Paragraph::new(lines), inner);

    if let Some(note) = hunk_note {
        let nw = note.width() as u16 + 2;
        if rect.width > nw + 2 {
            let r = Rect {
                x: rect.x + rect.width - nw - 1,
                y: rect.y + rect.height - 1,
                width: nw,
                height: 1,
            };
            f.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    format!(" {note} "),
                    Style::default().fg(FG_DIM),
                ))),
                r,
            );
        }
    }
}

fn render_help(f: &mut Frame, rect: Rect, app: &App) {
    let text: String = if app.filter_input {
        format!(" filter: {}▌  (Enter:apply Esc:clear)", app.filter)
    } else if let Some((msg, t)) = &app.status {
        if t.elapsed().as_secs() < 3 {
            format!(" {msg}")
        } else {
            help_text(app)
        }
    } else {
        help_text(app)
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text, Style::default().fg(FG_DIM)))),
        rect,
    );
}

fn help_text(app: &App) -> String {
    let base = match app.focus {
        Area::Worktree => "[worktree] h/l:select  Enter:history  1-9:jump",
        Area::History => "[history] h/l:select  Enter:files  Esc:worktree",
        Area::Tree => {
            "[tree] j/k:move  h/l:fold/expand  t:flat  /:filter  o:open  O:system  f:follow"
        }
        Area::Diff => "[diff] j/k:scroll  n/p:hunk  J/K:file  s:layout  w:wrap  h:back",
    };
    format!(" {base}  C-j/C-k:area  {{}}:worktree  []:history  q:quit")
}

fn render_picker(f: &mut Frame, area: Rect, app: &App) {
    let Some(sel) = app.picker else { return };
    let names = app.opener_names();
    if names.is_empty() {
        return;
    }
    let h = (names.len() as u16 + 2).min(area.height);
    let w = 34.min(area.width);
    let rect = Rect {
        x: (area.width.saturating_sub(w)) / 2,
        y: (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(" opener ");
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let default_name = app.cfg.open.default.clone();
    let lines: Vec<Line> = names
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let mark = if *n == default_name { "*" } else { " " };
            let style = if i == sel {
                Style::default()
                    .bg(ACCENT)
                    .fg(ACCENT_FG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(FG_MAIN)
            };
            Line::from(Span::styled(format!(" {mark} {n} "), style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(text: &str) -> Seg {
        Seg {
            style: Style::default(),
            text: text.to_string(),
        }
    }

    fn line_width(spans: &[Span]) -> usize {
        spans.iter().map(|s| s.width()).sum()
    }

    /// Equivalent of the old wrap_segs that emits all lines (test helper)
    fn wrap_segs(segs: &[Seg], avail: usize) -> Vec<Vec<Span<'static>>> {
        wrap_segs_window(segs, avail, 0, usize::MAX)
    }

    fn contents(lines: &[Vec<Span>]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn window_matches_full_slice() {
        // Windowed output (skip/max) matches [skip..skip+max] of the full output
        let segs = vec![
            seg("abcdefghij"),
            seg("日本語のテキスト"),
            seg("0123456789"),
        ];
        for avail in [1usize, 2, 3, 7, 10] {
            let full = wrap_segs(&segs, avail);
            for skip in 0..=full.len() {
                for max in 0..full.len() + 2 {
                    let win = wrap_segs_window(&segs, avail, skip, max);
                    let expect: Vec<String> =
                        contents(&full).into_iter().skip(skip).take(max).collect();
                    assert_eq!(
                        contents(&win),
                        expect,
                        "avail={avail} skip={skip} max={max}"
                    );
                }
            }
        }
    }

    #[test]
    fn wrap_segs_respects_width() {
        for avail in [1usize, 2, 3, 7, 10, 80] {
            for text in [
                "",
                "abc",
                "hello world, this is a long line",
                "日本語のテキストと ascii の混在した行です",
                "ああああああああああ",
            ] {
                let lines = wrap_segs(&[seg(text)], avail);
                for l in &lines {
                    // A 2-cell char physically can't fit in width 1; only then allow overflow
                    // and let the renderer (ratatui) clip it
                    assert!(line_width(l) <= avail.max(2), "avail={avail} text={text:?}");
                }
                // No characters are lost by wrapping
                let joined: String = lines
                    .iter()
                    .flat_map(|l| l.iter().map(|s| s.content.as_ref()))
                    .collect();
                assert_eq!(joined, text);
            }
        }
    }

    #[test]
    fn wrap_height_matches_wrap_segs() {
        let cases: Vec<Vec<Seg>> = vec![
            vec![seg("")],
            vec![seg("short")],
            vec![seg("a"), seg("bcdefghij"), seg("klmno")],
            vec![seg("日本語テキスト"), seg("と ascii "), seg("mixed の行")],
            vec![seg("x".repeat(200).as_str())],
        ];
        for segs in &cases {
            for avail in [1usize, 2, 5, 13, 80] {
                assert_eq!(
                    wrap_height(segs, avail),
                    wrap_segs(segs, avail).len(),
                    "avail={avail}"
                );
            }
        }
    }

    #[test]
    fn wrap_wide_char_moves_to_next_line() {
        // A 2-cell char that doesn't fit in the last cell moves to the next line
        let lines = wrap_segs(&[seg("a漢")], 2);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][0].content.as_ref(), "a");
        assert_eq!(lines[1][0].content.as_ref(), "漢");
    }

    fn urow(text: &str, is_hunk: bool) -> URow {
        URow {
            old_no: None,
            new_no: None,
            segs: vec![seg(text)],
            bg: None,
            is_hunk,
        }
    }

    #[test]
    fn offsets_uni_wrap_off_is_identity() {
        let rows = vec![urow("@@", true), urow("x".repeat(100).as_str(), false)];
        assert_eq!(build_offsets_uni(&rows, false, 10), vec![0, 1, 2]);
    }

    #[test]
    fn offsets_uni_wrap_counts_physical_lines() {
        // 25 chars at width 10 -> 3 physical lines. Hunk rows are always 1 line
        let rows = vec![
            urow("@@ hunk header long", true),
            urow("x".repeat(25).as_str(), false),
            urow("short", false),
        ];
        assert_eq!(build_offsets_uni(&rows, true, 10), vec![0, 1, 4, 5]);
    }

    #[test]
    fn offsets_sxs_takes_max_of_halves() {
        let half = |text: &str| {
            Some(Half {
                no: 1,
                segs: vec![seg(text)],
                bg: None,
            })
        };
        let rows = vec![
            SRow {
                left: None,
                right: None,
                header: Some("@@".into()),
            },
            // left 3 lines, right 1 line -> height 3
            SRow {
                left: half("x".repeat(25).as_str()),
                right: half("y"),
                header: None,
            },
            // one side missing -> height comes from the existing side
            SRow {
                left: None,
                right: half("z".repeat(11).as_str()),
                header: None,
            },
        ];
        assert_eq!(build_offsets_sxs(&rows, true, 10), vec![0, 1, 4, 6]);
        assert_eq!(build_offsets_sxs(&rows, false, 10), vec![0, 1, 2, 3]);
    }
}
