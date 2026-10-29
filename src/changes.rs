//! What one agent session changed in the code, whatever made the change (an edit, a written file, a shell script):
//! read from the repository itself, so no way of editing goes unseen. Also which symbols' callers the agent has
//! been shown, so it is told about each changed one once. Kept per session in the cache directory.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// A session nobody touched for this long is forgotten.
const KEPT_FOR: Duration = Duration::from_secs(7 * 86_400);
/// A file larger than this is no source code worth fingerprinting.
const MAX_FILE_BYTES: u64 = 2 << 20;
/// Changed lines located per file at most: a whole rewritten file says which functions changed long before this.
pub const MAX_LINES_PER_FILE: usize = 400;
/// How long a hook waits for another hook of the same session (parallel tool calls) to put the record back.
const LOCK_WAIT: Duration = Duration::from_secs(2);
/// A lock older than this was left by a hook that died.
const STALE_LOCK: Duration = Duration::from_secs(10);

#[derive(Serialize, Deserialize, Default)]
struct RepositoryChanges {
    /// The commit the session started from. Changes are measured from it, so a commit made during the session
    /// hides none of them.
    base: String,
    /// Fingerprint of each uncommitted code file at the last look.
    uncommitted: BTreeMap<String, u64>,
    /// Code files the session changed.
    changed: BTreeSet<String>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Session {
    repositories: BTreeMap<PathBuf, RepositoryChanges>,
    /// Symbols, or names asked about, whose callers the agent has been shown.
    shown: BTreeSet<String>,
}

/// Lines changed in one file, to be located.
pub struct ChangedLines {
    pub repo: PathBuf,
    pub file: String,
    pub lines: Vec<u32>,
}

/// Whether a file can hold calls: settings, notes, data and logs cannot.
pub fn holds_code(file: &Path) -> bool {
    let name = file.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
    let extension = file.extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
    !name.starts_with(".env") && !["md", "txt", "json", "yml", "yaml", "toml", "ini", "log", "csv", "lock", "xml", "sql", "neon"].contains(&extension.as_str())
}

fn sessions_directory() -> PathBuf {
    crate::daemon::cache_directory().join("sessions")
}

/// Runs `change` on the session's record and saves it, one hook at a time. None when the session cannot be
/// recorded (no usable id, the record is held too long by another hook).
pub fn with_session<T>(session_id: &str, change: impl FnOnce(&mut Session) -> T) -> Option<T> {
    if session_id.is_empty() || !session_id.chars().all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_') {
        return None;
    }
    let directory = sessions_directory();
    std::fs::create_dir_all(&directory).ok()?;
    let path = directory.join(format!("{session_id}.json"));
    let lock = Lock::take(path.with_extension("lock"))?;
    let mut session: Session = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let result = change(&mut session);
    let written = path.with_extension("json.new");
    if std::fs::write(&written, serde_json::to_vec(&session).ok()?).is_ok() {
        let _ = std::fs::rename(&written, &path);
    }
    drop(lock);
    Some(result)
}

/// Removes the records of sessions untouched for a week.
pub fn forget_old_sessions() {
    let Ok(entries) = std::fs::read_dir(sessions_directory()) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|metadata| metadata.modified()).ok().and_then(|modified| SystemTime::now().duration_since(modified).ok()).is_some_and(|age| age > KEPT_FOR);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

struct Lock(PathBuf);

impl Lock {
    fn take(path: PathBuf) -> Option<Lock> {
        let deadline = std::time::Instant::now() + LOCK_WAIT;
        loop {
            if std::fs::OpenOptions::new().write(true).create_new(true).open(&path).is_ok() {
                return Some(Lock(path));
            }
            let stale = std::fs::metadata(&path).and_then(|metadata| metadata.modified()).ok().and_then(|modified| SystemTime::now().duration_since(modified).ok()).is_some_and(|age| age > STALE_LOCK);
            if stale {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if std::time::Instant::now() > deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git").arg("-C").arg(repo).args(args).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Each uncommitted code file of the repository (relative path) with a fingerprint of its content.
fn uncommitted_code(repo: &Path) -> BTreeMap<String, u64> {
    let Some(status) = git(repo, &["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"]) else { return BTreeMap::new() };
    status
        .split('\0')
        .filter_map(|entry| entry.get(3..))
        .filter(|file| holds_code(Path::new(file)))
        .filter_map(|file| {
            let path = repo.join(file);
            let metadata = std::fs::metadata(&path).ok().filter(|metadata| metadata.is_file() && metadata.len() <= MAX_FILE_BYTES)?;
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            std::fs::read(&path).ok()?.hash(&mut hasher);
            metadata.len().hash(&mut hasher);
            Some((file.to_owned(), hasher.finish()))
        })
        .collect()
}

/// The lines each hunk of a `git diff -U0` puts in the new file; a hunk that only removes lines gives the line
/// the removal happened at, so the symbol around it is still asked about.
fn changed_lines(diff: &str) -> BTreeMap<String, Vec<u32>> {
    let mut files: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    let mut file = None;
    for line in diff.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            file = Some(path.to_owned());
            continue;
        }
        let (Some(file), Some(hunk)) = (&file, line.strip_prefix("@@ ")) else { continue };
        let Some(added) = hunk.split(' ').find_map(|part| part.strip_prefix('+')) else { continue };
        let (start, count) = match added.split_once(',') {
            Some((start, count)) => (start.parse::<u32>().unwrap_or(1), count.parse::<u32>().unwrap_or(1)),
            None => (added.parse::<u32>().unwrap_or(1), 1),
        };
        let lines = files.entry(file.clone()).or_default();
        if count == 0 {
            lines.push(start.max(1));
        } else {
            lines.extend(start..start + count);
        }
    }
    for lines in files.values_mut() {
        lines.dedup();
        lines.truncate(MAX_LINES_PER_FILE);
    }
    files
}

impl Session {
    /// Starts following a repository: the commit it is at and its uncommitted files now are where this session's
    /// changes are measured from. Returns whether the repository is followed from now on (a repository without any
    /// commit is not).
    pub fn follow(&mut self, repo: &Path) -> bool {
        if self.repositories.contains_key(repo) {
            return false;
        }
        let Some(base) = git(repo, &["rev-parse", "HEAD"]) else { return false };
        self.repositories.insert(repo.to_path_buf(), RepositoryChanges { base: base.trim().to_owned(), uncommitted: uncommitted_code(repo), changed: BTreeSet::new() });
        true
    }

    pub fn repositories(&self) -> Vec<PathBuf> {
        self.repositories.keys().cloned().collect()
    }

    /// The code files of a followed repository that changed since the previous look.
    pub fn changes_since_last_look(&mut self, repo: &Path) -> Vec<String> {
        let Some(changes) = self.repositories.get_mut(repo) else { return Vec::new() };
        let now = uncommitted_code(repo);
        let changed: Vec<String> = now.iter().filter(|(file, fingerprint)| changes.uncommitted.get(*file) != Some(fingerprint)).map(|(file, _)| file.clone()).collect();
        changes.uncommitted = now;
        changes.changed.extend(changed.iter().cloned());
        changed
    }

    /// Counts a file as changed by the session (one edited the moment its repository started being followed).
    pub fn note_changed(&mut self, repo: &Path, file: &str) {
        if let Some(changes) = self.repositories.get_mut(repo) {
            changes.changed.insert(file.to_owned());
        }
    }

    /// The lines of `files` that differ from the session's starting commit. A file the commit did not have is new
    /// code: nothing called it before the session.
    pub fn changed_lines(&self, repo: &Path, files: &[String]) -> Vec<ChangedLines> {
        let Some(changes) = self.repositories.get(repo) else { return Vec::new() };
        if files.is_empty() {
            return Vec::new();
        }
        let mut args = vec!["diff", "-U0", "--no-color", "--no-ext-diff", "--no-renames", "--diff-filter=M", changes.base.as_str(), "--"];
        args.extend(files.iter().map(String::as_str));
        let Some(diff) = git(repo, &args) else { return Vec::new() };
        changed_lines(&diff).into_iter().map(|(file, lines)| ChangedLines { repo: repo.to_path_buf(), file, lines }).collect()
    }

    /// The changed lines of every file this session changed in the repository.
    pub fn all_changed_lines(&self, repo: &Path) -> Vec<ChangedLines> {
        let files: Vec<String> = self.repositories.get(repo).map(|changes| changes.changed.iter().cloned().collect()).unwrap_or_default();
        self.changed_lines(repo, &files)
    }

    pub fn has_changes(&self) -> bool {
        self.repositories.values().any(|changes| !changes.changed.is_empty())
    }

    pub fn mark_shown<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        self.shown.extend(names.into_iter().map(|name| name.trim().trim_start_matches('\\').to_owned()).filter(|name| !name.is_empty()));
    }

    /// Whether the agent was shown the callers of `symbol`: asked by its full name, or by a shorter one it ends with
    /// (`Order::save` or `save` for `App\Order::save`).
    pub fn was_shown(&self, symbol: &str) -> bool {
        self.shown.iter().any(|name| match symbol.strip_suffix(name.as_str()) {
            Some("") => true,
            Some(before) => ["\\", "::", "#", ".", "/"].iter().any(|separator| before.ends_with(separator)),
            None => false,
        })
    }
}
