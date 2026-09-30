//! The folder tree in the library side bar

use std::path::{Path, PathBuf};

/// Files the player can open
pub fn is_song_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "mid" | "midi" | "musicxml" | "mxl" | "xml"))
}

#[derive(Debug)]
struct Node {
    path: PathBuf,
    name: String,
    kind: NodeKind,
}

#[derive(Debug)]
enum NodeKind {
    File,
    Dir {
        expanded: bool,
        /// Read when the folder is first expanded
        children: Option<Vec<Node>>,
    },
}

impl Node {
    fn new(path: PathBuf, is_dir: bool) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let kind = if is_dir {
            NodeKind::Dir {
                expanded: false,
                children: None,
            }
        } else {
            NodeKind::File
        };
        Self { path, name, kind }
    }

    fn set_expanded(&mut self, value: bool) {
        if let NodeKind::Dir { expanded, children } = &mut self.kind {
            *expanded = value;
            if value && children.is_none() {
                *children = Some(read_dir(&self.path));
            }
        }
    }
}

/// Sub folders first, then song files, both by name
fn read_dir(path: &Path) -> Vec<Node> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut nodes: Vec<Node> = entries
        .filter_map(|e| e.ok())
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let path = e.path();
            let is_dir = e.file_type().ok()?.is_dir();
            (is_dir || is_song_file(&path)).then(|| Node::new(path, is_dir))
        })
        .collect();
    nodes.sort_by_cached_key(|n| (matches!(n.kind, NodeKind::File), natural_key(&n.name)));
    nodes
}

/// "2. Etude" before "10. Etude": text pieces compare as text, digit runs as numbers
fn natural_key(name: &str) -> Vec<(String, u64)> {
    let mut key = Vec::new();
    let mut text = String::new();
    let mut digits = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_digit() {
            digits.push(c);
        } else {
            if !digits.is_empty() {
                let number = digits.parse().unwrap_or(u64::MAX);
                key.push((std::mem::take(&mut text), number));
                digits.clear();
            }
            text.push(c);
        }
    }
    let number = digits.parse().unwrap_or(0);
    key.push((text, number));
    key
}

/// One visible line of the tree
#[derive(Debug, Clone)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// A folder the user added (can be removed)
    pub is_root: bool,
}

#[derive(Debug, Default)]
pub struct Library {
    roots: Vec<Node>,
}

impl Library {
    pub fn new(folders: &[PathBuf]) -> Self {
        let mut lib = Self::default();
        for folder in folders {
            lib.add_root(folder.clone());
        }
        lib
    }

    pub fn root_paths(&self) -> Vec<PathBuf> {
        self.roots.iter().map(|n| n.path.clone()).collect()
    }

    /// Add a folder (expanded); false if it was already there
    pub fn add_root(&mut self, path: PathBuf) -> bool {
        if self.roots.iter().any(|n| n.path == path) {
            return false;
        }
        let mut node = Node::new(path, true);
        node.set_expanded(true);
        self.roots.push(node);
        true
    }

    pub fn remove_root(&mut self, path: &Path) {
        self.roots.retain(|n| n.path != path);
    }

    /// Read every expanded folder again, keeping what is expanded
    pub fn refresh(&mut self) {
        fn expanded_paths(nodes: &[Node], out: &mut Vec<PathBuf>) {
            for node in nodes {
                if let NodeKind::Dir {
                    expanded: true,
                    children,
                } = &node.kind
                {
                    out.push(node.path.clone());
                    if let Some(children) = children {
                        expanded_paths(children, out);
                    }
                }
            }
        }
        let mut open = Vec::new();
        expanded_paths(&self.roots, &mut open);

        for root in &mut self.roots {
            *root = Node::new(root.path.clone(), true);
        }
        // Parents come before their children in `open`
        for path in open {
            if let Some(node) = self.find_mut(&path) {
                node.set_expanded(true);
            }
        }
    }

    pub fn collapse_all(&mut self) {
        fn collapse(nodes: &mut [Node]) {
            for node in nodes {
                if let NodeKind::Dir { expanded, children } = &mut node.kind {
                    *expanded = false;
                    if let Some(children) = children {
                        collapse(children);
                    }
                }
            }
        }
        collapse(&mut self.roots);
    }

    fn find_mut(&mut self, path: &Path) -> Option<&mut Node> {
        fn find<'a>(nodes: &'a mut [Node], path: &Path) -> Option<&'a mut Node> {
            for node in nodes {
                if node.path == path {
                    return Some(node);
                }
                if path.starts_with(&node.path)
                    && let NodeKind::Dir {
                        children: Some(children),
                        ..
                    } = &mut node.kind
                {
                    return find(children, path);
                }
            }
            None
        }
        find(&mut self.roots, path)
    }

    pub fn set_expanded(&mut self, path: &Path, expanded: bool) {
        if let Some(node) = self.find_mut(path) {
            node.set_expanded(expanded);
        }
    }

    pub fn toggle(&mut self, path: &Path) {
        if let Some(node) = self.find_mut(path)
            && let NodeKind::Dir { expanded, .. } = node.kind
        {
            node.set_expanded(!expanded);
        }
    }

    /// Expand the folders down to `path` (a file inside one of the roots)
    pub fn reveal(&mut self, path: &Path) {
        let Some(root) = self
            .roots
            .iter()
            .map(|n| n.path.clone())
            .find(|root| path.starts_with(root))
        else {
            return;
        };
        let mut dir = root.clone();
        self.set_expanded(&dir, true);
        if let Ok(rest) = path.strip_prefix(&root) {
            let parts: Vec<_> = rest.components().collect();
            for part in parts.iter().take(parts.len().saturating_sub(1)) {
                dir.push(part);
                self.set_expanded(&dir, true);
            }
        }
    }

    /// The lines to draw, top to bottom
    pub fn rows(&self) -> Vec<Row> {
        fn push(nodes: &[Node], depth: usize, out: &mut Vec<Row>) {
            for node in nodes {
                let (is_dir, expanded) = match &node.kind {
                    NodeKind::File => (false, false),
                    NodeKind::Dir { expanded, .. } => (true, *expanded),
                };
                out.push(Row {
                    path: node.path.clone(),
                    name: node.name.clone(),
                    depth,
                    is_dir,
                    expanded,
                    is_root: depth == 0,
                });
                if let NodeKind::Dir {
                    expanded: true,
                    children: Some(children),
                } = &node.kind
                {
                    push(children, depth + 1, out);
                }
            }
        }
        let mut out = Vec::new();
        push(&self.roots, 0, &mut out);
        out
    }
}

/// The folders to show when none were added: the folder around the last song
pub fn default_folders(last_song: Option<&Path>) -> Vec<PathBuf> {
    let Some(dir) = last_song.and_then(Path::parent) else {
        return Vec::new();
    };
    // Songs usually sit in one folder per collection: show the collections
    let parent = dir.parent().filter(|p| {
        std::fs::read_dir(p)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .count()
                    > 1
            })
            .unwrap_or(false)
            && p.parent().is_some()
    });
    vec![parent.unwrap_or(dir).to_path_buf()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        let mut names = vec!["10. b", "2. a", "1. c", "Alpha", "alpha 3", "alpha 20"];
        names.sort_by_cached_key(|n| natural_key(n));
        assert_eq!(
            names,
            vec!["1. c", "2. a", "10. b", "Alpha", "alpha 3", "alpha 20"]
        );
    }

    #[test]
    fn tree_lists_song_files_and_folders() {
        let dir = std::env::temp_dir().join(format!("neothesia_lib_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Bach")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("Bach/Prelude 2.mid"), b"").unwrap();
        std::fs::write(dir.join("Bach/Prelude 10.musicxml"), b"").unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        std::fs::write(dir.join("Etude.MID"), b"").unwrap();

        let mut lib = Library::new(std::slice::from_ref(&dir));
        let names: Vec<String> = lib.rows().iter().map(|r| r.name.clone()).collect();
        assert_eq!(names[1..], ["Bach".to_string(), "Etude.MID".to_string()]);

        lib.reveal(&dir.join("Bach/Prelude 10.musicxml"));
        let rows = lib.rows();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names[1..],
            ["Bach", "Prelude 2.mid", "Prelude 10.musicxml", "Etude.MID"]
        );
        assert_eq!(rows[2].depth, 2);

        std::fs::write(dir.join("Bach/Air.mid"), b"").unwrap();
        lib.refresh();
        assert_eq!(lib.rows().len(), 6, "refresh keeps Bach open and finds Air");

        lib.collapse_all();
        assert_eq!(lib.rows().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
