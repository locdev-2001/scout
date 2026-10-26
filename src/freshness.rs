//! Detects which files changed between two queries, from the file system itself on every query.
//! A change is any difference in mtime, ctime, inode or size (Windows: write time, creation time, size): ctime cannot be set back by a tool,
//! so a write that restores the old mtime is still seen.
//!
//! The first sweep walks the tree (honouring .gitignore). Later sweeps only stat what is known, in a few
//! threads; a directory whose own stamp changed (an entry was added, removed or renamed) is listed again,
//! and a changed .gitignore triggers a full walk, so new and deleted files are found without walking.

use ignore::{WalkBuilder, WalkState};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;

type Stamp = (i64, i64, i64, i64, u64, u64);

/// Stat calls on a warm disk cache cost little each but add up over ten thousand files: spread them out.
fn stat_threads() -> usize {
    thread::available_parallelism().map_or(4, |count| count.get().clamp(2, 8))
}

#[derive(Default)]
struct Tree {
    files: HashMap<String, Stamp>,
    directories: HashMap<String, Stamp>,
}

pub struct Freshness {
    repo_root: PathBuf,
    tree: Option<Tree>,
    /// Files outside version control that a driver depends on (`.env`, lock files, ...).
    extra_inputs: BTreeSet<String>,
}

#[cfg(unix)]
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).ok()?;
    Some((metadata.mtime(), metadata.mtime_nsec(), metadata.ctime(), metadata.ctime_nsec(), metadata.ino(), metadata.size()))
}

/// Windows exposes no change time or inode through the standard library: write and creation time, and size.
#[cfg(windows)]
fn stamp(path: &Path) -> Option<Stamp> {
    use std::os::windows::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).ok()?;
    Some((metadata.last_write_time() as i64, 0, metadata.creation_time() as i64, 0, 0, metadata.file_size()))
}

/// Stats every path in a few threads; a path that no longer exists maps to None.
fn stamp_all(root: &Path, paths: Vec<&String>) -> Vec<(String, Option<Stamp>)> {
    let chunk = paths.len().div_ceil(stat_threads()).max(1);
    thread::scope(|scope| {
        let handles: Vec<_> = paths
            .chunks(chunk)
            .map(|part| scope.spawn(move || part.iter().map(|path| ((*path).clone(), stamp(&root.join(path)))).collect::<Vec<_>>()))
            .collect();
        handles.into_iter().flat_map(|handle| handle.join().unwrap_or_default()).collect()
    })
}

impl Freshness {
    pub fn new(repo_root: PathBuf) -> Self {
        Self { repo_root, tree: None, extra_inputs: BTreeSet::new() }
    }

    /// Starts watching extra paths from their current state, so adding a path never reports it as changed.
    pub fn watch(&mut self, paths: impl IntoIterator<Item = String>) {
        for path in paths {
            if self.extra_inputs.insert(path.clone()) {
                if let (Some(tree), Some(stamp)) = (self.tree.as_mut(), stamp(&self.repo_root.join(&path))) {
                    tree.files.insert(path, stamp);
                }
            }
        }
    }

    /// Walks `start` (the whole repo, or one directory with `depth` 1) honouring .gitignore files.
    fn walk(&self, start: &Path, depth: Option<usize>) -> Tree {
        let found = Mutex::new(Tree::default());
        let root = &self.repo_root;
        WalkBuilder::new(start)
            .hidden(false)
            .max_depth(depth)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build_parallel()
            .run(|| {
                let found = &found;
                Box::new(move |entry| {
                    if let Ok(entry) = entry {
                        if let (Some(kind), Some(stamp), Ok(relative)) = (entry.file_type(), stamp(entry.path()), entry.path().strip_prefix(root)) {
                            let relative = relative.to_string_lossy().into_owned();
                            let mut tree = found.lock().unwrap();
                            if kind.is_dir() {
                                tree.directories.insert(relative, stamp);
                            } else if kind.is_file() {
                                tree.files.insert(relative, stamp);
                            }
                        }
                    }
                    WalkState::Continue
                })
            });
        found.into_inner().unwrap()
    }

    fn full_tree(&self) -> Tree {
        let mut tree = self.walk(&self.repo_root, None);
        for path in &self.extra_inputs {
            if let Some(stamp) = stamp(&self.repo_root.join(path)) {
                tree.files.insert(path.clone(), stamp);
            }
        }
        tree
    }

    /// Directories worth watching for changes: every repo directory not ignored, plus those holding extra inputs.
    pub fn watched_directories(&self) -> Vec<PathBuf> {
        let mut directories: BTreeSet<PathBuf> = self.tree.as_ref().map(|tree| tree.directories.keys().map(|path| self.repo_root.join(path)).collect()).unwrap_or_default();
        directories.insert(self.repo_root.clone());
        for input in &self.extra_inputs {
            if let Some(parent) = self.repo_root.join(input).parent() {
                directories.insert(parent.to_path_buf());
            }
        }
        directories.into_iter().filter(|directory| directory.is_dir()).collect()
    }

    /// Paths added, removed or modified since the previous sweep; empty on the first sweep.
    pub fn sweep(&mut self) -> Vec<String> {
        let Some(mut tree) = self.tree.take() else {
            self.tree = Some(self.full_tree());
            return Vec::new();
        };
        let mut changed = BTreeSet::new();

        let directories: Vec<&String> = tree.directories.keys().collect();
        let moved_directories: Vec<String> = stamp_all(&self.repo_root, directories)
            .into_iter()
            .filter(|(path, now)| now.as_ref() != tree.directories.get(path))
            .map(|(path, _)| path)
            .collect();
        for directory in &moved_directories {
            self.relist(&mut tree, directory, &mut changed);
        }

        let files: Vec<&String> = tree.files.keys().collect();
        for (path, now) in stamp_all(&self.repo_root, files) {
            if now.as_ref() != tree.files.get(&path) {
                changed.insert(path);
            }
        }
        for path in &changed {
            match stamp(&self.repo_root.join(path)) {
                Some(now) if tree.files.contains_key(path) || self.extra_inputs.contains(path) => {
                    tree.files.insert(path.clone(), now);
                }
                Some(_) => {}
                None => {
                    tree.files.remove(path);
                }
            }
        }

        if changed.iter().any(|path| path.ends_with(".gitignore")) {
            // Ignore rules changed: what counts as a repo file may have changed anywhere.
            let fresh = self.full_tree();
            for (path, stamp) in &fresh.files {
                if tree.files.get(path) != Some(stamp) {
                    changed.insert(path.clone());
                }
            }
            changed.extend(tree.files.keys().filter(|path| !fresh.files.contains_key(*path)).cloned());
            tree = fresh;
        }
        self.tree = Some(tree);
        changed.into_iter().collect()
    }

    /// Lists one directory again after its own stamp changed: picks up added entries (walking new
    /// subdirectories fully) and drops removed ones with everything below them.
    fn relist(&self, tree: &mut Tree, directory: &str, changed: &mut BTreeSet<String>) {
        let absolute = self.repo_root.join(directory);
        let below = |path: &str| directory.is_empty() || path.starts_with(&format!("{directory}/"));
        if !absolute.is_dir() {
            tree.directories.retain(|path, _| path != directory && !below(path));
            let removed: Vec<String> = tree.files.keys().filter(|path| below(path)).cloned().collect();
            for path in removed {
                tree.files.remove(&path);
                changed.insert(path);
            }
            return;
        }
        let listed = self.walk(&absolute, Some(1));
        let direct_child = |path: &str| {
            let rest = if directory.is_empty() { path } else { path.strip_prefix(&format!("{directory}/")).unwrap_or("") };
            !rest.is_empty() && !rest.contains('/')
        };
        let gone_files: Vec<String> = tree.files.keys().filter(|path| direct_child(path) && !listed.files.contains_key(*path)).cloned().collect();
        for path in gone_files {
            tree.files.remove(&path);
            changed.insert(path);
        }
        let gone_directories: Vec<String> = tree.directories.keys().filter(|path| direct_child(path) && !listed.directories.contains_key(*path)).cloned().collect();
        for gone in gone_directories {
            self.relist(tree, &gone, changed);
        }
        for (path, stamp) in listed.files {
            if !tree.files.contains_key(&path) {
                tree.files.insert(path.clone(), stamp);
                changed.insert(path);
            }
        }
        for (path, stamp) in listed.directories {
            if path != directory && !tree.directories.contains_key(&path) {
                let subtree = self.walk(&self.repo_root.join(&path), None);
                for (file, file_stamp) in subtree.files {
                    changed.insert(file.clone());
                    tree.files.insert(file, file_stamp);
                }
                tree.directories.extend(subtree.directories);
            }
            tree.directories.insert(path, stamp);
        }
    }
}
