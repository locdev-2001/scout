//! Driver for languages whose own tooling speaks the Language Server Protocol (rust-analyzer, clangd, ...).
//! One implementation; each language is a `driver.json` with an `lsp` block naming its server.
//!
//! The server is the authority: `textDocument/references` lists what it resolved to the target. A call
//! site spelled like the target (`name(` or `name::<`) that the server resolves nowhere is listed as
//! `unresolved`, never dropped; one it resolves to another definition is left out on the server's word.
//! Files the core reports changed are handed to the server as open documents with their current text, so
//! every answer reflects the files on disk as they are now.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// Longest wait for a server to finish loading a project before answering anyway.
const LOAD_LIMIT: Duration = Duration::from_secs(180);
/// How long a server that reports its status may take to report a change as work in progress.
const CHANGE_NOTICE: Duration = Duration::from_millis(150);
/// The language server's process id (0 before it starts), for the orphan watch.
static LANGUAGE_SERVER: AtomicU32 = AtomicU32::new(0);

/// When the scout server that started this driver dies (killed, crashed), the driver may be deep in a
/// wait on the language server and never read its closed input: a watch ends both, so no language server holding
/// gigabytes outlives its user.
fn watch_parent() {
    #[cfg(unix)]
    {
        let parent = std::os::unix::process::parent_id();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(1));
            if std::os::unix::process::parent_id() != parent {
                let server = LANGUAGE_SERVER.load(Ordering::Relaxed);
                if server != 0 {
                    crate::resources::kill_tree(server);
                }
                std::process::exit(0);
            }
        });
    }
}

#[derive(Deserialize)]
struct LspConfig {
    command: Vec<String>,
    language_id: String,
    extensions: Vec<String>,
    #[serde(default)]
    initialization_options: Value,
    /// Environment for the server; `{cache}` is this repository's folder in the user's cache directory.
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Files outside `extensions` whose change reshapes the project (`Cargo.toml`), sent as watched-file events.
    #[serde(default)]
    project_files: Vec<String>,
}

#[derive(Deserialize)]
struct Manifest {
    lsp: LspConfig,
}

/// A named symbol of one file, from `textDocument/documentSymbol`.
#[derive(Clone, Default)]
struct FileSymbol {
    key: String,
    start: u32,
    end: u32,
    name_line: u32,
    name_character: u32,
    /// For a method in `impl Trait for Type`: `Trait::method`, which calls through `dyn Trait` or a generic
    /// bound resolve to instead of this method.
    implements: Option<String>,
    /// LSP SymbolKind (6 method, 9 constructor, 12 function: the ones with a body of statements)
    kind: u64,
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    inbox: Receiver<Value>,
    next_id: u64,
    /// rust-analyzer reports when it has finished loading; servers without that status are taken as ready.
    reports_status: bool,
    quiescent: bool,
    utf8_positions: bool,
    token_types: Vec<String>,
    token_modifiers: Vec<String>,
    /// When documents were last changed, and the latest status the server reported since.
    changed_at: Option<Instant>,
    status_since_change: Option<bool>,
    /// Why the server could not load the project, when it says so.
    trouble: Option<String>,
}

fn uri_of(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let encoded: String = text.chars().map(|character| if character == ' ' { "%20".to_owned() } else { character.to_string() }).collect();
    if encoded.starts_with('/') { format!("file://{encoded}") } else { format!("file:///{encoded}") }
}

fn path_of(uri: &str) -> PathBuf {
    let raw = uri.strip_prefix("file://").unwrap_or(uri).replace("%20", " ");
    // `file:///C:/x` on Windows
    let raw = if raw.len() > 3 && raw.as_bytes()[0] == b'/' && raw.as_bytes()[2] == b':' { raw[1..].to_owned() } else { raw };
    PathBuf::from(raw)
}

impl Server {
    fn start(config: &LspConfig, repo_root: &Path) -> Result<Self, String> {
        let (program, args) = config.command.split_first().ok_or("lsp.command is empty")?;
        let cache = crate::daemon::cache_directory().join(format!("{:016x}", repo_root.to_string_lossy().bytes().fold(0xcbf2_9ce4_8422_2325u64, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))));
        let mut command = Command::new(program);
        command.args(args).current_dir(repo_root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        for (name, value) in &config.env {
            command.env(name, value.replace("{cache}", &cache.to_string_lossy()));
        }
        let mut child = command.spawn().map_err(|error| format!("cannot start {program}: {error}"))?;
        LANGUAGE_SERVER.store(child.id(), Ordering::Relaxed);
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (sender, inbox) = channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                if let Ok(message) = serde_json::from_slice::<Value>(&body) {
                    if sender.send(message).is_err() {
                        return;
                    }
                }
            }
        });
        let mut server = Self { child, stdin, inbox, next_id: 1, reports_status: false, quiescent: false, utf8_positions: false, token_types: Vec::new(), token_modifiers: Vec::new(), changed_at: None, status_since_change: None, trouble: None };
        let capabilities = json!({
            "general": { "positionEncodings": ["utf-8", "utf-16"] },
            "textDocument": {
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "references": {},
                "definition": {},
                "semanticTokens": { "requests": { "full": true }, "tokenTypes": [], "tokenModifiers": [], "formats": ["relative"] }
            },
            "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": false } },
            "experimental": { "serverStatusNotification": true }
        });
        let initialized = server.request("initialize", json!({
            "processId": std::process::id(),
            "rootUri": uri_of(repo_root),
            "workspaceFolders": [{ "uri": uri_of(repo_root), "name": "repo" }],
            "capabilities": capabilities,
            "initializationOptions": config.initialization_options,
        }))?;
        server.utf8_positions = initialized.pointer("/capabilities/positionEncoding").and_then(Value::as_str) == Some("utf-8");
        let legend = |field: &str| initialized.pointer(&format!("/capabilities/semanticTokensProvider/legend/{field}")).and_then(|names| serde_json::from_value(names.clone()).ok()).unwrap_or_default();
        server.token_types = legend("tokenTypes");
        server.token_modifiers = legend("tokenModifiers");
        server.notify("initialized", json!({}));
        Ok(server)
    }

    fn write(&mut self, message: &Value) {
        let body = message.to_string();
        let _ = write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = self.stdin.flush();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Server-to-client traffic: status, progress, and requests the server expects an answer to.
    fn handle(&mut self, message: &Value) {
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        if method == "experimental/serverStatus" {
            self.reports_status = true;
            self.quiescent = message.pointer("/params/quiescent").and_then(Value::as_bool).unwrap_or(false);
            // `health: error` with a message: the project could not be loaded as configured (a missing
            // toolchain, a broken manifest); answers then rest on what the server could read.
            let health = message.pointer("/params/health").and_then(Value::as_str).unwrap_or("ok");
            self.trouble = (health == "error").then(|| message.pointer("/params/message").and_then(Value::as_str).unwrap_or("").lines().take(3).collect::<Vec<_>>().join(" "));
            if self.changed_at.is_some() {
                self.status_since_change = Some(self.quiescent);
            }
        }
        if let Some(id) = message.get("id").filter(|_| !method.is_empty()) {
            let result = if method == "workspace/configuration" {
                Value::Array(vec![Value::Null; message.pointer("/params/items").and_then(Value::as_array).map_or(0, Vec::len)])
            } else {
                Value::Null
            };
            self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    }

    /// Sends every request before reading any answer: the server works on them in parallel.
    fn request_many(&mut self, requests: Vec<(&str, Value)>) -> Vec<Result<Value, String>> {
        let first = self.next_id;
        for (method, params) in &requests {
            let id = self.next_id;
            self.next_id += 1;
            self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        }
        let mut answers: HashMap<u64, Result<Value, String>> = HashMap::new();
        while answers.len() < requests.len() {
            let Ok(message) = self.inbox.recv() else {
                return vec![Err("language server exited".to_owned()); requests.len()];
            };
            let id = message.get("id").and_then(Value::as_u64);
            match id {
                Some(id) if message.get("method").is_none() && id >= first && id < first + requests.len() as u64 => {
                    let answer = match message.get("error") {
                        Some(error) => Err(error.get("message").and_then(Value::as_str).unwrap_or("error").to_owned()),
                        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    answers.insert(id, answer);
                }
                _ => self.handle(&message),
            }
        }
        (first..first + requests.len() as u64).map(|id| answers.remove(&id).unwrap()).collect()
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.request_many(vec![(method, params)]).pop().unwrap()
    }

    /// Waits until the server finished loading the project (when it says so), handling traffic meanwhile.
    /// Bounded: a server that never reports again is answered from what it has.
    fn wait_until_loaded(&mut self) {
        let started = Instant::now();
        // A server that reports status says so within its first moments; give it a short chance to.
        while !self.quiescent && (self.reports_status || started.elapsed() < Duration::from_millis(300)) && started.elapsed() < LOAD_LIMIT {
            match self.inbox.recv_timeout(Duration::from_millis(50)) {
                Ok(message) => self.handle(&message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        // After document changes rust-analyzer reports itself busy within milliseconds and idle again when
        // done; `references` asked in between missed uses (seen on a fixture, 3 of 3). Wait for idle.
        if let Some(changed_at) = self.changed_at {
            while self.status_since_change != Some(true) && started.elapsed() < LOAD_LIMIT {
                if self.status_since_change.is_none() && changed_at.elapsed() > CHANGE_NOTICE {
                    break; // the server did not report this change as work
                }
                match self.inbox.recv_timeout(Duration::from_millis(10)) {
                    Ok(message) => self.handle(&message),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            self.changed_at = None;
            self.status_since_change = None;
        }
    }

    /// Takes whatever arrived while idle (status changes), without waiting.
    fn drain(&mut self) {
        while let Ok(message) = self.inbox.try_recv() {
            self.handle(&message);
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

/// One identifier in a source file.
struct Mention {
    file: PathBuf,
    line: u32,
    byte_column: u32,
    utf16_column: u32,
    /// `name(` or `name::<`
    call_like: bool,
    /// `fn name`: a definition
    definition: bool,
}

/// Which files name each identifier, and every file's lines: where a name occurs is found by reading those lines
/// again, which costs far less memory than keeping every occurrence (tens of millions in a large workspace). A
/// changed file is indexed again when the core reports it.
#[derive(Default)]
struct Mentions {
    files_by_name: HashMap<String, Vec<Arc<Path>>>,
    names_by_file: HashMap<PathBuf, Vec<String>>,
    lines_by_file: HashMap<PathBuf, Vec<String>>,
}

/// Byte ranges of the identifiers of a line.
fn identifiers(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut found = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if !(bytes[index].is_ascii_alphabetic() || bytes[index] == b'_') || (index > 0 && is_identifier_byte(bytes[index - 1])) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_identifier_byte(bytes[index]) {
            index += 1;
        }
        found.push((start, index));
    }
    found
}

impl Mentions {
    fn forget(&mut self, file: &Path) {
        self.lines_by_file.remove(file);
        for name in self.names_by_file.remove(file).unwrap_or_default() {
            if let Some(files) = self.files_by_name.get_mut(&name) {
                files.retain(|named| named.as_ref() != file);
                if files.is_empty() {
                    self.files_by_name.remove(&name);
                }
            }
        }
    }

    fn read(&mut self, file: &Path) {
        self.forget(file);
        let Ok(content) = fs::read_to_string(file) else { return };
        let lines: Vec<String> = content.lines().map(str::to_owned).collect();
        let mut names: Vec<String> = lines.iter().flat_map(|line| identifiers(line).into_iter().map(|(start, end)| line[start..end].to_owned())).collect();
        names.sort();
        names.dedup();
        let shared: Arc<Path> = Arc::from(file);
        for name in &names {
            self.files_by_name.entry(name.clone()).or_default().push(Arc::clone(&shared));
        }
        self.names_by_file.insert(file.to_path_buf(), names);
        self.lines_by_file.insert(file.to_path_buf(), lines);
    }

    fn knows(&self, name: &str) -> bool {
        self.files_by_name.contains_key(name)
    }

    /// Every occurrence of `name`, read from the lines of the files that have it.
    fn of(&self, name: &str) -> Vec<Mention> {
        let mut found = Vec::new();
        for file in self.files_by_name.get(name).into_iter().flatten() {
            self.collect(name, file, &mut found);
        }
        found
    }

    fn collect(&self, name: &str, file: &Path, found: &mut Vec<Mention>) {
        for (line_index, line) in self.lines_by_file.get(file).into_iter().flatten().enumerate() {
            if !line.contains(name) {
                continue;
            }
            for (start, end) in identifiers(line) {
                if &line[start..end] != name {
                    continue;
                }
                let rest = line[end..].trim_start();
                found.push(Mention {
                    file: file.to_path_buf(),
                    line: line_index as u32,
                    byte_column: start as u32,
                    utf16_column: line[..start].encode_utf16().count() as u32,
                    call_like: calls(rest),
                    definition: is_definition(line, start),
                });
            }
        }
    }

    /// (line, call-like) of every mention of `name` in `file`.
    fn in_file(&self, name: &str, file: &Path) -> Vec<(u32, bool)> {
        let mut found = Vec::new();
        self.collect(name, file, &mut found);
        found.into_iter().map(|mention| (mention.line, mention.call_like)).collect()
    }
}

/// What an edit did to one file: the lines it kept at the start and the end, the function bodies it touched,
/// and the searched names the file mentioned before and after.
struct FileEdit {
    path: PathBuf,
    kept_start: u32,
    old_end: u32,
    new_end: u32,
    delta: i64,
    /// Declarations, imports or attributes changed in place: names may now resolve differently anywhere here.
    touches_items: bool,
    /// Whole items added or removed between others: only the names written in them may resolve differently.
    item_names: HashSet<String>,
    /// Old line ranges of the function bodies the edit lies in.
    edited_bodies: Vec<(u32, u32)>,
    old_mentions: HashMap<String, Vec<(u32, bool)>>,
    new_mentions: HashMap<String, Vec<(u32, bool)>>,
}

impl FileEdit {
    fn new(path: PathBuf, old: &[String], new: &[String], old_symbols: Option<Vec<FileSymbol>>, old_mentions: HashMap<String, Vec<(u32, bool)>>, new_mentions: HashMap<String, Vec<(u32, bool)>>) -> Self {
        let mut start = 0;
        while start < old.len() && start < new.len() && old[start] == new[start] {
            start += 1;
        }
        let mut end = 0;
        while end < old.len() - start && end < new.len() - start && old[old.len() - 1 - end] == new[new.len() - 1 - end] {
            end += 1;
        }
        let (old_end, new_end) = (old.len() - end, new.len() - end);
        let touches = old[start..old_end].iter().chain(&new[start..new_end]).any(|line| touches_items(line));
        // Added (or removed) lines with no line changed around them, between items rather than in a body.
        // An insertion between two lines lies in a body that holds both of them.
        let (low, high) = if old_end > start { (start as u32, old_end as u32 - 1) } else { ((start as u32).saturating_sub(1), start as u32) };
        let edited_bodies = old_symbols
            .as_ref()
            .map(|symbols| symbols.iter().filter(|symbol| matches!(symbol.kind, 6 | 9 | 12) && symbol.start <= low && symbol.end >= high).map(|symbol| (symbol.start, symbol.end)).collect())
            .unwrap_or_default();
        let between_items = old_symbols.is_some() && edited_bodies_is_empty(&old_symbols, start as u32, old_end as u32);
        let only_added_or_removed = old_end == start || new_end == start;
        let item_names: HashSet<String> = if touches && between_items && only_added_or_removed {
            old[start..old_end].iter().chain(&new[start..new_end]).flat_map(|line| line.split(|character: char| !character.is_alphanumeric() && character != '_').filter(|word| !word.is_empty()).map(str::to_owned).collect::<Vec<_>>()).collect()
        } else {
            HashSet::new()
        };
        let touches = touches && item_names.is_empty();
        Self {
            item_names,
            path,
            kept_start: start as u32,
            old_end: old_end as u32,
            new_end: new_end as u32,
            delta: new.len() as i64 - old.len() as i64,
            // Without the file's symbols from before the edit, its bodies are unknown: treat it as touching items.
            touches_items: touches || old_symbols.is_none(),
            edited_bodies,
            old_mentions,
            new_mentions,
        }
    }

    /// Where a line (zero-based) before the edit is now; None inside the edit.
    fn map_old(&self, line: u32) -> Option<u32> {
        if line < self.kept_start {
            Some(line)
        } else if line >= self.old_end {
            Some((line as i64 + self.delta) as u32)
        } else {
            None
        }
    }

    fn unmap_new(&self, line: u32) -> Option<u32> {
        if line < self.kept_start {
            Some(line)
        } else if line >= self.new_end {
            Some((line as i64 - self.delta) as u32)
        } else {
            None
        }
    }

    fn in_edited_body(&self, old_line: u32) -> bool {
        self.edited_bodies.iter().any(|(start, end)| *start <= old_line && old_line <= *end)
    }

    fn mentions_any(&self, names: &[String]) -> bool {
        names.iter().any(|name| self.old_mentions.contains_key(name) || self.new_mentions.contains_key(name))
    }

    /// An answer that searched these names stays true with moved lines: no item changed, and the file mentions
    /// the names at the same places as before, none inside an edited body.
    fn keeps(&self, names: &[String]) -> bool {
        if self.touches_items || names.iter().any(|name| self.item_names.contains(name)) {
            return false;
        }
        names.iter().all(|name| {
            let before = self.old_mentions.get(name).cloned().unwrap_or_default();
            let after = self.new_mentions.get(name).cloned().unwrap_or_default();
            let moved: Option<Vec<(u32, bool)>> = before.iter().map(|(line, call)| self.map_old(*line).filter(|_| !self.in_edited_body(*line)).map(|line| (line, *call))).collect();
            let unmoved: Option<Vec<u32>> = after.iter().map(|(line, _)| self.unmap_new(*line).filter(|old| !self.in_edited_body(*old))).collect();
            unmoved.is_some() && moved.is_some_and(|moved| moved == after)
        })
    }
}

/// Whether an edit spanning old lines [start, end) lies outside every body of statements.
fn edited_bodies_is_empty(symbols: &Option<Vec<FileSymbol>>, start: u32, end: u32) -> bool {
    let (low, high) = if end > start { (start, end - 1) } else { (start.saturating_sub(1), start) };
    symbols.iter().flatten().all(|symbol| !(matches!(symbol.kind, 6 | 9 | 12) && symbol.start <= low && symbol.end >= high))
}

/// One `callers` question while its batch is answered.
#[derive(Default)]
struct Pending {
    index: usize,
    path: PathBuf,
    target: FileSymbol,
    /// The target's name, then aliases a `use ... as` gives it.
    names: Vec<String>,
    alias_lines: HashSet<(PathBuf, u32)>,
    covered: HashSet<(PathBuf, u32)>,
    found: Vec<(PathBuf, u32, &'static str, &'static str)>,
    /// (file, line, column, call-like, through an alias)
    candidates: Vec<(PathBuf, u32, u32, bool, bool)>,
    judged: Vec<(PathBuf, u32, bool, Vec<Value>)>,
}

/// Source lines read once per batch.
#[derive(Default)]
struct SourceLines(HashMap<PathBuf, Vec<String>>);

impl SourceLines {
    fn line(&mut self, file: &Path, line: u32) -> String {
        let lines = self.0.entry(file.to_path_buf()).or_insert_with(|| fs::read_to_string(file).unwrap_or_default().lines().map(str::to_owned).collect());
        lines.get(line as usize).cloned().unwrap_or_default()
    }
}

/// One semantic token of a file: (line, start, length, type, modifier bits).
type SemanticToken = (u32, u32, u32, u32, u32);

struct LspDriver {
    repo_root: PathBuf,
    config: LspConfig,
    server: Option<Server>,
    failure: Option<String>,
    /// Open documents and their version: files the core reported changed are given to the server with their text.
    opened: HashMap<PathBuf, i64>,
    symbols: HashMap<PathBuf, Vec<FileSymbol>>,
    /// Answered `callers` symbols with what their answer read: the target's file and the names searched for.
    answered: HashMap<String, (PathBuf, Vec<String>)>,
    mentions: Mentions,
    tokens: HashMap<PathBuf, Vec<SemanticToken>>,
    primed: bool,
    /// The last answer of each `callers` question, kept while no change could alter it (or only moved lines).
    answers: HashMap<String, Value>,
    /// Questions whose answer a change discarded, to answer again while idle.
    stale: Vec<String>,
    /// Every function of the repository, answered ahead while idle; None until the server has loaded.
    warm_up: Option<Vec<String>>,
    /// The workspace's crates when the project is a Cargo one: which crates can reach a target at all.
    crates: Option<crate::cargo::CrateGraph>,
}

/// LSP symbol kinds that can be called: method, constructor, function.
const CALLABLE_KINDS: [u64; 3] = [6, 9, 12];

/// Files of a crate other than the library (`src/main.rs`, `src/bin/`, examples, benches, build scripts). A module
/// such a crate declares again (`mod api;` in main.rs) is a second copy the server resolves within its own crate,
/// so a reference search from the library's copy misses calls written in these files.
fn outside_library(relative: &str) -> bool {
    let parts: Vec<&str> = relative.split('/').collect();
    relative.ends_with("src/main.rs") || relative.ends_with("build.rs") || parts.windows(2).any(|pair| pair == ["src", "bin"]) || parts.iter().any(|part| *part == "examples" || *part == "benches")
}

/// How the language calls a trait method without writing its name.
struct ImplicitUse {
    label: &'static str,
    uses: fn(&str) -> bool,
    /// Columns of the values whose type decides which implementation runs, when the server can say it (`*x`: the
    /// type of `x`); without it, any line of a function naming the type is listed.
    operands: Option<fn(&str) -> Vec<usize>>,
    /// A conversion is chosen by the types a function declares; the others by any value of the type in it.
    signature_only: bool,
}

const IMPLICIT_USES: [(&str, ImplicitUse); 8] = [
    ("From::from", ImplicitUse { label: "implicit From", uses: converts, signature_only: true, operands: None }),
    ("Display::fmt", ImplicitUse { label: "implicit Display", uses: displays, signature_only: false, operands: None }),
    ("Debug::fmt", ImplicitUse { label: "implicit Debug", uses: debug_prints, signature_only: false, operands: None }),
    ("Iterator::next", ImplicitUse { label: "implicit Iterator", uses: loops, signature_only: false, operands: None }),
    ("IntoIterator::into_iter", ImplicitUse { label: "implicit IntoIterator", uses: loops, signature_only: false, operands: None }),
    ("Deref::deref", ImplicitUse { label: "implicit Deref", uses: dereferences, signature_only: false, operands: Some(deref_operands) }),
    ("DerefMut::deref_mut", ImplicitUse { label: "implicit DerefMut", uses: dereferences, signature_only: false, operands: Some(deref_operands) }),
    ("Drop::drop", ImplicitUse { label: "implicit Drop", uses: drops, signature_only: false, operands: None }),
];

/// By the trait's own name, however its path is written (`std::fmt::Display::fmt`, `fmt::Display::fmt`).
fn implicit_use(implements: &str) -> Option<&'static ImplicitUse> {
    let parts: Vec<&str> = implements.rsplit("::").take(2).collect();
    let name = parts.iter().rev().copied().collect::<Vec<_>>().join("::");
    IMPLICIT_USES.iter().find(|(known, _)| *known == name).map(|(_, implicit)| implicit)
}

/// What follows a name when it is called: `(`, `::<` (turbofish), or `!` with its brackets (a macro).
fn calls(after: &str) -> bool {
    after.starts_with('(') || after.starts_with("::<") || ["!(", "![", "!{"].iter().any(|opening| after.replace(' ', "").starts_with(opening))
}

fn code_of(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}

/// The `?` operator or `.into()`.
fn converts(line: &str) -> bool {
    let code = code_of(line);
    code.contains(".into()") || ["?;", "?)", "?.", "?,", "? "].iter().any(|operator| code.contains(operator)) || code.trim_end().ends_with('?')
}

/// A formatting macro with a `{}` or `{name}` placeholder, or `.to_string()`.
fn displays(line: &str) -> bool {
    let code = code_of(line);
    let formats = ["format!(", "print!(", "println!(", "eprint!(", "eprintln!(", "write!(", "writeln!(", "panic!(", "format_args!("].iter().any(|name| code.contains(name));
    let placeholder = code.match_indices('{').any(|(start, _)| code[start + 1..].split('}').next().is_some_and(|inside| !inside.contains('?') && !inside.contains('{') && !inside.contains(' ')));
    // `"text".to_string()` turns a literal, not a value of the type.
    let converted = code.match_indices(".to_string()").any(|(start, _)| !code[..start].ends_with('"'));
    converted || (formats && placeholder)
}

/// `{:?}` in a formatting macro, or `dbg!`.
fn debug_prints(line: &str) -> bool {
    let code = code_of(line);
    code.contains(":?}") || code.contains(":#?}") || code.contains("dbg!(")
}

/// A unary `*value` (not a multiplication: nothing that ends a value stands right before the star).
fn dereferences(line: &str) -> bool {
    !deref_operands(line).is_empty()
}

/// Byte columns of the operands of unary `*`, not of multiplication: the last name of the path (`inner` in
/// `*self.inner`), whose type is the one dereferenced.
fn deref_operands(line: &str) -> Vec<usize> {
    let code = code_of(line);
    let bytes = code.as_bytes();
    (0..bytes.len())
        .filter(|&index| bytes[index] == b'*')
        .filter(|&index| {
            let operand = bytes.get(index + 1).is_some_and(|&next| next == b'(' || next == b'_' || next.is_ascii_alphabetic());
            let before = code[..index].trim_end().bytes().next_back();
            operand && before.is_none_or(|previous| !(is_identifier_byte(previous) || previous == b')' || previous == b']'))
        })
        .map(|index| {
            let mut last = index + 1;
            let mut at = index + 1;
            while at < bytes.len() && (is_identifier_byte(bytes[at]) || bytes[at] == b'.' || (bytes[at] == b':' && bytes.get(at + 1) == Some(&b':'))) {
                if bytes[at] == b'.' || bytes[at] == b':' {
                    at += if bytes[at] == b':' { 2 } else { 1 };
                    last = at;
                } else {
                    at += 1;
                }
            }
            last
        })
        .collect()
}

/// `for x in value`.
fn loops(line: &str) -> bool {
    let code = code_of(line).trim_start();
    code.starts_with("for ") && code.contains(" in ")
}

/// `drop(value)` (every value is also dropped where its scope ends, which no line spells).
fn drops(line: &str) -> bool {
    let code = code_of(line);
    code.match_indices("drop(").any(|(start, _)| code[..start].chars().next_back().is_none_or(|previous| !(previous.is_alphanumeric() || previous == '_' || previous == '.')))
}

/// Whether `word` appears in `text` as a whole identifier.
fn names_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(start, _)| {
        let before = text[..start].bytes().next_back().is_none_or(|byte| !is_identifier_byte(byte));
        let after = text[start + word.len()..].bytes().next().is_none_or(|byte| !is_identifier_byte(byte));
        before && after
    })
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Keywords that introduce a named declaration in the languages this driver serves.
const DECLARATION_KEYWORDS: [&str; 16] = ["fn", "def", "func", "function", "void", "struct", "enum", "trait", "union", "mod", "type", "const", "static", "class", "interface", "macro_rules!"];

/// Words that start an item rather than a statement inside a body: an edit touching one may change how names
/// resolve anywhere in the file.
const ITEM_WORDS: [&str; 15] = ["fn", "struct", "enum", "trait", "impl", "mod", "use", "type", "const", "static", "macro_rules", "union", "extern", "pub", "where"];

fn touches_items(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("#[") || trimmed.starts_with("#![") || line.split(|character: char| !character.is_alphanumeric() && character != '_' && character != '!').any(|word| ITEM_WORDS.contains(&word.trim_end_matches('!')))
}

/// `fn name(`: a definition, not a call (keywords that introduce one in the languages this driver serves).
fn is_definition(line: &str, name_start: usize) -> bool {
    let before = line[..name_start].trim_end();
    DECLARATION_KEYWORDS.iter().any(|keyword| before.ends_with(keyword) && before[..before.len() - keyword.len()].chars().last().is_none_or(|character| !character.is_alphanumeric() && character != '_'))
}

/// The module path a Rust source file defines: `src/util.rs` and `src/util/mod.rs` are `util`; `src/main.rs`
/// and `src/lib.rs` are the crate root.
fn module_path(relative: &str) -> String {
    let inside = relative.split_once("src/").map_or(relative, |(_, inside)| inside);
    let inside = inside.trim_end_matches(".rs").trim_end_matches("/mod");
    if inside == "main" || inside == "lib" { String::new() } else { inside.replace('/', "::") }
}

/// `impl Trait for Type` and `impl<T> Type<T>` are both reached as `Type`.
fn container_segment(name: &str) -> String {
    let name = match name.strip_prefix("impl") {
        Some(rest) if rest.starts_with(' ') || rest.starts_with('<') => rest.rsplit(" for ").next().unwrap_or(rest).trim(),
        _ => name,
    };
    let name = name.trim_start_matches(|character: char| character == '<' || character == '>' || character.is_whitespace());
    name.split(['<', ' ', '(']).next().unwrap_or(name).to_owned()
}

impl LspDriver {
    fn handles(&self, path: &Path) -> bool {
        path.extension().map(|extension| extension.to_string_lossy().to_lowercase()).is_some_and(|extension| self.config.extensions.contains(&extension))
    }

    /// Once, before the first answer: every file's highlighting, asked together. rust-analyzer then has every
    /// body analysed, so a later `references` takes tens of milliseconds instead of hundreds (measured on a
    /// 165-file crate: 4 s for all files, then 25-40 ms per symbol), and the tokens serve the answers too.
    fn prime(&mut self) {
        if self.primed {
            return;
        }
        self.primed = true;
        let started = Instant::now();
        let files: Vec<PathBuf> = self.source_files().into_iter().filter(|file| !self.tokens.contains_key(file)).collect();
        self.fetch_tokens(&files);
        crate::resources::profile_phase(&format!("lsp primed: {} files", files.len()), started);
    }

    /// Only once the server runs: a repository that is mostly another language (a TypeScript monorepo with one
    /// crate) never pays for rust-analyzer until a question is about its files.
    fn warm_up_pending(&self) -> bool {
        self.server.is_some() && self.failure.is_none() && self.warm_up.as_ref().is_none_or(|queue| !queue.is_empty())
    }

    /// The warm-up: who calls each function of the repository, worked out before anyone asks (a widely used
    /// function's first answer otherwise waits on rust-analyzer's slowest searches). Answers stay valid until
    /// an edit touches them, like any other.
    fn warm_up_step(&mut self) {
        const BATCH: usize = 8;
        if self.warm_up.is_none() {
            let started = Instant::now();
            self.prime();
            let mut queue = Vec::new();
            for file in self.source_files() {
                let relative = self.relative(&file).unwrap_or_default();
                for symbol in self.file_symbols(&file) {
                    if matches!(symbol.kind, 6 | 9 | 12) {
                        queue.push(format!("{relative}#{}", symbol.key));
                    }
                }
            }
            queue.reverse();
            crate::resources::profile_phase(&format!("lsp warm-up queued: {} functions", queue.len()), started);
            self.warm_up = Some(queue);
            return;
        }
        let batch: Vec<String> = {
            let queue = self.warm_up.as_mut().unwrap();
            let take = queue.len().min(BATCH);
            queue.split_off(queue.len() - take).into_iter().filter(|symbol| !self.answers.contains_key(symbol)).collect()
        };
        if !batch.is_empty() {
            self.callers_batch(&batch);
        }
        if self.warm_up.as_ref().is_some_and(Vec::is_empty) {
            crate::resources::profile_phase("lsp warm-up: done", Instant::now());
        }
    }

    fn server(&mut self) -> Result<&mut Server, String> {
        if self.server.is_none() && self.failure.is_none() {
            match Server::start(&self.config, &self.repo_root) {
                Ok(server) => self.server = Some(server),
                Err(error) => self.failure = Some(error),
            }
        }
        match (&mut self.server, &self.failure) {
            (Some(server), _) => {
                server.drain();
                server.wait_until_loaded();
                Ok(server)
            }
            (None, failure) => Err(failure.clone().unwrap_or_default()),
        }
    }

    /// The crate graph of a Cargo project (its manifests are among the project files); None for other servers.
    fn read_crates(&mut self) {
        if !self.config.project_files.iter().any(|name| name == "Cargo.toml") {
            return;
        }
        let manifests: Vec<PathBuf> = ignore::WalkBuilder::new(&self.repo_root)
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name() == "Cargo.toml")
            .map(|entry| entry.into_path())
            .collect();
        self.crates = Some(crate::cargo::CrateGraph::read(&manifests));
    }

    fn relative(&self, path: &Path) -> Option<String> {
        path.strip_prefix(&self.repo_root).ok().map(|relative| relative.to_string_lossy().replace('\\', "/"))
    }

    fn file_symbols(&mut self, path: &Path) -> Vec<FileSymbol> {
        if let Some(symbols) = self.symbols.get(path) {
            return symbols.clone();
        }
        let uri = uri_of(path);
        let answer = match self.server() {
            Ok(server) => server.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } })).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        let mut symbols = Vec::new();
        fn walk(items: &[Value], prefix: &str, trait_name: Option<&str>, into: &mut Vec<FileSymbol>) {
            for item in items {
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                let segment = container_segment(name);
                let key = if prefix.is_empty() { segment.clone() } else { format!("{prefix}::{segment}") };
                let range = item.get("range").or_else(|| item.pointer("/location/range"));
                let selection = item.get("selectionRange").or(range);
                let line = |value: Option<&Value>, field: &str| value.and_then(|range| range.pointer(&format!("/{field}/line"))).and_then(Value::as_u64).unwrap_or(0) as u32;
                into.push(FileSymbol {
                    key: key.clone(),
                    start: line(range, "start"),
                    end: line(range, "end"),
                    name_line: line(selection, "start"),
                    name_character: selection.and_then(|range| range.pointer("/start/character")).and_then(Value::as_u64).unwrap_or(0) as u32,
                    implements: trait_name.map(|trait_name| format!("{trait_name}::{segment}")),
                    kind: item.get("kind").and_then(Value::as_u64).unwrap_or(0),
                });
                let implemented = name.strip_prefix("impl").and_then(|rest| rest.split_once(" for ")).map(|(trait_name, _)| container_segment(trait_name.trim()));
                if let Some(children) = item.get("children").and_then(Value::as_array) {
                    walk(children, &key, implemented.as_deref(), into);
                }
            }
        }
        walk(answer.as_array().map(Vec::as_slice).unwrap_or(&[]), "", None, &mut symbols);
        self.symbols.insert(path.to_path_buf(), symbols.clone());
        symbols
    }

    /// The innermost named symbol around a zero-based line, or the file itself.
    fn symbol_at(&mut self, path: &Path, line: u32) -> Value {
        let relative = self.relative(path).unwrap_or_default();
        let around: Vec<FileSymbol> = self.file_symbols(path).into_iter().filter(|symbol| symbol.start <= line && line <= symbol.end).collect();
        let best = around.iter().max_by_key(|symbol| symbol.start).cloned();
        // Test code: a test folder or file (`tests/`, `foo_tests.rs` included through `#[path]`), or inside an item
        // marked `#[test]`, `#[tokio::test]` or `#[cfg(test)]` (its attributes lie between its start and its name).
        let stem = Path::new(&relative).file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
        let lines = self.mentions.lines_by_file.get(path);
        let marked = around.iter().any(|symbol| {
            (symbol.start..symbol.name_line).filter_map(|index| lines.and_then(|lines| lines.get(index as usize))).any(|text| {
                let text = text.trim();
                text.starts_with("#[") && (text.contains("cfg(test)") || text.contains("cfg(all(test") || text == "#[test]" || text.contains("::test]") || text.contains("::test("))
            })
        });
        let is_test = marked || stem.ends_with("_tests") || stem.ends_with("_test") || relative.split('/').any(|part| part == "tests" || part == "tests.rs" || part == "benches");
        let kind = |key: &str| if is_test || key.split("::").any(|part| part == "tests") { json!("test") } else { Value::Null };
        match best {
            Some(symbol) => json!({ "symbol": format!("{relative}#{}", symbol.key), "file": relative, "line": symbol.name_line + 1, "kind": kind(&symbol.key) }),
            None => json!({ "symbol": format!("{relative}#(file)"), "file": relative, "line": 1, "kind": kind("") }),
        }
    }

    fn target(&mut self, symbol: &str) -> Option<(PathBuf, FileSymbol)> {
        let (path, key) = symbol.rsplit_once('#')?;
        let path = self.repo_root.join(path);
        if !self.handles(&path) || !path.is_file() {
            return None;
        }
        if let Some(found) = self.macro_item(&path, key) {
            return Some((path, found));
        }
        // A struct field and a method may share a name (`pg` and `pg()`): the callable one is meant.
        let found = self.file_symbols(&path).into_iter().filter(|candidate| candidate.key == key).max_by_key(|candidate| CALLABLE_KINDS.contains(&candidate.kind))?;
        Some((path, found))
    }

    /// `fill@219`: an item a `macro_rules!` body defines (`fn fill` on line 219). The server lists no symbol for
    /// it; a use of the item resolves to that line.
    fn macro_item(&self, path: &Path, key: &str) -> Option<FileSymbol> {
        let (name, line) = key.rsplit_once('@')?;
        let line = line.parse::<u32>().ok()?.checked_sub(1)?;
        let utf8 = self.server.as_ref().is_some_and(|server| server.utf8_positions);
        let mut mentions = Vec::new();
        self.mentions.collect(name, path, &mut mentions);
        let invoked = self.invokes_macro(path, line);
        let mention = mentions.into_iter().find(|mention| mention.line == line && (mention.definition || invoked))?;
        let character = if utf8 { mention.byte_column } else { mention.utf16_column };
        Some(FileSymbol { key: key.to_owned(), start: line, end: line, name_line: line, name_character: character, implements: None, kind: 12 })
    }

    fn macro_ref(&self, path: &Path, key: &str, line: u32) -> Value {
        let relative = self.relative(path).unwrap_or_default();
        json!({ "symbol": format!("{relative}#{key}"), "file": relative, "line": line + 1 })
    }

    /// Whether a line invokes a macro (`make_getter!(get_one, 1);`): the items it makes are named there.
    fn invokes_macro(&self, path: &Path, line: u32) -> bool {
        self.mentions.lines_by_file.get(path).and_then(|lines| lines.get(line as usize)).is_some_and(|text| ["!(", "! (", "!{", "! {", "!["].iter().any(|opening| code_of(text).contains(opening)))
    }

    fn resolve(&mut self, query: &str) -> Value {
        if query.contains('#') {
            return match self.target(query) {
                // An item a macro makes has no symbol of its own to look up: it is the one asked about.
                Some((path, symbol)) if symbol.key.contains('@') => json!([self.macro_ref(&path, &symbol.key, symbol.name_line)]),
                Some((path, symbol)) => json!([self.symbol_at(&path, symbol.name_line)]),
                None => Value::Null,
            };
        }
        let name = query.rsplit("::").next().unwrap_or(query).to_owned();
        // Declarations the index saw (`fn name`, `struct Name`...): no server round trip.
        let declared: Vec<(PathBuf, u32)> = self.mentions.of(&name).into_iter().filter(|mention| mention.definition).map(|mention| (mention.file, mention.line)).collect();
        let mut results = Vec::new();
        let mut seen = HashSet::new();
        for (path, line) in declared {
            let reference = self.symbol_at(&path, line);
            let symbol = reference.get("symbol").and_then(Value::as_str).unwrap_or("").to_owned();
            let named = symbol.ends_with(&format!("#{name}")) || symbol.ends_with(&format!("::{name}"));
            let matches_query = !query.contains("::") || symbol.ends_with(&format!("#{query}")) || symbol.ends_with(&format!("::{query}"));
            if named && matches_query && seen.insert(symbol) {
                results.push(reference);
            } else if !named && !query.contains("::") && self.in_macro_body(&path, line) {
                let relative = self.relative(&path).unwrap_or_default();
                results.push(json!({ "symbol": format!("{relative}#{name}@{}", line + 1), "file": relative, "line": line + 1 }));
            }
        }
        // A name no source file of this language mentions is not one of its symbols: no need to start the server.
        if !results.is_empty() || !self.mentions.knows(&name) {
            return Value::Array(results);
        }
        let found = match self.server() {
            Ok(server) => server.request("workspace/symbol", json!({ "query": name })).unwrap_or(Value::Null),
            Err(_) => return Value::Null,
        };
        let mut results = Vec::new();
        let mut seen = HashSet::new();
        for item in found.as_array().into_iter().flatten().filter(|item| item.get("name").and_then(Value::as_str) == Some(name.as_str())) {
            let Some(uri) = item.pointer("/location/uri").and_then(Value::as_str) else { continue };
            let path = path_of(uri);
            if self.relative(&path).is_none() {
                continue; // a dependency outside the repository
            }
            let line = item.pointer("/location/range/start/line").and_then(Value::as_u64).unwrap_or(0) as u32;
            // `make_getter!(get_one, 1)`: the server places the function it makes on the invocation.
            if self.invokes_macro(&path, line) && self.mentions.in_file(&name, &path).iter().any(|&(mentioned, _)| mentioned == line) {
                let reference = self.macro_ref(&path, &format!("{name}@{}", line + 1), line);
                if seen.insert(reference["symbol"].as_str().unwrap_or("").to_owned()) {
                    results.push(reference);
                }
                continue;
            }
            let reference = self.symbol_at(&path, line);
            let symbol = reference.get("symbol").and_then(Value::as_str).unwrap_or("").to_owned();
            let matches_query = !query.contains("::") || symbol.ends_with(&format!("#{query}")) || symbol.ends_with(&format!("::{query}"));
            if matches_query && seen.insert(symbol) {
                results.push(reference);
            }
        }
        Value::Array(results)
    }

    /// Whether a line lies in a `macro_rules!` definition (its items exist only once the macro is used).
    fn in_macro_body(&mut self, path: &Path, line: u32) -> bool {
        let lines = self.mentions.lines_by_file.get(path).cloned().unwrap_or_default();
        let mut depth = 0i32;
        let mut inside = false;
        for (index, text) in lines.iter().enumerate() {
            if !inside && text.trim_start().starts_with("macro_rules!") {
                inside = true;
                depth = 0;
            }
            if inside {
                depth += text.matches('{').count() as i32 - text.matches('}').count() as i32;
                if index as u32 == line {
                    return true;
                }
                if depth <= 0 && text.contains('}') {
                    inside = false;
                }
            }
            if index as u32 >= line {
                break;
            }
        }
        false
    }

    fn locate(&mut self, file: &str, line: u32) -> Value {
        let path = self.repo_root.join(file);
        if !self.handles(&path) || !path.is_file() {
            return Value::Null;
        }
        self.symbol_at(&path, line.saturating_sub(1))
    }

    /// Answers a batch of `callers` questions together: every server request of a kind is sent at once
    /// (references, then highlighting, then definitions), so the server works on them in parallel.
    /// The callers of each symbol; an error where the server could not answer (asked again next time, not kept).
    fn callers_batch(&mut self, symbols: &[String]) -> Vec<Result<Value, String>> {
        let started = Instant::now();
        let mut results = vec![Ok(Value::Null); symbols.len()];
        let mut pending: Vec<Pending> = Vec::new();
        for (index, symbol) in symbols.iter().enumerate() {
            if let Some(answer) = self.answers.get(symbol) {
                results[index] = Ok(answer.clone());
                continue;
            }
            let target_file = symbol.rsplit_once('#').map(|(file, _)| self.repo_root.join(file));
            match self.target(symbol) {
                Some((path, target)) => {
                    let name = target.key.rsplit("::").next().unwrap_or(&target.key).split('@').next().unwrap_or_default().to_owned();
                    pending.push(Pending { index, path, names: vec![name], target, ..Pending::default() });
                }
                // Not (or no longer) a symbol of this language; the answer changes only if that file does.
                None => {
                    if let Some(file) = target_file {
                        self.answered.insert(symbol.clone(), (file, Vec::new()));
                    }
                }
            }
        }
        if pending.is_empty() {
            return results;
        }
        self.prime();

        let request = |item: &Pending| ("textDocument/references", json!({ "textDocument": { "uri": uri_of(&item.path) }, "position": { "line": item.target.name_line, "character": item.target.name_character }, "context": { "includeDeclaration": false } }));
        let Ok(server) = self.server() else { return results };
        let mut references = server.request_many(pending.iter().map(request).collect());
        // A server busy with a change answers "content modified" or cancels: asked again once it settles. An
        // error is never read as "no references".
        for _ in 0..3 {
            let failed: Vec<usize> = (0..references.len()).filter(|&index| references[index].is_err()).collect();
            if failed.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
            let Ok(server) = self.server() else { break };
            let retried = server.request_many(failed.iter().map(|&index| request(&pending[index])).collect());
            for (index, answer) in failed.into_iter().zip(retried) {
                references[index] = answer;
            }
        }
        let program = self.config.command.first().cloned().unwrap_or_default();
        let mut answered = Vec::new();
        for (item, answer) in std::mem::take(&mut pending).into_iter().zip(references) {
            match answer {
                Ok(answer) => answered.push((item, answer)),
                Err(error) => results[item.index] = Err(format!("{program} could not list the references of {} ({error}); ask again", symbols[item.index])),
            }
        }
        let (mut pending, references): (Vec<Pending>, Vec<Value>) = answered.into_iter().unzip();
        crate::resources::profile_phase(&format!("lsp references: {}", pending.len()), started);

        let started = Instant::now();
        let mut lines = SourceLines::default();
        for (item, answer) in pending.iter_mut().zip(references) {
            for location in answer.as_array().into_iter().flatten() {
                let file = path_of(location.get("uri").and_then(Value::as_str).unwrap_or(""));
                if self.relative(&file).is_none() {
                    continue;
                }
                let line = location.pointer("/range/start/line").and_then(Value::as_u64).unwrap_or(0) as u32;
                let name_start = location.pointer("/range/start/character").and_then(Value::as_u64).unwrap_or(0) as usize;
                let name_end = location.pointer("/range/end/character").and_then(Value::as_u64).unwrap_or(0) as usize;
                item.covered.insert((file.clone(), line));
                let text = lines.line(&file, line);
                if is_definition(&text, name_start.min(text.len())) {
                    continue; // an implementation of a trait method, not a use of it
                }
                let after = text.get(name_end.min(text.len())..).unwrap_or("").trim_start();
                // A `use ... as other` makes `other` a second name for the target; its uses are looked up too.
                if let Some(alias) = after.strip_prefix("as ") {
                    let alias: String = alias.trim_start().chars().take_while(|character| character.is_alphanumeric() || *character == '_').collect();
                    if !alias.is_empty() && !item.names.contains(&alias) {
                        item.names.push(alias);
                    }
                    item.alias_lines.insert((file.clone(), line));
                }
                let statement = text.trim_start();
                if statement.starts_with("use ") || statement.starts_with("pub use ") || statement.starts_with("pub(crate) use ") {
                    continue; // an import names the target; the calls are the uses of that name
                }
                let kind = if calls(after) { "call" } else { "reference" };
                item.found.push((file, line, kind, "exact"));
            }

            // Spelled like the target (or an alias) but not among its references. In a file changed since the
            // server started, and for an alias, every mention is asked about, not only calls.
            let utf8 = self.server.as_ref().is_some_and(|server| server.utf8_positions);
            // A crate that does not depend on the target's crate cannot call it: its same-named calls are skipped.
            let users = self.crates.as_ref().and_then(|graph| graph.users_of(&item.path));
            for (alias_index, spelled) in item.names.iter().enumerate() {
                for mention in &self.mentions.of(spelled) {
                    if let (Some(graph), Some(users)) = (&self.crates, &users) {
                        if !graph.may_use(&mention.file, users) {
                            continue;
                        }
                    }
                    let every_mention = alias_index > 0 || self.opened.contains_key(&mention.file) || item.target.key.contains('@');
                    let declared_here = mention.file == item.path && mention.line == item.target.name_line;
                    if (mention.call_like || every_mention) && !mention.definition && !declared_here && !item.covered.contains(&(mention.file.clone(), mention.line)) {
                        let column = if utf8 { mention.byte_column } else { mention.utf16_column };
                        item.candidates.push((mention.file.clone(), mention.line, column, mention.call_like, alias_index > 0));
                    }
                }
            }
        }
        crate::resources::profile_phase("lsp reference edges", started);

        // What the server's own highlighting says each candidate is (one request per file, kept until the file
        // changes): unresolved, resolved to something else, or a call through a trait the target implements.
        let started = Instant::now();
        let mut highlight: Vec<PathBuf> = pending
            .iter()
            .flat_map(|item| item.candidates.iter().filter(|candidate| !candidate.4 && !self.opened.contains_key(&candidate.0)).map(|candidate| candidate.0.clone()))
            .filter(|file| !self.tokens.contains_key(file))
            .collect();
        highlight.sort();
        highlight.dedup();
        // Highlighting a file costs the server its whole analysis: worth it for many candidates in few files. Spread
        // over many files (a common method name across a workspace: 386 files, 68 s), asking each candidate's
        // definition is cheaper and says exactly what it calls.
        let candidate_count: usize = pending.iter().map(|item| item.candidates.len()).sum();
        let by_definition = highlight.len() > HIGHLIGHT_FILE_LIMIT && candidate_count <= DEFINITION_LIMIT;
        if !by_definition {
            self.fetch_tokens(&highlight);
        }
        crate::resources::profile_phase(&format!("lsp tokens: {} files", if by_definition { 0 } else { highlight.len() }), started);

        let started = Instant::now();
        let mut definition_requests = Vec::new();
        let mut asked_by = Vec::new();
        for (item_index, item) in pending.iter_mut().enumerate() {
            for (file, line, column, call_like, through_alias) in std::mem::take(&mut item.candidates) {
                // Which definition an alias use, or a use in a changed file, reaches only the definition tells.
                let other_crate = self.relative(&file).is_some_and(|relative| outside_library(&relative));
                let macro_item = item.target.key.contains('@');
                let ask = by_definition || through_alias || other_crate || macro_item || self.opened.contains_key(&file) || match self.token_at(&file, line, column) {
                    Some((kind, _)) if kind == "unresolvedReference" => {
                        item.judged.push((file.clone(), line, call_like, Vec::new()));
                        false
                    }
                    Some((kind, modifiers)) => item.target.implements.is_some() && kind == "method" && modifiers.iter().any(|modifier| modifier == "trait"),
                    // No highlighting for it (the server could not answer): its definition says.
                    None => true,
                };
                if ask {
                    definition_requests.push(("textDocument/definition", json!({ "textDocument": { "uri": uri_of(&file) }, "position": { "line": line, "character": column } })));
                    asked_by.push((item_index, file, line, call_like));
                }
            }
        }
        let definition_count = definition_requests.len();
        let mut answers = match self.server() {
            Ok(server) if definition_count > 0 => server.request_many(definition_requests.clone()),
            _ => Vec::new(),
        };
        // As for references: an error is asked again once the server settles, never read as "resolves to nothing".
        for _ in 0..3 {
            let failed: Vec<usize> = (0..answers.len()).filter(|&index| answers[index].is_err()).collect();
            if failed.is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(200));
            let Ok(server) = self.server() else { break };
            let retried = server.request_many(failed.iter().map(|&index| definition_requests[index].clone()).collect());
            for (index, answer) in failed.into_iter().zip(retried) {
                answers[index] = answer;
            }
        }
        for ((item_index, file, line, call_like), answer) in asked_by.into_iter().zip(answers) {
            let definitions = match answer {
                Ok(Value::Array(items)) => items,
                Ok(Value::Object(item)) => vec![Value::Object(item)],
                _ => Vec::new(),
            };
            pending[item_index].judged.push((file, line, call_like, definitions));
        }
        crate::resources::profile_phase(&format!("lsp definitions: {definition_count}"), started);

        for mut item in pending {
            for (file, line, call_like, definitions) in std::mem::take(&mut item.judged) {
                if let Some(confidence) = self.judge(&item, &file, line, call_like, &definitions, &mut lines) {
                    if item.covered.insert((file.clone(), line)) {
                        item.found.push((file, line, if call_like { "call" } else { "reference" }, confidence));
                    }
                }
            }
            if let Some(implicit) = item.target.implements.as_deref().and_then(implicit_use) {
                self.implicit_uses(&mut item, implicit);
            }
            let edges: Vec<Value> = std::mem::take(&mut item.found)
                .into_iter()
                .map(|(file, line, kind, confidence)| {
                    let relative = self.relative(&file).unwrap_or_default();
                    json!({ "from": self.symbol_at(&file, line), "at": { "file": relative, "line": line + 1 }, "kind": kind, "confidence": confidence })
                })
                .collect();
            self.answered.insert(symbols[item.index].clone(), (item.path.clone(), item.names.clone()));
            self.answers.insert(symbols[item.index].clone(), Value::Array(edges.clone()));
            results[item.index] = Ok(Value::Array(edges));
        }
        results
    }

    /// How sure a candidate is a use of the target, from the definitions the server gave for it; None when it
    /// is something else.
    fn judge(&mut self, item: &Pending, file: &Path, line: u32, call_like: bool, definitions: &[Value], lines: &mut SourceLines) -> Option<&'static str> {
        let mut points_at_target = false;
        let mut points_at_trait = false;
        for definition in definitions {
            let uri = definition.get("uri").or_else(|| definition.get("targetUri")).and_then(Value::as_str).unwrap_or("");
            let range = definition.get("targetSelectionRange").or_else(|| definition.get("range"));
            let definition_line = range.and_then(|range| range.pointer("/start/line")).and_then(Value::as_u64).unwrap_or(0) as u32;
            let definition_path = path_of(uri);
            points_at_target |= (definition_path == item.path && definition_line == item.target.name_line) || item.alias_lines.contains(&(definition_path.clone(), definition_line));
            if let Some(trait_method) = &item.target.implements {
                let key = self.symbol_at(&definition_path, definition_line).get("symbol").and_then(Value::as_str).unwrap_or("").to_owned();
                points_at_trait |= key.ends_with(&format!("#{trait_method}")) || key.ends_with(&format!("::{trait_method}"));
            }
        }
        if points_at_target {
            return Some("exact");
        }
        if points_at_trait {
            return Some("likely");
        }
        if !definitions.is_empty() || !call_like {
            return None; // something else, or a mention that is no call and resolves to nothing (comment, field)
        }
        // Inside a `macro_rules!` body nothing is resolved until expansion; a crate path spelled out there still
        // names one module: the target's (then the macro calls it) or another (then it does not).
        let name = &item.names[0];
        let text = lines.line(file, line);
        let before = text[..text.find(&format!("{name}(")).or_else(|| text.find(&format!("{name}::<"))).unwrap_or(0)].trim_end();
        let spelled: String = before.chars().rev().take_while(|character| character.is_alphanumeric() || *character == '_' || *character == ':' || *character == '$').collect::<String>().chars().rev().collect();
        let owner = item.target.key.rsplit_once("::").map(|(owner, _)| owner.rsplit("::").next().unwrap_or(owner));
        let named_type = spelled.trim_end_matches("::").rsplit("::").next().unwrap_or("");
        if named_type.starts_with(|character: char| character.is_uppercase()) && Some(named_type) != owner && named_type != "Self" {
            return None; // `Command::new(...)` calls `new` of `Command`
        }
        let Some(path_text) = spelled.strip_prefix("$crate::").or_else(|| spelled.strip_prefix("crate::")) else { return Some("unresolved") };
        let target_module = module_path(&self.relative(&item.path).unwrap_or_default());
        // A free function is reached through its module, a method through its type.
        let expected = match item.target.key.rsplit_once("::") {
            Some((owner, _)) if target_module.is_empty() => owner.to_owned(),
            Some((owner, _)) => format!("{target_module}::{owner}"),
            None => target_module,
        };
        (path_text.trim_end_matches("::") == expected).then_some("likely")
    }

    /// Trait methods the language calls without their name (`?` calls `From::from`, `{}` calls `Display::fmt`):
    /// which implementation runs depends on types the source does not spell. In a function that names the
    /// target's type (in its signature for a conversion, anywhere in it otherwise), each such line is listed.
    fn implicit_uses(&mut self, item: &mut Pending, implicit: &ImplicitUse) {
        let Some(owner) = item.target.key.rsplit_once("::").map(|(owner, _)| owner.rsplit("::").next().unwrap_or(owner).to_owned()) else { return };
        let mut files: Vec<PathBuf> = self.mentions.files_by_name.get(&owner).into_iter().flatten().map(|file| file.to_path_buf()).collect();
        files.sort();
        for file in files {
            let lines = self.mentions.lines_by_file.get(&file).cloned().unwrap_or_default();
            let converting: Vec<u32> = (0..lines.len() as u32).filter(|&line| (implicit.uses)(&lines[line as usize])).collect();
            if converting.is_empty() {
                continue;
            }
            let functions: Vec<FileSymbol> = self.file_symbols(&file).into_iter().filter(|symbol| CALLABLE_KINDS.contains(&symbol.kind)).collect();
            for line in converting {
                let Some(function) = functions.iter().filter(|symbol| symbol.start <= line && line <= symbol.end).max_by_key(|symbol| symbol.start) else { continue };
                let signature: String = lines[function.name_line as usize..].iter().take_while(|text| !text.contains('{')).chain(lines.get(function.name_line as usize..).and_then(|rest| rest.iter().find(|text| text.contains('{')))).cloned().collect::<Vec<_>>().join(" ");
                let body = lines[function.start as usize..=(function.end as usize).min(lines.len() - 1)].join(" ");
                // With the server's types at hand the operand decides; without, the function must name the type.
                let named = implicit.operands.is_some() || names_word(if implicit.signature_only { &signature } else { &body }, &owner);
                if !named || item.covered.contains(&(file.clone(), line)) {
                    continue;
                }
                let confidence = match implicit.operands {
                    Some(operands) => self.operand_types(&file, line, &lines[line as usize], operands(&lines[line as usize]), &owner),
                    None => Some("unresolved"),
                };
                if let Some(confidence) = confidence {
                    item.covered.insert((file.clone(), line));
                    item.found.push((file.clone(), line, implicit.label, confidence));
                }
            }
        }
    }

    /// What the server says the operands' types are: `likely` when one names the target's type, None when every
    /// one names another type, `unresolved` when the server cannot tell.
    fn operand_types(&mut self, file: &Path, line: u32, text: &str, columns: Vec<usize>, owner: &str) -> Option<&'static str> {
        let utf8 = self.server.as_ref().is_some_and(|server| server.utf8_positions);
        let requests = columns
            .iter()
            .map(|&column| {
                let character = if utf8 { column } else { text[..column].encode_utf16().count() };
                ("textDocument/hover", json!({ "textDocument": { "uri": uri_of(file) }, "position": { "line": line, "character": character } }))
            })
            .collect();
        let Ok(server) = self.server() else { return Some("unresolved") };
        let hovers: Vec<String> = server.request_many(requests).into_iter().filter_map(Result::ok).map(|hover| hover.pointer("/contents/value").and_then(Value::as_str).unwrap_or("").to_owned()).filter(|hover| !hover.is_empty()).collect();
        if hovers.iter().any(|hover| names_word(hover, owner)) {
            Some("likely")
        } else if hovers.len() == columns.len() {
            None
        } else {
            Some("unresolved")
        }
    }

    fn fetch_tokens(&mut self, files: &[PathBuf]) {
        if files.is_empty() {
            return;
        }
        let requests = files.iter().map(|file| ("textDocument/semanticTokens/full", json!({ "textDocument": { "uri": uri_of(file) } }))).collect();
        let answers = match self.server() {
            Ok(server) => server.request_many(requests),
            Err(_) => return,
        };
        for (file, answer) in files.iter().zip(answers) {
            // Not kept when the server failed: asked again when needed instead of read as "no tokens".
            let Ok(answer) = answer else { continue };
            let numbers: Vec<u32> = answer.get("data").cloned().and_then(|data| serde_json::from_value(data).ok()).unwrap_or_default();
            let mut decoded = Vec::new();
            let (mut current_line, mut current_start) = (0u32, 0u32);
            for token in numbers.chunks_exact(5) {
                current_start = if token[0] == 0 { current_start + token[1] } else { token[1] };
                current_line += token[0];
                decoded.push((current_line, current_start, token[2], token[3], token[4]));
            }
            self.tokens.insert(file.clone(), decoded);
        }
    }

    /// The server's highlighting of the token at a position: its type and modifiers.
    fn token_at(&mut self, file: &Path, line: u32, column: u32) -> Option<(String, Vec<String>)> {
        if !self.tokens.contains_key(file) {
            self.fetch_tokens(&[file.to_path_buf()]);
        }
        let server = self.server.as_ref()?;
        let &(_, _, _, kind, modifiers) = self.tokens.get(file)?.iter().find(|(token_line, start, length, _, _)| *token_line == line && *start <= column && column < start + length)?;
        let modifiers = server.token_modifiers.iter().enumerate().filter(|(bit, _)| modifiers >> bit & 1 == 1).map(|(_, name)| name.clone()).collect();
        Some((server.token_types.get(kind as usize).cloned().unwrap_or_default(), modifiers))
    }

    fn source_files(&self) -> Vec<PathBuf> {
        ignore::WalkBuilder::new(&self.repo_root)
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
            .map(|entry| entry.into_path())
            .filter(|path| self.handles(path))
            .collect()
    }

    /// Every change goes in as document text, in order: a changed or added file with its content, a removed
    /// one as empty text (for a call graph, an empty file and a missing one are the same). rust-analyzer applies
    /// document edits before the next request, while watched-file events are processed in the background and
    /// gave partial answers in between. Only a project file (`Cargo.toml`) is sent as an event: the server then
    /// reloads the workspace, reports it, and the next answer waits for that.
    fn sync(&mut self, changed: Vec<String>) -> Value {
        let mut project_events = Vec::new();
        let mut documents = Vec::new();
        let mut edits: Vec<FileEdit> = Vec::new();
        let searched: HashSet<String> = self.answered.values().flat_map(|(_, names)| names.iter().cloned()).collect();
        for relative in &changed {
            let path = self.repo_root.join(relative);
            if self.config.project_files.iter().any(|name| relative == name || relative.ends_with(&format!("/{name}"))) {
                project_events.push(json!({ "uri": uri_of(&path), "type": if path.is_file() { 2 } else { 3 } }));
                self.read_crates();
                continue;
            }
            if !self.handles(&path) {
                continue;
            }
            let old_symbols = self.symbols.remove(&path);
            self.tokens.remove(&path);
            let old_lines = self.mentions.lines_by_file.get(&path).cloned().unwrap_or_default();
            let mentioned = |mentions: &Mentions| -> HashMap<String, Vec<(u32, bool)>> {
                mentions.names_by_file.get(&path).into_iter().flatten().filter(|name| searched.contains(*name)).map(|name| (name.clone(), mentions.in_file(name, &path))).collect()
            };
            let old_mentions = mentioned(&self.mentions);
            self.mentions.read(&path);
            let new_mentions = mentioned(&self.mentions);
            let new_lines = self.mentions.lines_by_file.get(&path).cloned().unwrap_or_default();
            edits.push(FileEdit::new(path.clone(), &old_lines, &new_lines, old_symbols, old_mentions, new_mentions));
            let text = fs::read_to_string(&path).unwrap_or_default();
            let version = self.opened.get(&path).map_or(1, |version| version + 1);
            self.opened.insert(path.clone(), version);
            documents.push((path, version, text));
        }
        if project_events.is_empty() && documents.is_empty() {
            return json!({ "invalidated": [] });
        }
        let language_id = self.config.language_id.clone();
        if let Some(server) = self.server.as_mut() {
            if server.reports_status && !documents.is_empty() {
                server.changed_at = Some(Instant::now());
                server.status_since_change = None;
            }
            for (path, version, text) in documents {
                if version == 1 {
                    server.notify("textDocument/didOpen", json!({ "textDocument": { "uri": uri_of(&path), "languageId": language_id, "version": version, "text": text } }));
                } else {
                    server.notify("textDocument/didChange", json!({ "textDocument": { "uri": uri_of(&path), "version": version }, "contentChanges": [{ "text": text }] }));
                }
            }
            if !project_events.is_empty() {
                if server.reports_status {
                    server.quiescent = false;
                }
                server.notify("workspace/didChangeWatchedFiles", json!({ "changes": project_events }));
            }
        }

        // Which answers a change can alter: those that searched a name the file mentions (before or after), that
        // live in the file, or that list a place in it. Most edits move lines and touch one function's body:
        // an answer whose mentions in the file all lie outside that body, and are the same mentions as before,
        // is kept with its lines moved; any other is asked again. A project file can change everything.
        let mut invalidated = Vec::new();
        let symbols: Vec<String> = self.answered.keys().cloned().collect();
        for symbol in symbols {
            let (target, names) = self.answered[&symbol].clone();
            let relevant: Vec<&FileEdit> = edits.iter().filter(|edit| edit.path == target || edit.mentions_any(&names) || self.answers.get(&symbol).is_some_and(|answer| self.lists_file(answer, &edit.path))).collect();
            if relevant.is_empty() && project_events.is_empty() {
                continue;
            }
            invalidated.push(symbol.clone());
            let keeps = project_events.is_empty() && relevant.iter().all(|edit| edit.keeps(&names));
            let moved = if keeps { self.answers.get(&symbol).cloned().and_then(|answer| self.moved(answer, &relevant)) } else { None };
            match moved {
                Some(answer) => {
                    self.answers.insert(symbol, answer);
                }
                None => {
                    self.answers.remove(&symbol);
                    self.answered.remove(&symbol);
                    self.stale.push(symbol);
                }
            }
        }
        json!({ "invalidated": invalidated, "watch": self.config.project_files })
    }

    fn lists_file(&self, answer: &Value, file: &Path) -> bool {
        let relative = self.relative(file).unwrap_or_default();
        answer.as_array().into_iter().flatten().any(|edge| edge.pointer("/at/file").and_then(Value::as_str) == Some(relative.as_str()) || edge.pointer("/from/file").and_then(Value::as_str) == Some(relative.as_str()))
    }

    /// The answer with its lines in the edited files moved; None when a listed line was inside an edit.
    fn moved(&self, mut answer: Value, edits: &[&FileEdit]) -> Option<Value> {
        for edit in edits {
            let relative = self.relative(&edit.path).unwrap_or_default();
            for edge in answer.as_array_mut().into_iter().flatten() {
                for pointer in ["/at", "/from"] {
                    let Some(place) = edge.pointer_mut(pointer) else { continue };
                    if place.get("file").and_then(Value::as_str) != Some(relative.as_str()) {
                        continue;
                    }
                    let line = place.get("line").and_then(Value::as_u64)? as u32;
                    place["line"] = json!(edit.map_old(line - 1)? + 1);
                }
            }
        }
        Some(answer)
    }

    fn probe(&mut self) -> Value {
        let program = self.config.command.first().cloned().unwrap_or_default();
        let version = Command::new(&program).arg("--version").output().ok().map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned());
        json!({ "driver": format!("lsp:{}", self.config.language_id), "server": program, "version": version, "running": self.server.is_some(), "failure": self.failure })
    }
}

/// Above this many files holding candidates, each candidate's definition is asked instead of highlighting files,
/// unless there are more candidates than `DEFINITION_LIMIT`.
const HIGHLIGHT_FILE_LIMIT: usize = 40;
const DEFINITION_LIMIT: usize = 3000;

/// Source files up to which every file is analysed ahead of questions.
const ANALYSE_AHEAD_LIMIT: usize = 1500;

/// Stale answers worked out again per idle step: small enough that a question arriving meanwhile barely waits.
const STALE_PER_STEP: usize = 1;

pub fn serve(repo_root: &Path) {
    let manifest: Result<Manifest, String> = fs::read_to_string("driver.json").map_err(|error| error.to_string()).and_then(|text| serde_json::from_str(&text).map_err(|error| error.to_string()));
    let manifest = match manifest {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("driver.json: {error}");
            return;
        }
    };
    watch_parent();
    let mut driver = LspDriver { repo_root: repo_root.to_path_buf(), config: manifest.lsp, server: None, failure: None, opened: HashMap::new(), symbols: HashMap::new(), answered: HashMap::new(), mentions: Mentions::default(), tokens: HashMap::new(), primed: false, answers: HashMap::new(), stale: Vec::new(), warm_up: None, crates: None };
    driver.read_crates();
    let files = driver.source_files();
    for file in &files {
        driver.mentions.read(file);
    }
    // Analysing every file ahead (highlighting all, then the callers of every function) makes the server hold
    // the whole workspace in memory: past this size (codex: 5,300 files, 11 GB) only what is asked is analysed.
    if files.len() > ANALYSE_AHEAD_LIMIT {
        driver.primed = true;
        driver.warm_up = Some(Vec::new());
    }
    // The core sends a level of `callers` questions at once; reading ahead lets them be answered together.
    let (lines, incoming) = channel::<String>();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if lines.send(line).is_err() {
                return;
            }
        }
    });
    let mut stdout = std::io::stdout();
    let mut waiting: std::collections::VecDeque<Value> = std::collections::VecDeque::new();
    loop {
        if waiting.is_empty() {
            // Idle: answer ahead for the next symbols of the warm-up, a few at a time, so a question that arrives
            // waits for one small batch at most.
            let idle_work = !driver.stale.is_empty() || driver.warm_up_pending();
            match incoming.recv_timeout(if idle_work { Duration::ZERO } else { Duration::from_secs(3600) }) {
                Ok(line) => waiting.extend(serde_json::from_str::<Value>(&line).ok()),
                Err(RecvTimeoutError::Timeout) => {
                    // Answers a change made stale come first, a few at a time: the file watcher reports a save
                    // before the next question comes, which then finds them ready or waits for one small batch.
                    if driver.stale.is_empty() {
                        driver.warm_up_step();
                    } else {
                        let stale: Vec<String> = driver.stale.drain(..driver.stale.len().min(STALE_PER_STEP)).filter(|symbol| !driver.answers.contains_key(symbol)).collect();
                        driver.callers_batch(&stale);
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        while let Ok(line) = incoming.try_recv() {
            waiting.extend(serde_json::from_str::<Value>(&line).ok());
        }
        let is_callers = |request: &Value| request.get("op").and_then(Value::as_str) == Some("callers");
        if waiting.front().is_some_and(is_callers) {
            let batch: Vec<Value> = std::iter::from_fn(|| waiting.front().is_some_and(is_callers).then(|| waiting.pop_front()).flatten()).collect();
            let symbols: Vec<String> = batch.iter().map(|request| request.get("symbol").and_then(Value::as_str).unwrap_or("").to_owned()).collect();
            for (request, result) in batch.iter().zip(driver.callers_batch(&symbols)) {
                let reply = match result {
                    Ok(result) => json!({ "id": request.get("id"), "result": result }),
                    Err(error) => json!({ "id": request.get("id"), "error": error }),
                };
                let _ = writeln!(stdout, "{reply}");
            }
            let _ = stdout.flush();
            continue;
        }
        let Some(request) = waiting.pop_front() else { continue };
        let text = |field: &str| request.get(field).and_then(Value::as_str).unwrap_or("").to_owned();
        let result = match text("op").as_str() {
            "probe" => driver.probe(),
            "sync" => {
                let changed = request.get("changed").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
                let mut answer = driver.sync(changed);
                if let Some(failure) = &driver.failure {
                    answer["warnings"] = json!([format!("{} could not start ({failure}); callers in its files are unknown", driver.config.language_id)]);
                } else if let Some(trouble) = driver.server.as_ref().and_then(|server| server.trouble.as_ref()) {
                    answer["warnings"] = json!([format!("{} could not load the project as configured ({trouble}); callers found by name may be marked unresolved", driver.config.command.first().cloned().unwrap_or_default())]);
                }
                answer
            }
            "resolve" => driver.resolve(&text("query")),
            "locate" => driver.locate(&text("file"), request.get("line").and_then(Value::as_u64).unwrap_or(1) as u32),
            _ => Value::Null,
        };
        let _ = writeln!(stdout, "{}", json!({ "id": request.get("id"), "result": result }));
        let _ = stdout.flush();
    }
}
