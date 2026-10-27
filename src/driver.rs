use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::resources::{describe_tree, kill_tree, profile_phase, Progress};

/// A repository matches when it has `file` (containing `contains`, when given), any file ending in one of
/// `extensions`, or matches one of the `any` rules.
#[derive(Deserialize, Clone)]
struct DetectRule {
    #[serde(default)]
    file: Option<String>,
    contains: Option<String>,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    any: Vec<DetectRule>,
}

#[derive(Deserialize, Clone)]
struct DriverManifest {
    detect: Vec<DetectRule>,
    command: Vec<String>,
    /// Operations the driver answers; a driver is never asked (nor waited for) about any other.
    ops: Vec<String>,
    /// File extensions this driver understands; the generic driver reads every other text file.
    #[serde(default)]
    extensions: Vec<String>,
    /// Operations whose answers the core keeps until the driver's `sync` answer lists the request's
    /// `symbol` under `invalidated`; such a driver tracks what each answer read.
    #[serde(default)]
    cache: Vec<String>,
    /// Files the repository must have for this driver to work (`vendor/autoload.php`). When one is missing, its
    /// languages go to the generic driver (calls by name), and every answer says why.
    #[serde(default)]
    requires: Vec<Requirement>,
}

#[derive(Deserialize, Clone)]
struct Requirement {
    file: String,
    /// What to do about it, said on every answer.
    missing: String,
}

#[derive(Deserialize)]
struct DriverResponse {
    id: u64,
    #[serde(default)]
    result: Value,
    error: Option<String>,
}

/// One long-lived driver process speaking JSON lines: `{id, op, ...}` in, `{id, result | error}` out.
/// A driver may exit on purpose (for example when code it loaded went stale); the pool restarts it.
struct Driver {
    name: String,
    directory: PathBuf,
    manifest: DriverManifest,
    repo_root: PathBuf,
    child: Child,
    stdin: Option<BufWriter<ChildStdin>>,
    responses: Receiver<DriverResponse>,
    stderr_tail: Arc<Mutex<String>>,
    next_request_id: u64,
    exited: bool,
    claimed_extensions: String,
    /// Answers that arrived while waiting for other requests (a deferred request answered first).
    parked: HashMap<u64, Result<Value, String>>,
    /// Bumped on every restart, so answers owed by a process that died are not waited for.
    generation: u64,
    /// Kept answers of `manifest.cache` operations, by request; a restarted process starts without any.
    answers: HashMap<String, (Option<String>, Arc<Value>)>,
    /// A `sync` whose invalidations must be applied before a kept answer is used again.
    pending_sync: Option<u64>,
    /// Requests not answered yet, as sent: named in the log when the driver hangs on one.
    unanswered: HashMap<u64, String>,
}

fn append_tail(tail: &Mutex<String>, text: &str) {
    let mut tail = tail.lock().unwrap();
    tail.push_str(text);
    if tail.len() > 2000 {
        let cut = tail.len() - 2000;
        let boundary = (cut..tail.len()).find(|&index| tail.is_char_boundary(index)).unwrap_or(tail.len());
        tail.drain(..boundary);
    }
}

/// The other checkouts of this repository (git worktrees sharing its history), one path per line: a driver may start
/// from the analysis one of them saved instead of reading every file again.
fn sibling_checkouts(repo_root: &Path) -> String {
    let Ok(output) = Command::new("git").arg("-C").arg(repo_root).args(["worktree", "list", "--porcelain"]).output() else { return String::new() };
    let own = repo_root.canonicalize().unwrap_or_else(|_| repo_root.to_path_buf());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .filter(|path| Path::new(path).canonicalize().is_ok_and(|path| path != own))
        .collect::<Vec<_>>()
        .join("\n")
}

impl Driver {
    fn spawn(name: String, directory: PathBuf, manifest: DriverManifest, repo_root: PathBuf, claimed_extensions: String) -> Result<Self, String> {
        let (program, args) = manifest.command.split_first().ok_or(format!("{name}: driver.json has no command"))?;
        // `{scout}` is this very binary, so a driver built into it needs no shell script (Windows included).
        let program = if program == "{scout}" { crate::daemon::own_binary().map_err(|error| error.to_string())?.to_string_lossy().into_owned() } else { program.clone() };
        let mut child = Command::new(&program)
            .args(args)
            .arg(&repo_root)
            .current_dir(&directory)
            .env("SCOUT_CLAIMED_EXTENSIONS", &claimed_extensions)
            .env("SCOUT_CACHE", crate::daemon::cache_directory())
            .envs(crate::slots::driver_environment())
            .env("SCOUT_SEED_REPOS", sibling_checkouts(&repo_root))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("{name}: cannot start {program}: {error}"))?;

        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let (sender, responses) = channel();
        let stdout = child.stdout.take().unwrap();
        let stdout_tail = Arc::clone(&stderr_tail);
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                match serde_json::from_str::<DriverResponse>(&line) {
                    Ok(response) => {
                        if sender.send(response).is_err() {
                            break;
                        }
                    }
                    Err(_) => append_tail(&stdout_tail, &format!("\n[stdout] {line}")),
                }
            }
        });
        let mut stderr = child.stderr.take().unwrap();
        let stderr_reader_tail = Arc::clone(&stderr_tail);
        thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                append_tail(&stderr_reader_tail, &String::from_utf8_lossy(&buffer[..read]));
            }
        });

        Ok(Self {
            claimed_extensions,
            name,
            directory,
            manifest,
            repo_root,
            stdin: child.stdin.take().map(BufWriter::new),
            child,
            responses,
            stderr_tail,
            next_request_id: 1,
            exited: false,
            parked: HashMap::new(),
            generation: 0,
            answers: HashMap::new(),
            pending_sync: None,
            unanswered: HashMap::new(),
        })
    }

    fn restarted(&self) -> Result<Self, String> {
        let mut fresh = Self::spawn(self.name.clone(), self.directory.clone(), self.manifest.clone(), self.repo_root.clone(), self.claimed_extensions.clone())?;
        fresh.generation = self.generation + 1;
        Ok(fresh)
    }

    fn supports(&self, op: &str) -> bool {
        self.manifest.ops.iter().any(|supported| supported == op)
    }

    fn keeps(&self, op: &str) -> bool {
        self.manifest.cache.iter().any(|kept| kept == op)
    }

    /// Drops the kept answers a `sync` answer lists, and removes that list so it is applied once.
    fn apply_invalidation(&mut self, answer: &mut Value) {
        let Some(invalidated) = answer.as_object_mut().and_then(|answer| answer.remove("invalidated")) else { return };
        let symbols: BTreeSet<&str> = invalidated.as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        self.answers.retain(|_, (symbol, _)| symbol.as_deref().is_none_or(|symbol| !symbols.contains(symbol)));
    }

    /// Reads the latest `sync` answer now (it is parked again for whoever collects it), so no kept answer
    /// outlives a change.
    fn settle_sync(&mut self) {
        let Some(id) = self.pending_sync.take() else { return };
        let mut answer = self.receive(&[id]).remove(&id).unwrap_or_else(|| Err("no answer".to_owned()));
        match &mut answer {
            Ok(value) => self.apply_invalidation(value),
            Err(_) => self.answers.clear(),
        }
        self.parked.insert(id, answer);
    }

    fn send(&mut self, op: &str, args: &Value) -> Result<u64, String> {
        let id = self.next_request_id;
        self.next_request_id += 1;
        let mut request = json!({ "id": id, "op": op });
        if let (Some(request), Some(args)) = (request.as_object_mut(), args.as_object()) {
            request.extend(args.clone());
        }
        let written = match self.stdin.as_mut() {
            Some(stdin) => writeln!(stdin, "{request}").map_err(|error| error.to_string()),
            None => Err("stdin closed".to_owned()),
        };
        written.map_err(|reason| self.failure(&reason))?;
        self.unanswered.insert(id, request.to_string().chars().take(300).collect());
        Ok(id)
    }

    /// Requests are buffered; this hands them to the driver. Called before waiting for any answer.
    fn flush(&mut self) {
        if let Some(Err(error)) = self.stdin.as_mut().map(Write::flush) {
            self.exited = true;
            append_tail(&self.stderr_tail, &format!("\n[stdin] {error}"));
        }
    }

    /// Reads until every id in `ids` has an answer; the reader thread keeps draining stdout so the driver never blocks.
    fn receive(&mut self, ids: &[u64]) -> HashMap<u64, Result<Value, String>> {
        self.flush();
        let mut answers: HashMap<u64, Result<Value, String>> = ids.iter().filter_map(|id| self.parked.remove(id).map(|answer| (*id, answer))).collect();
        let mut progress = Progress::new(self.child.id());
        let mut ending = "process exited".to_owned();
        while answers.len() < ids.len() {
            // A driver that died can leave its output pipe open in a process it started (a forked worker), so
            // the end of output alone does not show it: its process is checked while waiting. One that lives but
            // stopped working (it lost the question, or waits for something that never comes) is ended.
            let received = match self.responses.recv_timeout(Duration::from_millis(500)) {
                Err(RecvTimeoutError::Timeout) if matches!(self.child.try_wait(), Ok(None)) => {
                    if !progress.stalled() {
                        continue;
                    }
                    ending = format!("no answer and no work for {} s", progress.idle_seconds());
                    self.end_stuck(ids, &answers, &ending);
                    Err(())
                }
                Err(RecvTimeoutError::Timeout) => Err(()),
                other => other.map_err(|_| ()),
            };
            match received {
                Ok(response) => {
                    self.unanswered.remove(&response.id);
                    let answer = match response.error {
                        Some(error) => Err(format!("{}: {error}", self.name)),
                        None => Ok(response.result),
                    };
                    if ids.contains(&response.id) {
                        answers.insert(response.id, answer);
                    } else {
                        self.parked.insert(response.id, answer);
                    }
                }
                Err(_) => {
                    self.exited = true;
                    let failure = self.failure(&ending);
                    for id in ids {
                        answers.entry(*id).or_insert_with(|| Err(failure.clone()));
                    }
                }
            }
        }
        answers
    }

    /// Ends a driver that waits with questions unanswered and does no work; the log names what it was asked.
    fn end_stuck(&mut self, ids: &[u64], answers: &HashMap<u64, Result<Value, String>>, reason: &str) {
        let waiting: Vec<&str> = ids.iter().filter(|id| !answers.contains_key(id)).filter_map(|id| self.unanswered.get(id).map(String::as_str)).collect();
        eprintln!("{}: {reason}; restarting it. Waiting for: {}\n{}", self.name, waiting.join(" | "), describe_tree(self.child.id()));
        kill_tree(self.child.id());
        let _ = self.child.kill();
    }

    fn send_all(&mut self, op: &str, requests: &[Value]) -> Vec<Result<u64, String>> {
        requests.iter().map(|args| self.send(op, args)).collect()
    }

    fn collect(&mut self, ids: Vec<Result<u64, String>>) -> Vec<Result<Value, String>> {
        let pending: Vec<u64> = ids.iter().filter_map(|id| id.as_ref().ok().copied()).collect();
        let mut received = self.receive(&pending);
        ids.into_iter()
            .map(|id| match id {
                Ok(id) => received.remove(&id).unwrap_or_else(|| Err(self.failure("no answer"))),
                Err(error) => {
                    self.exited = true;
                    Err(error)
                }
            })
            .collect()
    }

    /// A driver that exited mid-way is restarted once and asked again.
    fn retry_after_exit(&mut self, op: &str, requests: &[Value], answers: Vec<Result<Value, String>>) -> Vec<Result<Value, String>> {
        if !self.exited {
            return answers;
        }
        match self.restarted() {
            Ok(fresh) => {
                *self = fresh;
                let ids = self.send_all(op, requests);
                self.collect(ids)
            }
            Err(error) => vec![Err(error); requests.len()],
        }
    }

    fn failure(&self, reason: &str) -> String {
        let tail = self.stderr_tail.lock().unwrap();
        match tail.trim() {
            "" => format!("{} failed ({reason})", self.name),
            tail => format!("{} failed ({reason}): {tail}", self.name),
        }
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

fn matches_repo(rule: &DetectRule, repo_root: &Path) -> bool {
    if !rule.any.is_empty() {
        return rule.any.iter().any(|rule| matches_repo(rule, repo_root));
    }
    if !rule.extensions.is_empty() {
        return ignore::WalkBuilder::new(repo_root).build().filter_map(Result::ok).any(|entry| entry.path().extension().is_some_and(|extension| rule.extensions.iter().any(|wanted| extension == wanted.as_str())));
    }
    let Some(file) = &rule.file else { return false };
    let path = repo_root.join(file);
    match &rule.contains {
        None => path.exists(),
        Some(needle) => fs::read_to_string(path).is_ok_and(|content| content.contains(needle.as_str())),
    }
}

/// A request sent with `send_deferred`, answered later.
pub struct Deferred {
    sent: Vec<Option<(u64, u64)>>,
}

/// Every driver whose `driver.json` matches the repo; asks them all and keeps the answers of those that succeed.
pub struct DriverPool {
    drivers: Vec<Driver>,
    pub warnings: BTreeSet<String>,
    /// Warnings about test files: shown only when callers inside tests are.
    pub test_warnings: BTreeSet<String>,
    /// A driver's warnings, with the extensions it reads: shown only when the answer names one of its files.
    pub language_warnings: BTreeMap<String, Vec<String>>,
    /// Drivers that could not start, said on every answer.
    pub unavailable: BTreeSet<String>,
}

impl DriverPool {
    pub fn start(drivers_directory: &Path, repo_root: &Path) -> Result<Self, String> {
        let mut entries: Vec<_> = fs::read_dir(drivers_directory)
            .map_err(|error| format!("cannot read {}: {error}", drivers_directory.display()))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join("driver.json").is_file())
            .collect();
        entries.sort();

        let mut matched = Vec::new();
        let mut unavailable = BTreeSet::new();
        for directory in entries {
            let name = directory.file_name().unwrap().to_string_lossy().into_owned();
            let manifest: DriverManifest = serde_json::from_str(&fs::read_to_string(directory.join("driver.json")).map_err(|error| error.to_string())?)
                .map_err(|error| format!("{name}/driver.json: {error}"))?;
            if !manifest.detect.iter().all(|rule| matches_repo(rule, repo_root)) {
                continue;
            }
            match manifest.requires.iter().find(|requirement| !repo_root.join(&requirement.file).exists()) {
                Some(requirement) => {
                    unavailable.insert(format!("{name}: {} is missing ({}); its files are searched by name only", requirement.file, requirement.missing));
                }
                None => matched.push((name, directory, manifest)),
            }
        }
        let claimed = matched.iter().flat_map(|(_, _, manifest)| manifest.extensions.iter().cloned()).collect::<Vec<_>>().join(",");
        let mut drivers = Vec::new();
        // A language whose engine is missing (no `node`, no `php`) is reported on every answer; the others still work.
        for (name, directory, manifest) in matched {
            match Driver::spawn(name, directory, manifest, repo_root.to_path_buf(), claimed.clone()) {
                Ok(driver) => drivers.push(driver),
                Err(error) => {
                    unavailable.insert(format!("{error}; callers in its files are unknown"));
                }
            }
        }
        if drivers.is_empty() {
            return Err(format!("No driver supports repository {}{}.", repo_root.display(), unavailable.iter().map(|reason| format!(" ({reason})")).collect::<String>()));
        }
        Ok(Self { drivers, warnings: BTreeSet::new(), test_warnings: BTreeSet::new(), language_warnings: BTreeMap::new(), unavailable })
    }

    /// Process ids of the driver processes, for resource accounting.
    pub fn process_ids(&self) -> Vec<u32> {
        self.drivers.iter().map(|driver| driver.child.id()).collect()
    }

    /// Sends a request to every driver that answers `op` and returns at once; the drivers handle it before any
    /// later request, and collect_deferred() reads the answers when they are needed.
    pub fn send_deferred(&mut self, op: &str, args: Value) -> Deferred {
        let sent = self
            .drivers
            .iter_mut()
            .map(|driver| {
                if !driver.supports(op) {
                    return None;
                }
                let id = driver.send(op, &args).ok()?;
                driver.flush();
                if op == "sync" && !driver.manifest.cache.is_empty() {
                    driver.pending_sync = Some(id);
                }
                Some((driver.generation, id))
            })
            .collect();
        Deferred { sent }
    }

    /// Answers of a deferred request, each with the extensions its driver reads; a driver that restarted since
    /// will never answer it and is skipped.
    pub fn collect_deferred(&mut self, deferred: Deferred) -> Vec<(Vec<String>, Value)> {
        let mut answers = Vec::new();
        for (driver, sent) in self.drivers.iter_mut().zip(deferred.sent) {
            let Some((generation, id)) = sent else { continue };
            if generation != driver.generation {
                continue;
            }
            if driver.pending_sync == Some(id) {
                driver.settle_sync();
            }
            match driver.receive(&[id]).remove(&id) {
                Some(Ok(Value::Null)) | None => {}
                Some(Ok(mut value)) => {
                    driver.apply_invalidation(&mut value);
                    answers.push((driver.manifest.extensions.clone(), value))
                }
                Some(Err(_)) if driver.exited => {}
                Some(Err(error)) => {
                    self.warnings.insert(error);
                }
            }
        }
        answers
    }

    /// Sends every request to every driver that answers `op` before reading any answer, so drivers work in
    /// parallel; returns per request the non-null answers.
    /// Answers are shared, not copied: a kept answer can hold thousands of edges.
    pub fn ask_many(&mut self, op: &str, requests: &[Value]) -> Vec<Vec<Arc<Value>>> {
        let keys: Vec<String> = requests.iter().map(|request| format!("{op} {request}")).collect();
        let mut answers: Vec<Vec<Arc<Value>>> = vec![Vec::new(); requests.len()];
        let push = |answers: &mut Vec<Vec<Arc<Value>>>, warnings: &mut BTreeSet<String>, index: usize, answer: Result<Arc<Value>, String>| match answer {
            Ok(value) if value.is_null() => {}
            Ok(value) => answers[index].push(value),
            Err(error) => {
                warnings.insert(error);
            }
        };
        // Only what a driver has not answered since its last change is sent; every driver gets its share first,
        // then the answers are read, so drivers work in parallel.
        let mut sent = Vec::new();
        for driver in self.drivers.iter_mut() {
            if !driver.supports(op) {
                sent.push(None);
                continue;
            }
            let keeps = driver.keeps(op);
            if keeps {
                driver.settle_sync();
            }
            let missing: Vec<usize> = (0..requests.len()).filter(|&index| !keeps || !driver.answers.contains_key(&keys[index])).collect();
            let subset: Vec<Value> = missing.iter().map(|&index| requests[index].clone()).collect();
            let ids = driver.send_all(op, &subset);
            driver.flush();
            sent.push(Some((missing, subset, ids)));
        }
        for (driver, sent) in self.drivers.iter_mut().zip(sent) {
            let Some((missing, subset, ids)) = sent else { continue };
            let keeps = driver.keeps(op);
            let started = Instant::now();
            let collected = driver.collect(ids);
            profile_phase(&format!("{op} {}: {} asked", driver.name, subset.len()), started);
            let collected = driver.retry_after_exit(op, &subset, collected);
            let mut fresh: HashMap<usize, Result<Value, String>> = missing.into_iter().zip(collected).collect();
            for index in 0..requests.len() {
                let answer = match fresh.remove(&index) {
                    Some(answer) => {
                        let answer = answer.map(Arc::new);
                        if let (true, Ok(value)) = (keeps, &answer) {
                            let symbol = requests[index].get("symbol").and_then(Value::as_str).map(str::to_owned);
                            driver.answers.insert(keys[index].clone(), (symbol, Arc::clone(value)));
                        }
                        answer
                    }
                    None => match driver.answers.get(&keys[index]) {
                        Some((_, value)) => Ok(Arc::clone(value)),
                        None => continue,
                    },
                };
                push(&mut answers, &mut self.warnings, index, answer);
            }
        }
        answers
    }

    /// Asks only the drivers that read files with this one's extension (all of them when none claims it): a
    /// driver of another language would answer with a file-level guess.
    pub fn ask_many_about_file(&mut self, op: &str, requests: &[Value], file: &str) -> Vec<Vec<Arc<Value>>> {
        let extension = Path::new(file).extension().map(|extension| extension.to_string_lossy().to_lowercase()).unwrap_or_default();
        let claimed: Vec<bool> = self.drivers.iter().map(|driver| driver.manifest.extensions.iter().any(|claimed| claimed.eq_ignore_ascii_case(&extension))).collect();
        if !claimed.contains(&true) {
            return self.ask_many(op, requests);
        }
        let others: Vec<Driver> = {
            let mut kept = Vec::new();
            let mut others = Vec::new();
            for (driver, claims) in std::mem::take(&mut self.drivers).into_iter().zip(&claimed) {
                if *claims { kept.push(driver) } else { others.push(driver) }
            }
            self.drivers = kept;
            others
        };
        let answers = self.ask_many(op, requests);
        // Put every driver back in its original order.
        let mut kept = std::mem::take(&mut self.drivers).into_iter();
        let mut others = others.into_iter();
        self.drivers = claimed.iter().map(|claims| if *claims { kept.next().unwrap() } else { others.next().unwrap() }).collect();
        answers
    }

    pub fn ask(&mut self, op: &str, args: Value) -> Vec<Arc<Value>> {
        self.ask_many(op, &[args]).pop().unwrap_or_default()
    }
}
