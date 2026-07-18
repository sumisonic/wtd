use ratatui::style::{Color, Modifier, Style};
use similar::{capture_diff_slices, Algorithm, DiffOp};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;

// Unified diff text -> rows for display.
// - Syntax highlighting via syntect (separate state machines for the old and new sides)
// - Word-level intra-line diff emphasis via similar
// - Both unified and side-by-side rows are prebuilt; rendering only clips

const MAX_LINES: usize = 4000; // guard for huge diffs (overflow shows a truncation note)

pub const BG_ADD: Color = Color::Rgb(18, 58, 28);
pub const BG_ADD_EMPH: Color = Color::Rgb(28, 100, 46);
pub const BG_DEL: Color = Color::Rgb(70, 28, 28);
pub const BG_DEL_EMPH: Color = Color::Rgb(122, 44, 44);
pub const FG_HUNK: Color = Color::Rgb(97, 175, 239);

pub struct Highlighter {
    pub ss: SyntaxSet,
    pub theme: Theme,
}

impl Highlighter {
    pub fn new(theme_name: &str) -> Self {
        let ss = SyntaxSet::load_defaults_newlines();
        let mut themes = ThemeSet::load_defaults();
        let theme = themes
            .themes
            .remove(theme_name)
            .or_else(|| themes.themes.remove("base16-ocean.dark"))
            .unwrap_or_default();
        Self { ss, theme }
    }
}

/// Highlighting mode. syntect is slow relative to line count (measured >0.2ms/line), so the
/// main thread first builds plain with Pending; On runs on the worker thread and gets swapped in.
#[derive(Clone, Copy)]
pub enum Hl<'a> {
    /// With syntect highlighting (used on the worker thread)
    On(&'a Highlighter),
    /// Plain; an On version will replace it later, so no "syntax off" note is shown
    Pending,
    /// Plain for good (huge diff); shows the "syntax off (large diff)" note
    Off,
}

#[derive(Clone)]
pub struct Seg {
    pub style: Style,
    pub text: String,
}

#[derive(Clone)]
pub struct URow {
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub segs: Vec<Seg>,
    pub bg: Option<Color>,
    pub is_hunk: bool,
}

#[derive(Clone)]
pub struct Half {
    pub no: u32,
    pub segs: Vec<Seg>,
    pub bg: Option<Color>,
}

#[derive(Clone)]
pub struct SRow {
    pub left: Option<Half>,
    pub right: Option<Half>,
    /// A hunk header row is drawn as one line spanning both halves
    pub header: Option<String>,
}

pub struct DiffDoc {
    pub note: Option<String>,
    pub unified: Vec<URow>,
    pub sxs: Vec<SRow>,
    /// Index of each hunk's first row (unified / sxs)
    pub hunks_u: Vec<usize>,
    pub hunks_s: Vec<usize>,
    pub first_new_line: Option<u32>,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Ctx,
    Add,
    Del,
}

struct PLine {
    kind: Kind,
    old_no: Option<u32>,
    new_no: Option<u32>,
    text: String,
    /// Byte ranges for word emphasis
    emph: Vec<(usize, usize)>,
}

struct PHunk {
    header: String,
    lines: Vec<PLine>,
}

/// Neutralize control characters. Writing tabs or C0 controls straight to the terminal
/// desyncs the cursor from ratatui's buffer model, leaving stray noise outside the pane (observed on real terminals).
fn sanitize(s: &str) -> String {
    if !s
        .chars()
        .any(|c| c == '\t' || (c as u32) < 0x20 || c == '\u{7f}')
    {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\t' => out.push_str("    "),
            c if (c as u32) < 0x20 || c == '\u{7f}' => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Naive tokenizer: runs of identifier/digit chars, or any other single char, form a token
fn tokenize(s: &str) -> Vec<&str> {
    let mut toks = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        // i is always on a char boundary so next() cannot fail, but per convention (no unwrap) bail defensively
        let Some(c) = s[i..].chars().next() else {
            break;
        };
        if c.is_alphanumeric() || c == '_' {
            while i < bytes.len() {
                let Some(c2) = s[i..].chars().next() else {
                    break;
                };
                if c2.is_alphanumeric() || c2 == '_' {
                    i += c2.len_utf8();
                } else {
                    break;
                }
            }
        } else {
            i += c.len_utf8();
        }
        if i == start {
            break; // bail if no progress (prevents an infinite loop)
        }
        toks.push(&s[start..i]);
    }
    toks
}

/// Attach word-level emphasis ranges to a del/add line pair
fn word_emphasis(old: &mut PLine, new: &mut PLine) {
    let a = tokenize(&old.text);
    let b = tokenize(&new.text);
    let ops = capture_diff_slices(Algorithm::Myers, &a, &b);
    let offs = |toks: &[&str], upto: usize| -> usize { toks[..upto].iter().map(|t| t.len()).sum() };
    for op in ops {
        match op {
            DiffOp::Delete {
                old_index, old_len, ..
            } => {
                let s = offs(&a, old_index);
                let e = offs(&a, old_index + old_len);
                old.emph.push((s, e));
            }
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                let s = offs(&b, new_index);
                let e = offs(&b, new_index + new_len);
                new.emph.push((s, e));
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                let s = offs(&a, old_index);
                let e = offs(&a, old_index + old_len);
                old.emph.push((s, e));
                let s2 = offs(&b, new_index);
                let e2 = offs(&b, new_index + new_len);
                new.emph.push((s2, e2));
            }
            DiffOp::Equal { .. } => {}
        }
    }
}

fn parse(diff: &str) -> (Vec<PHunk>, bool) {
    let mut hunks: Vec<PHunk> = Vec::new();
    let mut binary = false;
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    let mut count = 0usize;
    for line in diff.lines() {
        if count > MAX_LINES {
            break;
        }
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            binary = true;
            continue;
        }
        if line.starts_with("@@") {
            // @@ -a,b +c,d @@ ...
            let parse_no = |part: &str| -> u32 {
                part.trim_start_matches(['-', '+'])
                    .split(',')
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1)
            };
            let mut it = line.split_whitespace();
            it.next(); // @@
            old_no = it.next().map(parse_no).unwrap_or(1);
            new_no = it.next().map(parse_no).unwrap_or(1);
            hunks.push(PHunk {
                header: sanitize(line),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            continue;
        };
        count += 1;
        if let Some(t) = line.strip_prefix('+') {
            hunk.lines.push(PLine {
                kind: Kind::Add,
                old_no: None,
                new_no: Some(new_no),
                text: sanitize(t),
                emph: Vec::new(),
            });
            new_no += 1;
        } else if let Some(t) = line.strip_prefix('-') {
            hunk.lines.push(PLine {
                kind: Kind::Del,
                old_no: Some(old_no),
                new_no: None,
                text: sanitize(t),
                emph: Vec::new(),
            });
            old_no += 1;
        } else if let Some(t) = line.strip_prefix(' ') {
            hunk.lines.push(PLine {
                kind: Kind::Ctx,
                old_no: Some(old_no),
                new_no: Some(new_no),
                text: sanitize(t),
                emph: Vec::new(),
            });
            old_no += 1;
            new_no += 1;
        }
        // Ignore "\ No newline at end of file"
    }
    // Word emphasis: pair runs of consecutive del and add lines by index
    for hunk in &mut hunks {
        let mut i = 0;
        while i < hunk.lines.len() {
            if hunk.lines[i].kind == Kind::Del {
                let del_start = i;
                while i < hunk.lines.len() && hunk.lines[i].kind == Kind::Del {
                    i += 1;
                }
                let add_start = i;
                while i < hunk.lines.len() && hunk.lines[i].kind == Kind::Add {
                    i += 1;
                }
                let pairs = (i - add_start).min(add_start - del_start);
                for k in 0..pairs {
                    let (a, b) = hunk.lines.split_at_mut(add_start + k);
                    word_emphasis(&mut a[del_start + k], &mut b[0]);
                }
            } else {
                i += 1;
            }
        }
    }
    (hunks, binary)
}

fn to_ratatui(fs: syntect::highlighting::Style) -> Style {
    Style::default().fg(Color::Rgb(
        fs.foreground.r,
        fs.foreground.g,
        fs.foreground.b,
    ))
}

/// syntect highlight result -> Seg list. Split at emph byte ranges and override the emphasis bg.
/// When hl is None, skip syntect and render plain in terminal default colors (fast path).
fn styled_segs(
    hl: Option<(&mut HighlightLines, &SyntaxSet)>,
    text: &str,
    emph: &[(usize, usize)],
    base_bg: Option<Color>,
    emph_bg: Option<Color>,
) -> Vec<Seg> {
    let with_nl = format!("{text}\n");
    let plain = hl.is_none();
    let ranges: Vec<(syntect::highlighting::Style, &str)> = match hl {
        Some((hl, ss)) => hl
            .highlight_line(&with_nl, ss)
            .unwrap_or_else(|_| vec![(syntect::highlighting::Style::default(), with_nl.as_str())]),
        None => vec![(syntect::highlighting::Style::default(), with_nl.as_str())],
    };
    let mut segs: Vec<Seg> = Vec::new();
    let mut offset = 0usize;
    for (st, chunk) in ranges {
        let chunk = chunk.strip_suffix('\n').unwrap_or(chunk);
        if chunk.is_empty() {
            continue;
        }
        // Split at emph boundaries
        let mut pos = 0usize;
        let base_style = if plain {
            Style::default()
        } else {
            to_ratatui(st)
        };
        while pos < chunk.len() {
            let abs = offset + pos;
            let in_emph = emph.iter().any(|&(s, e)| abs >= s && abs < e);
            // Advance to the next boundary
            let mut end = chunk.len();
            for &(s, e) in emph {
                let s_rel = s.saturating_sub(offset);
                let e_rel = e.saturating_sub(offset);
                if s_rel > pos && s_rel < end {
                    end = s_rel;
                }
                if e_rel > pos && e_rel < end {
                    end = e_rel;
                }
            }
            // Round to a char boundary
            let mut end2 = end;
            while end2 < chunk.len() && !chunk.is_char_boundary(end2) {
                end2 += 1;
            }
            let piece = &chunk[pos..end2.min(chunk.len())];
            let bg = if in_emph { emph_bg } else { base_bg };
            let mut style = base_style;
            if let Some(bg) = bg {
                style = style.bg(bg);
            }
            if in_emph {
                style = style.add_modifier(Modifier::BOLD);
            }
            segs.push(Seg {
                style,
                text: piece.to_string(),
            });
            pos = end2.min(chunk.len());
            if piece.is_empty() {
                break;
            }
        }
        offset += chunk.len();
    }
    if segs.is_empty() {
        segs.push(Seg {
            style: Style::default(),
            text: String::new(),
        });
    }
    segs
}

pub fn build_doc(diff: &str, path: &str, hl: Hl) -> DiffDoc {
    let (hunks, binary) = parse(diff);
    let syntax_off = matches!(hl, Hl::Off);
    // Separate state machines for the old (ctx+del) and new (ctx+add) sides; not built for plain
    let mut machines: Option<(HighlightLines, HighlightLines, &SyntaxSet)> = match hl {
        Hl::On(hi) => {
            let syntax = path
                .rsplit_once('.')
                .and_then(|(_, ext)| hi.ss.find_syntax_by_extension(ext))
                .or_else(|| {
                    path.rsplit('/')
                        .next()
                        .and_then(|n| hi.ss.find_syntax_by_extension(n))
                })
                .unwrap_or_else(|| hi.ss.find_syntax_plain_text());
            Some((
                HighlightLines::new(syntax, &hi.theme),
                HighlightLines::new(syntax, &hi.theme),
                &hi.ss,
            ))
        }
        _ => None,
    };

    let mut unified: Vec<URow> = Vec::new();
    let mut sxs: Vec<SRow> = Vec::new();
    let mut hunks_u = Vec::new();
    let mut hunks_s = Vec::new();
    let mut first_new_line: Option<u32> = None;
    let total: usize = hunks.iter().map(|h| h.lines.len()).sum();

    for hunk in &hunks {
        hunks_u.push(unified.len());
        hunks_s.push(sxs.len());
        unified.push(URow {
            old_no: None,
            new_no: None,
            segs: vec![Seg {
                style: Style::default().fg(FG_HUNK),
                text: hunk.header.clone(),
            }],
            bg: None,
            is_hunk: true,
        });
        sxs.push(SRow {
            left: None,
            right: None,
            header: Some(hunk.header.clone()),
        });

        // Pairing buffers for side-by-side
        let mut pend_left: Vec<Half> = Vec::new();
        let mut pend_right: Vec<Half> = Vec::new();
        let flush_sxs = |sxs: &mut Vec<SRow>, l: &mut Vec<Half>, r: &mut Vec<Half>| {
            let n = l.len().max(r.len());
            let mut li = l.drain(..);
            let mut ri = r.drain(..);
            for _ in 0..n {
                sxs.push(SRow {
                    left: li.next(),
                    right: ri.next(),
                    header: None,
                });
            }
        };

        for line in &hunk.lines {
            match line.kind {
                Kind::Ctx => {
                    flush_sxs(&mut sxs, &mut pend_left, &mut pend_right);
                    let segs_old = styled_segs(
                        machines.as_mut().map(|m| (&mut m.0, m.2)),
                        &line.text,
                        &[],
                        None,
                        None,
                    );
                    let segs_new = styled_segs(
                        machines.as_mut().map(|m| (&mut m.1, m.2)),
                        &line.text,
                        &[],
                        None,
                        None,
                    );
                    unified.push(URow {
                        old_no: line.old_no,
                        new_no: line.new_no,
                        segs: segs_new.clone(),
                        bg: None,
                        is_hunk: false,
                    });
                    sxs.push(SRow {
                        left: Some(Half {
                            no: line.old_no.unwrap_or(0),
                            segs: segs_old,
                            bg: None,
                        }),
                        right: Some(Half {
                            no: line.new_no.unwrap_or(0),
                            segs: segs_new,
                            bg: None,
                        }),
                        header: None,
                    });
                }
                Kind::Del => {
                    let segs = styled_segs(
                        machines.as_mut().map(|m| (&mut m.0, m.2)),
                        &line.text,
                        &line.emph,
                        Some(BG_DEL),
                        Some(BG_DEL_EMPH),
                    );
                    unified.push(URow {
                        old_no: line.old_no,
                        new_no: None,
                        segs: segs.clone(),
                        bg: Some(BG_DEL),
                        is_hunk: false,
                    });
                    pend_left.push(Half {
                        no: line.old_no.unwrap_or(0),
                        segs,
                        bg: Some(BG_DEL),
                    });
                }
                Kind::Add => {
                    if first_new_line.is_none() {
                        first_new_line = line.new_no;
                    }
                    let segs = styled_segs(
                        machines.as_mut().map(|m| (&mut m.1, m.2)),
                        &line.text,
                        &line.emph,
                        Some(BG_ADD),
                        Some(BG_ADD_EMPH),
                    );
                    unified.push(URow {
                        old_no: None,
                        new_no: line.new_no,
                        segs: segs.clone(),
                        bg: Some(BG_ADD),
                        is_hunk: false,
                    });
                    pend_right.push(Half {
                        no: line.new_no.unwrap_or(0),
                        segs,
                        bg: Some(BG_ADD),
                    });
                }
            }
        }
        flush_sxs(&mut sxs, &mut pend_left, &mut pend_right);
    }

    let note = if binary {
        Some("binary file (no diff)".to_string())
    } else if hunks.is_empty() {
        Some("no diff".to_string())
    } else if total > MAX_LINES && syntax_off {
        Some(format!(
            "… truncated after {MAX_LINES} lines · syntax off (large diff)"
        ))
    } else if total > MAX_LINES {
        Some(format!("… truncated after {MAX_LINES} lines"))
    } else if syntax_off {
        Some("syntax off (large diff)".to_string())
    } else {
        None
    };

    if first_new_line.is_none() {
        first_new_line = hunks
            .first()
            .and_then(|h| h.lines.first().and_then(|l| l.new_no));
    }

    DiffDoc {
        note,
        unified,
        sxs,
        hunks_u,
        hunks_s,
        first_new_line,
    }
}

#[cfg(test)]
mod bench {
    use super::*;

    #[test]
    #[ignore]
    fn bench_build_doc() {
        let path = std::env::var("WTD_BENCH_DIFF").unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        let hi = Highlighter::new("base16-ocean.dark");
        for (label, hl) in [("plain", Hl::Pending), ("syntect", Hl::On(&hi))] {
            let t = std::time::Instant::now();
            let doc = build_doc(&text, "big.rs", hl);
            eprintln!(
                "{label}: {:?} (unified rows: {})",
                t.elapsed(),
                doc.unified.len()
            );
        }
    }
}
