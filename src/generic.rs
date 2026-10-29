//! The fallback driver for every language without a driver of its own. It cannot tell types apart, so it
//! never claims more than "this line calls something with that name": every match is `unresolved`, listed
//! for reading, never expanded. It keeps, in memory, the call-like names of each text file no other driver
//! handles, and refreshes a file when the core reports it changed.

use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

struct GenericIndex {
    repo_root: PathBuf,
    claimed_extensions: HashSet<String>,
    /// call-like name => (file, line) occurrences
    calls: HashMap<String, Vec<(String, u32)>>,
    /// file => names it contributed, to remove them when it changes
    names_by_file: HashMap<String, Vec<String>>,
    /// file => hashes of every word in it, sorted: whether a file names a class without reading it again
    words_by_file: HashMap<String, Vec<u64>>,
}

/// FNV-1a of a lowercased word.
fn word_hash(word: &[u8]) -> u64 {
    word.iter().fold(0xcbf29ce484222325u64, |hash, byte| (hash ^ byte.to_ascii_lowercase() as u64).wrapping_mul(0x100000001b3))
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80
}

/// Text that is no code (prose, data, source maps): a `name(` there calls nothing.
const NOT_CODE: [&str; 13] = ["md", "markdown", "rst", "txt", "csv", "tsv", "json", "jsonl", "map", "lock", "log", "svg", "snap"];

/// Keywords that introduce a definition rather than a call, across common languages.
const DEFINITION_KEYWORDS: [&str; 8] = ["function", "def", "fn", "func", "fun", "sub", "proc", "method"];

/// `name(` and `name (`: what a call looks like in nearly every language (definitions left out), and the hash of
/// every word.
fn call_like_names(content: &[u8]) -> (Vec<(String, u32)>, Vec<u64>) {
    let mut found = Vec::new();
    let mut words = Vec::new();
    let mut line = 1u32;
    let mut index = 0;
    let mut previous_word: &[u8] = b"";
    while index < content.len() {
        let byte = content[index];
        if byte == b'\n' {
            line += 1;
            index += 1;
            continue;
        }
        if is_identifier_byte(byte) && !byte.is_ascii_digit() && (index == 0 || !is_identifier_byte(content[index - 1])) {
            let start = index;
            while index < content.len() && is_identifier_byte(content[index]) {
                index += 1;
            }
            let mut after = index;
            while after < content.len() && (content[after] == b' ' || content[after] == b'\t') {
                after += 1;
            }
            words.push(word_hash(&content[start..index]));
            let defines = DEFINITION_KEYWORDS.iter().any(|keyword| previous_word.eq_ignore_ascii_case(keyword.as_bytes()));
            if after < content.len() && content[after] == b'(' && !defines {
                found.push((String::from_utf8_lossy(&content[start..index]).to_lowercase(), line));
            }
            previous_word = &content[start..index];
            continue;
        }
        index += 1;
    }
    words.sort_unstable();
    words.dedup();
    (found, words)
}

impl GenericIndex {
    fn new(repo_root: PathBuf, claimed_extensions: HashSet<String>) -> Self {
        let mut index = Self { repo_root, claimed_extensions, calls: HashMap::new(), names_by_file: HashMap::new(), words_by_file: HashMap::new() };
        let files: Vec<PathBuf> = WalkBuilder::new(&index.repo_root)
            .hidden(false)
            .filter_entry(|entry| entry.file_name() != ".git")
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(|entry| entry.into_path())
            .collect();
        for file in files {
            if let Ok(relative) = file.strip_prefix(&index.repo_root) {
                let relative = relative.to_string_lossy().into_owned();
                index.read(&relative);
            }
        }
        index
    }

    fn handles(&self, path: &str) -> bool {
        let extension = Path::new(path).extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
        !self.claimed_extensions.contains(&extension) && !NOT_CODE.contains(&extension.as_str())
    }

    fn forget(&mut self, path: &str) {
        self.words_by_file.remove(path);
        for name in self.names_by_file.remove(path).unwrap_or_default() {
            if let Some(sites) = self.calls.get_mut(&name) {
                sites.retain(|(file, _)| file != path);
            }
        }
    }

    fn read(&mut self, path: &str) {
        self.forget(path);
        if !self.handles(path) {
            return;
        }
        let absolute = self.repo_root.join(path);
        let Ok(mut file) = fs::File::open(&absolute) else { return };
        let mut head = [0u8; 8192];
        let read = std::io::Read::read(&mut file, &mut head).unwrap_or(0);
        if head[..read].contains(&0) {
            return; // binary, not code
        }
        let Ok(content) = fs::read(&absolute) else { return };
        let mut names = Vec::new();
        let (calls, words) = call_like_names(&content);
        self.words_by_file.insert(path.to_owned(), words);
        for (name, line) in calls {
            self.calls.entry(name.clone()).or_default().push((path.to_owned(), line));
            names.push(name);
        }
        names.sort();
        names.dedup();
        self.names_by_file.insert(path.to_owned(), names);
    }

    fn callers(&self, symbol: &str) -> Value {
        let mut parts = symbol.rsplit(|character: char| matches!(character, ':' | '.' | '\\' | '#' | '/')).filter(|part| !part.is_empty());
        let name = parts.next().unwrap_or(symbol).to_lowercase();
        // `Orders_model::get`: a `get()` can only be that one in a file that names the class, and most likely on a
        // line that names it (`$this->orders_model->get()`). When the repository calls it that way anywhere, a
        // `get()` on a line that does not name the class is something else; when it never does, every `get()`
        // of the files naming the class is listed.
        let owner = parts.next().filter(|owner| owner.chars().next().is_some_and(char::is_alphabetic) && !owner.contains(' ')).map(str::to_lowercase);
        let mut by_file: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
        for (file, line) in self.calls.get(&name).into_iter().flatten() {
            by_file.entry(file.as_str()).or_default().push(*line);
        }
        let edge = |file: &str, line: u32, confidence: &str| {
            json!({
                "from": { "symbol": format!("{file}:{line}"), "file": file, "line": line, "kind": "text" },
                "at": { "file": file, "line": line },
                "kind": "text call",
                "confidence": confidence
            })
        };
        let mut edges = Vec::new();
        let mut loose = Vec::new();
        for (file, lines) in by_file {
            let Some(owner) = &owner else {
                edges.extend(lines.iter().map(|&line| edge(file, line, "unresolved")));
                continue;
            };
            if self.words_by_file.get(file).is_none_or(|words| words.binary_search(&word_hash(owner.as_bytes())).is_err()) {
                continue;
            }
            let Ok(text) = fs::read_to_string(self.repo_root.join(file)) else { continue };
            let text = text.to_lowercase();
            let rows: Vec<&str> = text.lines().collect();
            let naming: Vec<u32> = lines.iter().copied().filter(|&line| rows.get(line as usize - 1).is_some_and(|row| row.contains(owner.as_str()))).collect();
            if naming.is_empty() {
                loose.extend(lines.iter().map(|&line| edge(file, line, "unresolved")));
            } else {
                edges.extend(naming.iter().map(|&line| edge(file, line, "likely")));
            }
        }
        if edges.is_empty() {
            edges = loose;
        }
        Value::Array(edges)
    }
}

/// Words a declaration may start with before its keyword (`export default function`, `public static function`).
const MODIFIERS: [&str; 22] = [
    "export", "default", "async", "public", "private", "protected", "internal", "static", "final", "abstract", "readonly", "override", "virtual", "sealed", "open",
    "data", "inline", "pub", "unsafe", "extern", "const", "suspend",
];

/// The name a line declares with one of `keywords`: the keyword comes first, after modifiers only, so neither a
/// comment (`// the class of`), nor a variable (`$class`), nor a call (`$this->function(`) counts. A Go method's
/// receiver (`func (r *Cart) Add`) is skipped.
fn declared(line: &str, keywords: &[&str]) -> Option<String> {
    let mut rest = line.trim_start();
    if ["//", "#", "*", "/*", "--", ";"].iter().any(|comment| rest.starts_with(comment)) {
        return None;
    }
    loop {
        let word: String = rest.chars().take_while(|character| character.is_alphanumeric() || *character == '_').collect();
        if word.is_empty() {
            return None;
        }
        rest = rest[word.len()..].trim_start();
        if keywords.contains(&word.as_str()) {
            break;
        }
        if !MODIFIERS.contains(&word.as_str()) {
            return None;
        }
        // `pub(crate)`.
        if rest.starts_with('(') {
            rest = rest.split_once(')').map_or("", |(_, after)| after).trim_start();
        }
    }
    if rest.starts_with('(') {
        rest = rest.split_once(')').map_or("", |(_, after)| after).trim_start();
    }
    let name: String = rest.chars().take_while(|character| character.is_alphanumeric() || *character == '_').collect();
    (!name.is_empty()).then_some(name)
}

/// The function a line is in, read from the text around it: the nearest `function`/`def`/`fn`/`func`/`sub`
/// declared above it, named after the nearest `class` declared above that one and indented less
/// (`Orders_model::get`). A guess, said as one.
fn locate(repo_root: &Path, file: &str, line: usize) -> Value {
    let Ok(text) = fs::read_to_string(repo_root.join(file)) else { return Value::Null };
    let lines: Vec<&str> = text.lines().collect();
    let indent = |text: &str| text.len() - text.trim_start().len();
    let top = line.min(lines.len());
    let Some((at, function)) = (0..top).rev().find_map(|index| declared(lines[index], &["function", "def", "fn", "func", "sub"]).map(|name| (index, name))) else {
        return json!({ "symbol": format!("{file}#(file)"), "file": file, "line": 1, "kind": "file" });
    };
    let owner = (0..at).rev().filter(|index| indent(lines[*index]) < indent(lines[at])).find_map(|index| declared(lines[index], &["class", "trait", "interface", "struct", "impl"]));
    let symbol = owner.map_or(function.clone(), |owner| format!("{owner}::{function}"));
    json!({ "symbol": symbol, "file": file, "line": at + 1, "kind": "text" })
}

pub fn serve(repo_root: &Path, claimed_extensions: HashSet<String>) {
    let mut index = GenericIndex::new(repo_root.to_path_buf(), claimed_extensions);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(request) = serde_json::from_str::<Value>(&line) else { continue };
        let result = match request.get("op").and_then(Value::as_str).unwrap_or("") {
            "probe" => json!({ "driver": "generic", "filesRead": index.names_by_file.len() }),
            "sync" => {
                for path in request.get("changed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                    index.read(path);
                }
                json!({})
            }
            "locate" => locate(&index.repo_root, request.get("file").and_then(Value::as_str).unwrap_or(""), request.get("line").and_then(Value::as_u64).unwrap_or(1) as usize),
            // Only symbols no language driver recognised: for the others it would add every same-named call
            // in unrelated languages, which no reader can use.
            "callers" if request.get("unclaimed").and_then(Value::as_bool) == Some(true) => {
                index.callers(request.get("symbol").and_then(Value::as_str).unwrap_or(""))
            }
            _ => Value::Null,
        };
        let _ = writeln!(stdout, "{}", json!({ "id": request.get("id"), "result": result }));
        let _ = stdout.flush();
    }
}
