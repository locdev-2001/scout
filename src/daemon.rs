//! One warm server per repository keeps the drivers booted, so a query costs only the search itself.
//! Freshness stays with the drivers: on every query they check the code they loaded and restart when it changed.

use crate::driver::DriverPool;
use crate::freshness::Freshness;
use crate::ipc::{self, Stream};
use crate::query::{execute, refresh, QueryRequest, QueryResponse};
use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const IDLE_SECONDS: u64 = 10 * 60;
const STARTUP_WAIT: Duration = Duration::from_secs(15);

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3))
}

/// This program's file. A long-lived process (the MCP server an agent keeps open) outlives a rebuild: Linux then
/// names its old, deleted file `scout (deleted)`, and the new build at the original path is the one to run.
pub fn own_binary() -> std::io::Result<PathBuf> {
    let current = env::current_exe()?;
    let text = current.to_string_lossy();
    match text.strip_suffix(" (deleted)").map(PathBuf::from) {
        Some(rebuilt) if rebuilt.is_file() => Ok(rebuilt),
        _ => Ok(current),
    }
}

/// Which build of this program runs: its file's modification time.
fn build_stamp() -> String {
    own_binary().and_then(fs::metadata).and_then(|metadata| metadata.modified()).map(|modified| format!("{modified:?}")).unwrap_or_default()
}

/// The build is part of the name, so a rebuilt binary never talks to a server running old code.
fn socket_path(repo_root: &Path) -> PathBuf {
    runtime_directory().join(format!("{:016x}.sock", fnv1a(&format!("{}|{}", repo_root.display(), build_stamp()))))
}

/// A running server, as its `.info` file tells: `pid repo` on the first line, `build <stamp>` on the second. The
/// file's modification time is the last question it answered; a `.busy` file beside it means it is answering one.
pub struct RunningServer {
    pub pid: u32,
    pub repo: String,
    pub log: PathBuf,
    build: String,
    info: PathBuf,
    last_answer: SystemTime,
    busy: bool,
}

fn alive(pid: u32) -> bool {
    Command::new("kill").args(["-0", &pid.to_string()]).output().is_ok_and(|output| output.status.success())
}

pub fn running_servers() -> Vec<RunningServer> {
    let Ok(entries) = fs::read_dir(runtime_directory()) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "info"))
        .filter_map(|info| {
            let text = fs::read_to_string(&info).ok()?;
            let mut lines = text.lines();
            let (pid, repo) = lines.next()?.split_once(' ')?;
            let pid: u32 = pid.parse().ok()?;
            let build = lines.next().and_then(|line| line.strip_prefix("build ")).unwrap_or_default().to_owned();
            let last_answer = fs::metadata(&info).and_then(|metadata| metadata.modified()).unwrap_or(UNIX_EPOCH);
            alive(pid).then(|| RunningServer { pid, repo: repo.to_owned(), log: info.with_extension("log"), build, busy: info.with_extension("busy").exists(), last_answer, info })
        })
        .collect()
}

/// Ends a server and everything it started, and waits until it is gone: a question asked right after must start a
/// new server, not reach this one.
pub fn stop_server(server: &RunningServer) {
    // The whole tree: a language server still loading a project (gigabytes) must not outlive it.
    crate::resources::kill_tree(server.pid);
    for _ in 0..100 {
        if !alive(server.pid) {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = fs::remove_file(&server.info);
    let _ = fs::remove_file(server.info.with_extension("busy"));
}

/// How much memory (MB) every server together may spend: `SCOUT_MEMORY_BUDGET`, else 40 % of the machine's; 0 means
/// no limit.
pub fn memory_budget_mb() -> u64 {
    env::var("SCOUT_MEMORY_BUDGET").ok().and_then(|value| value.parse().ok()).unwrap_or_else(|| crate::resources::system_memory_mb("MemTotal").map_or(0, |total| total * 2 / 5))
}

/// A server asked within this long is in use: it is never stopped to make room.
const RECENTLY_ASKED: Duration = Duration::from_secs(60);
const BUDGET_CHECK: Duration = Duration::from_secs(30);

/// Stops the servers of an older build for this repository, then, while all servers together spend more than the
/// memory budget, the one asked longest ago. Run by a server, never by a client: a hook starting a server must not
/// wait for others to stop. One server at a time does it, so two never stop more than needed.
fn enforce_budget(own_pid: u32, repo_root: &Path) {
    let Ok(lock) = File::create(runtime_directory().join("budget.lock")) else { return };
    if lock.try_lock().is_err() {
        return;
    }
    let (repo, build) = (repo_root.display().to_string(), build_stamp());
    let (outdated, others): (Vec<_>, Vec<_>) = running_servers().into_iter().filter(|server| server.pid != own_pid).partition(|server| server.repo == repo && server.build != build);
    for server in outdated.iter().filter(|server| !server.busy) {
        eprintln!("stopping server {} of an older build", server.pid);
        stop_server(server);
    }
    let budget = memory_budget_mb();
    if budget == 0 {
        return;
    }
    let mut spent: Vec<(RunningServer, u64)> = others.into_iter().map(|server| {
        let memory = crate::resources::tree_memory_mb(server.pid);
        (server, memory)
    }).collect();
    let mut total = crate::resources::tree_memory_mb(own_pid) + spent.iter().map(|(_, memory)| memory).sum::<u64>();
    spent.retain(|(server, _)| !server.busy && server.last_answer.elapsed().unwrap_or_default() >= RECENTLY_ASKED);
    spent.sort_by_key(|(server, _)| server.last_answer);
    for (server, memory) in spent {
        if total <= budget {
            break;
        }
        eprintln!("stopping server {} of {} ({memory} MB): all servers spend {total} MB, the budget is {budget} MB", server.pid, server.repo);
        stop_server(&server);
        total = total.saturating_sub(memory);
    }
}

/// Where servers keep their socket, lock, log and `.info` (pid and repository), one set per repository.
pub fn runtime_directory() -> PathBuf {
    let user = env::var("USER").or_else(|_| env::var("USERNAME")).unwrap_or_default();
    env::temp_dir().join(format!("scout-{user}"))
}

/// Where analysis kept between servers lives (saved facts, rust-analyzer's files), one folder per repository;
/// drivers get it as `SCOUT_CACHE`.
pub fn cache_directory() -> PathBuf {
    env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(env::temp_dir)
        .join("scout")
}

fn now_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|elapsed| elapsed.as_secs()).unwrap_or_default()
}

fn start_server(repo_root: &Path, socket: &Path) -> Result<(), String> {
    // Appended to, so the story of a server that stopped is still there when the next one starts.
    let log = fs::OpenOptions::new().create(true).append(true).open(socket.with_extension("log")).map_err(|error| error.to_string())?;
    let mut command = Command::new(own_binary().map_err(|error| error.to_string())?);
    command.arg("serve").arg(repo_root).stdin(Stdio::null()).stdout(Stdio::null()).stderr(log);
    ipc::detach(&mut command);
    let mut child = command.spawn().map_err(|error| format!("cannot start server: {error}"))?;
    // A long-lived client (the MCP server) must collect the server's exit status, or it stays a zombie.
    thread::spawn(move || child.wait());
    Ok(())
}

fn connect(repo_root: &Path, start: bool) -> Result<Stream, String> {
    let socket = socket_path(repo_root);
    if let Ok(stream) = ipc::connect(&socket) {
        return Ok(stream);
    }
    if !start {
        return Err("no server runs for this repository".to_owned());
    }
    fs::create_dir_all(socket.parent().unwrap()).map_err(|error| error.to_string())?;
    start_server(repo_root, &socket)?;
    let deadline = SystemTime::now() + STARTUP_WAIT;
    while SystemTime::now() < deadline {
        thread::sleep(Duration::from_millis(10));
        if let Ok(stream) = ipc::connect(&socket) {
            return Ok(stream);
        }
    }
    let log = fs::read_to_string(socket.with_extension("log")).unwrap_or_default();
    Err(format!("server did not start: {}", log.trim()))
}

/// Starts the repository's server when none runs, without waiting for it: it reads the code while the agent works,
/// so the next question finds it ready. A second start for the same repository is turned away by the server's lock.
pub fn ensure_running(repo_root: &Path) -> Result<(), String> {
    let socket = socket_path(repo_root);
    if ipc::connect(&socket).is_ok() {
        return Ok(());
    }
    fs::create_dir_all(socket.parent().unwrap()).map_err(|error| error.to_string())?;
    start_server(repo_root, &socket)
}

pub fn query(repo_root: &Path, request: &QueryRequest) -> Result<QueryResponse, String> {
    ask(repo_root, request, true)
}

/// A question to a server already running for the repository, never starting one (a hook must not make a
/// text search wait for a repository to be read).
pub fn query_running(repo_root: &Path, request: &QueryRequest) -> Result<QueryResponse, String> {
    ask(repo_root, request, false)
}

fn ask(repo_root: &Path, request: &QueryRequest, start: bool) -> Result<QueryResponse, String> {
    let started = std::time::Instant::now();
    let mut stream = connect(repo_root, start)?;
    let line = serde_json::to_string(request).map_err(|error| error.to_string())?;
    writeln!(stream, "{line}").map_err(|error| error.to_string())?;
    let mut answer = String::new();
    BufReader::new(stream).read_line(&mut answer).map_err(|error| error.to_string())?;
    let mut response: QueryResponse = serde_json::from_str(&answer).map_err(|_| "server closed the connection without an answer".to_owned())?;
    // `Cost:` covers answering; the asker also waited for a server to start or to finish another question.
    let waited = (started.elapsed().as_millis() as u64).saturating_sub(response.answered_ms);
    if waited >= WAIT_WORTH_REPORTING_MS {
        response.stderr.push_str(&format!("Waited: {waited} ms before the server took this question (starting, or busy with another).\n"));
    }
    Ok(response)
}

const WAIT_WORTH_REPORTING_MS: u64 = 200;

enum Work {
    Query(Stream),
    FilesChanged,
}

/// How long a burst of file events may last before the drivers are brought up to date.
const CHANGE_SETTLE: Duration = Duration::from_millis(30);

/// Watches each repo directory (not ignored by git) on its own, so ignored trees such as vendor/ or
/// node_modules/ cost no watch. If the OS refuses (a watch limit), queries still sweep, only later.
struct FileWatcher {
    watcher: Option<notify::RecommendedWatcher>,
    watched: std::collections::HashSet<PathBuf>,
}

impl FileWatcher {
    fn start(work: mpsc::Sender<Work>) -> Self {
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event {
                let relevant = !matches!(event.kind, notify::EventKind::Access(_)) && event.paths.iter().any(|path| !path.components().any(|part| part.as_os_str() == ".git"));
                if relevant {
                    let _ = work.send(Work::FilesChanged);
                }
            }
        });
        match watcher {
            Ok(watcher) => Self { watcher: Some(watcher), watched: std::collections::HashSet::new() },
            Err(error) => {
                eprintln!("file watching unavailable ({error}); changes are picked up when queried");
                Self { watcher: None, watched: std::collections::HashSet::new() }
            }
        }
    }

    fn follow(&mut self, freshness: &Freshness) {
        let Some(watcher) = self.watcher.as_mut() else { return };
        let added: Vec<PathBuf> = freshness.watched_directories().into_iter().filter(|directory| !self.watched.contains(directory)).collect();
        if added.is_empty() {
            return;
        }
        // Added together: macOS restarts its event stream with every watched directory on each change, so one
        // directory at a time costs a large repository tens of seconds before the first question is taken.
        let mut paths = notify::Watcher::paths_mut(watcher);
        for directory in &added {
            if let Err(error) = paths.add(directory, notify::RecursiveMode::NonRecursive) {
                eprintln!("stopped watching files at {} ({error}); changes are picked up when queried", directory.display());
                drop(paths);
                self.watcher = None;
                return;
            }
        }
        if let Err(error) = paths.commit() {
            eprintln!("stopped watching files ({error}); changes are picked up when queried");
            self.watcher = None;
            return;
        }
        self.watched.extend(added);
    }
}

fn answer(stream: &Stream, pool: &mut DriverPool, freshness: &mut Freshness, repo_root: &Path, last_activity: &AtomicU64, socket: &Path) {
    last_activity.store(0, Ordering::Relaxed);
    let busy = socket.with_extension("busy");
    let _ = File::create(&busy);
    answer_question(stream, pool, freshness, repo_root);
    let _ = fs::remove_file(&busy);
    // Its modification time tells other servers when this one was last asked (see `enforce_budget`).
    if let Ok(info) = fs::read_to_string(socket.with_extension("info")) {
        let _ = fs::write(socket.with_extension("info"), info);
    }
    last_activity.store(now_seconds(), Ordering::Relaxed);
}

fn answer_question(stream: &Stream, pool: &mut DriverPool, freshness: &mut Freshness, repo_root: &Path) {
    let mut request_line = String::new();
    if BufReader::new(stream).read_line(&mut request_line).is_err() {
        return;
    }
    let response = match serde_json::from_str::<QueryRequest>(&request_line) {
        Ok(request) => execute(pool, freshness, repo_root, &request, &|| ipc::hung_up(stream)),
        Err(error) => QueryResponse { stderr: format!("bad request: {error}\n"), exit_code: 2, ..Default::default() },
    };
    let mut writer = stream;
    let _ = writeln!(writer, "{}", serde_json::to_string(&response).unwrap_or_default());
}

pub fn serve(drivers_directory: &Path, repo_root: &Path) -> Result<(), String> {
    let socket = socket_path(repo_root);
    // One server per repository: several clients may start one at the same moment, and a second server
    // binding the address would orphan the first with the questions it is answering. The lock is held for
    // the server's life and released by the system when it ends, however it ends.
    let lock = File::create(socket.with_extension("lock")).map_err(|error| error.to_string())?;
    if lock.try_lock().is_err() || ipc::connect(&socket).is_ok() {
        return Ok(());
    }
    eprintln!("server {} started for {}", std::process::id(), repo_root.display());
    let _ = fs::write(socket.with_extension("info"), format!("{} {}\nbuild {}\n", std::process::id(), repo_root.display(), build_stamp()));
    let budget_repo = repo_root.to_path_buf();
    thread::spawn(move || loop {
        enforce_budget(std::process::id(), &budget_repo);
        thread::sleep(BUDGET_CHECK);
    });
    let listener = ipc::listen(&socket).map_err(|error| format!("cannot listen on {}: {error}", socket.display()))?;
    let started = std::time::Instant::now();
    let mut pool = DriverPool::start(drivers_directory, repo_root)?;
    crate::resources::profile_phase("start: drivers spawned", started);
    let mut freshness = Freshness::new(repo_root.to_path_buf());
    freshness.sweep();
    crate::resources::profile_phase("start: first sweep", started);

    let last_activity = Arc::new(AtomicU64::new(now_seconds()));
    let watchdog_activity = Arc::clone(&last_activity);
    let watchdog_socket = socket.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(30));
        // While a question is being answered the activity mark is 0: never idle.
        let last = watchdog_activity.load(Ordering::Relaxed);
        if last != 0 && now_seconds().saturating_sub(last) > IDLE_SECONDS {
            let _ = fs::remove_file(&watchdog_socket);
            let _ = fs::remove_file(watchdog_socket.with_extension("info"));
            drop(lock);
            std::process::exit(0);
        }
    });

    let (work, inbox) = mpsc::channel::<Work>();
    let connections = work.clone();
    thread::spawn(move || {
        while let Ok(stream) = listener.accept() {
            if connections.send(Work::Query(stream)).is_err() {
                break;
            }
        }
    });
    let mut watcher = FileWatcher::start(work);
    crate::resources::profile_phase("start: watcher started", started);
    watcher.follow(&freshness);
    crate::resources::profile_phase("start: watcher following", started);

    while let Ok(next) = inbox.recv() {
        match next {
            Work::FilesChanged => {
                // An editor often writes several files (or one file twice) in a burst: take them together.
                let mut pending_query = None;
                while let Ok(more) = inbox.recv_timeout(CHANGE_SETTLE) {
                    if let Work::Query(stream) = more {
                        pending_query = Some(stream);
                        break;
                    }
                }
                refresh(&mut pool, &mut freshness);
                watcher.follow(&freshness);
                if let Some(stream) = pending_query {
                    answer(&stream, &mut pool, &mut freshness, repo_root, &last_activity, &socket);
                }
            }
            Work::Query(stream) => answer(&stream, &mut pool, &mut freshness, repo_root, &last_activity, &socket),
        }
    }
    Ok(())
}
