use crate::git::FileChange;
use std::collections::{BTreeMap, HashSet};

/// Tree building from the set of changed files.
/// - Directory chains with a single child are compressed into one node like `docs/archive/sow` (GitHub style)
/// - Collapse state is held by App as the set of full paths of collapsed directories

#[derive(Debug)]
enum Node {
    Dir(BTreeMap<String, Node>),
    File(usize), // index into files
}

#[derive(Debug, Clone)]
pub struct Row {
    pub depth: usize,
    /// Display label ("a/b/c" for compressed directories)
    pub label: String,
    /// Full path for a directory (collapse key); None for a file
    pub dir_path: Option<String>,
    pub file_idx: Option<usize>,
    pub expanded: bool,
}

fn insert(root: &mut BTreeMap<String, Node>, parts: &[&str], idx: usize) {
    if parts.len() == 1 {
        root.insert(parts[0].to_string(), Node::File(idx));
        return;
    }
    let entry = root
        .entry(parts[0].to_string())
        .or_insert_with(|| Node::Dir(BTreeMap::new()));
    if let Node::Dir(children) = entry {
        insert(children, &parts[1..], idx);
    }
}

fn walk(
    map: &BTreeMap<String, Node>,
    prefix: &str,
    depth: usize,
    collapsed: &HashSet<String>,
    rows: &mut Vec<Row>,
) {
    // Directories before files, each sorted by name
    let (dirs, files): (Vec<_>, Vec<_>) = map.iter().partition(|(_, n)| matches!(n, Node::Dir(_)));
    for (name, node) in dirs {
        let Node::Dir(children) = node else { continue };
        // Single-chain compression: keep joining while the only child is a directory
        let mut label = name.clone();
        let mut cur = children;
        loop {
            if cur.len() == 1 {
                if let Some((only_name, Node::Dir(grand))) = cur.iter().next() {
                    label = format!("{label}/{only_name}");
                    cur = grand;
                    continue;
                }
            }
            break;
        }
        let dir_path = if prefix.is_empty() {
            label.clone()
        } else {
            format!("{prefix}/{label}")
        };
        let expanded = !collapsed.contains(&dir_path);
        rows.push(Row {
            depth,
            label: label.clone(),
            dir_path: Some(dir_path.clone()),
            file_idx: None,
            expanded,
        });
        if expanded {
            walk(cur, &dir_path, depth + 1, collapsed, rows);
        }
    }
    for (name, node) in files {
        let Node::File(idx) = node else { continue };
        rows.push(Row {
            depth,
            label: name.clone(),
            dir_path: None,
            file_idx: Some(*idx),
            expanded: false,
        });
    }
}

/// Build rows for the tree view
pub fn tree_rows(files: &[FileChange], collapsed: &HashSet<String>, filter: &str) -> Vec<Row> {
    let mut root: BTreeMap<String, Node> = BTreeMap::new();
    for (i, f) in files.iter().enumerate() {
        if !filter.is_empty() && !f.path.contains(filter) {
            continue;
        }
        let parts: Vec<&str> = f.path.split('/').collect();
        insert(&mut root, &parts, i);
    }
    let mut rows = Vec::new();
    walk(&root, "", 0, collapsed, &mut rows);
    rows
}

/// Rows for the flat view
pub fn flat_rows(files: &[FileChange], filter: &str) -> Vec<Row> {
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| filter.is_empty() || f.path.contains(filter))
        .map(|(i, f)| Row {
            depth: 0,
            label: f.path.clone(),
            dir_path: None,
            file_idx: Some(i),
            expanded: false,
        })
        .collect()
}
