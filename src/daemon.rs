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

/// The binary's own mtime is part of the name, so a rebuilt binary never talks to a server running old code.
fn socket_path(repo_root: &Path) -> PathBuf {
    let build = own_binary()
        .and_then(fs::metadata)
        .and_then(|metadata| metadata.modified())
        .map(|modified| format!("{modified:?}"))
        .unwrap_or_default();
    runtime_directory().join(format!("{:016x}.sock", fnv1a(&format!("{}|{build}", repo_root.display()))))
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
        for directory in freshness.watched_directories() {
            if self.watched.contains(&directory) {
                continue;
            }
            match notify::Watcher::watch(watcher, &directory, notify::RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.watched.insert(directory);
                }
                Err(error) => {
                    eprintln!("stopped watching files at {} ({error}); changes are picked up when queried", directory.display());
                    self.watcher = None;
                    return;
                }
            }
        }
    }
}

fn answer(stream: &Stream, pool: &mut DriverPool, freshness: &mut Freshness, repo_root: &Path, last_activity: &AtomicU64) {
    last_activity.store(0, Ordering::Relaxed);
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
    last_activity.store(now_seconds(), Ordering::Relaxed);
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
    let _ = fs::write(socket.with_extension("info"), format!("{} {}", std::process::id(), repo_root.display()));
    let listener = ipc::listen(&socket).map_err(|error| format!("cannot listen on {}: {error}", socket.display()))?;
    let mut pool = DriverPool::start(drivers_directory, repo_root)?;
    let mut freshness = Freshness::new(repo_root.to_path_buf());
    freshness.sweep();

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
    watcher.follow(&freshness);

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
                    answer(&stream, &mut pool, &mut freshness, repo_root, &last_activity);
                }
            }
            Work::Query(stream) => answer(&stream, &mut pool, &mut freshness, repo_root, &last_activity),
        }
    }
    Ok(())
}
