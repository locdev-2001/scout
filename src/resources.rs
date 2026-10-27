//! CPU and memory of the server and its drivers, read from /proc (Linux) or libproc (macOS), so every answer can
//! state what it cost.

use std::fs;
use std::time::Instant;

/// Linux reports CPU time in clock ticks; USER_HZ is 100 on every mainstream Linux build. macOS time is counted in
/// the same unit.
const CLOCK_TICKS_PER_SECOND: u64 = 100;

/// User and system time of the process itself.
#[cfg(target_os = "macos")]
fn cpu_ticks(pid: u32) -> u64 {
    libproc::task_info(pid).map_or(0, |info| (info.total_user + info.total_system) * CLOCK_TICKS_PER_SECOND / 1_000_000_000)
}

/// Resident set size: on macOS a page a forked worker shares with its parent counts in both.
#[cfg(target_os = "macos")]
fn memory_kb(pid: u32) -> u64 {
    libproc::task_info(pid).map_or(0, |info| info.resident_bytes / 1024)
}

/// utime + stime + cutime + cstime: own CPU plus that of children already waited for (forked workers).
#[cfg(not(target_os = "macos"))]
fn cpu_ticks(pid: u32) -> u64 {
    let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else { return 0 };
    let Some(after_name) = stat.rfind(')').map(|index| &stat[index + 2..]) else { return 0 };
    after_name.split_whitespace().skip(11).take(4).filter_map(|field| field.parse::<u64>().ok()).sum()
}

/// Proportional set size: memory a process shares with its forks (PHP workers) counts once across them, so
/// the sum over the server and everything it started is what the machine actually spends. RSS where the
/// kernel has no `smaps_rollup`.
#[cfg(not(target_os = "macos"))]
fn memory_kb(pid: u32) -> u64 {
    let field = |file: &str, name: &str| {
        fs::read_to_string(format!("/proc/{pid}/{file}"))
            .ok()
            .and_then(|text| text.lines().find(|line| line.starts_with(name)).and_then(|line| line.split_whitespace().nth(1)?.parse().ok()))
    };
    field("smaps_rollup", "Pss:").or_else(|| field("status", "VmRSS:")).unwrap_or(0)
}

/// A line of `/proc/meminfo` in MB (`MemTotal`, `MemAvailable`); None where the system does not report it.
pub fn system_memory_mb(field: &str) -> Option<u64> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let line = text.lines().find(|line| line.split(':').next() == Some(field))?;
    line.split_whitespace().nth(1)?.parse::<u64>().ok().map(|kilobytes| kilobytes / 1024)
}

/// The processes and every live process they started (a language server, warm-up readers, forked workers).
fn with_descendants(roots: Vec<u32>) -> Vec<u32> {
    let mut all = Vec::new();
    let mut pending = roots;
    while let Some(pid) = pending.pop() {
        if all.contains(&pid) {
            continue;
        }
        all.push(pid);
        pending.extend(children_of(pid));
    }
    all
}

/// libproc, part of the system library on macOS: process facts without starting a `pgrep` or `ps` per question.
#[cfg(target_os = "macos")]
mod libproc {
    use std::ffi::c_void;

    extern "C" {
        fn proc_listchildpids(ppid: i32, buffer: *mut c_void, buffersize: i32) -> i32;
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut c_void, buffersize: i32) -> i32;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    const PROC_PIDTASKINFO: i32 = 4;

    #[repr(C)]
    #[derive(Default)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }

    /// `struct proc_taskinfo` of <sys/proc_info.h>.
    #[allow(dead_code)]
    #[repr(C)]
    #[derive(Default)]
    struct ProcTaskInfo {
        virtual_size: u64,
        resident_size: u64,
        total_user: u64,
        total_system: u64,
        threads_user: u64,
        threads_system: u64,
        policy: i32,
        faults: i32,
        pageins: i32,
        cow_faults: i32,
        messages_sent: i32,
        messages_received: i32,
        syscalls_mach: i32,
        syscalls_unix: i32,
        csw: i32,
        threadnum: i32,
        numrunning: i32,
        priority: i32,
    }

    pub struct TaskInfo {
        pub resident_bytes: u64,
        /// Nanoseconds.
        pub total_user: u64,
        pub total_system: u64,
    }

    /// Memory and CPU time of one process. The kernel counts CPU time in Mach time units (not nanoseconds on Apple
    /// silicon): converted here.
    pub fn task_info(pid: u32) -> Option<TaskInfo> {
        let mut info = ProcTaskInfo::default();
        let size = std::mem::size_of::<ProcTaskInfo>() as i32;
        // SAFETY: `info` is a `proc_taskinfo` of `size` bytes; the call writes at most that many.
        if unsafe { proc_pidinfo(pid as i32, PROC_PIDTASKINFO, 0, (&mut info as *mut ProcTaskInfo).cast(), size) } != size {
            return None;
        }
        let mut timebase = Timebase::default();
        // SAFETY: writes the two fields of `timebase`.
        if unsafe { mach_timebase_info(&mut timebase) } != 0 || timebase.denom == 0 {
            timebase = Timebase { numer: 1, denom: 1 };
        }
        let nanoseconds = |units: u64| (units as u128 * timebase.numer as u128 / timebase.denom as u128) as u64;
        Some(TaskInfo { resident_bytes: info.resident_size, total_user: nanoseconds(info.total_user), total_system: nanoseconds(info.total_system) })
    }

    pub fn children(pid: u32) -> Vec<u32> {
        let mut buffer = vec![0i32; 1024];
        // SAFETY: the buffer holds `buffersize` bytes; the call returns how many pids it wrote.
        let count = unsafe { proc_listchildpids(pid as i32, buffer.as_mut_ptr().cast(), (buffer.len() * std::mem::size_of::<i32>()) as i32) };
        buffer.truncate(count.max(0) as usize);
        buffer.into_iter().filter(|&child| child > 0).map(|child| child as u32).collect()
    }
}

/// Direct children of a process, from libproc.
#[cfg(target_os = "macos")]
fn children_of(pid: u32) -> Vec<u32> {
    libproc::children(pid)
}

/// Direct children of a process: from /proc on Linux, from `pgrep -P` where there is no /proc.
#[cfg(not(target_os = "macos"))]
fn children_of(pid: u32) -> Vec<u32> {
    let listed = match fs::read_dir(format!("/proc/{pid}/task")) {
        Ok(tasks) => tasks.filter_map(Result::ok).map(|task| fs::read_to_string(task.path().join("children")).unwrap_or_default()).collect::<Vec<_>>().join(" "),
        Err(_) => std::process::Command::new("pgrep").args(["-P", &pid.to_string()]).output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default(),
    };
    listed.split_whitespace().filter_map(|child| child.parse::<u32>().ok()).collect()
}

/// Tells a process tree that is working on an answer from one that waits for something that will never come:
/// on Linux and macOS by its CPU time, elsewhere only by a long wall-clock limit.
pub struct Progress {
    pid: u32,
    ticks: Option<u64>,
    since: Instant,
}

/// Working processes use some CPU every few seconds; this long without any is a hang.
const IDLE_LIMIT_SECONDS: u64 = 20;
/// Where CPU time cannot be read, a question still answered after this long is taken as hung.
const WALL_LIMIT_SECONDS: u64 = 300;

impl Progress {
    pub fn new(pid: u32) -> Self {
        Self { pid, ticks: Self::tree_ticks(pid), since: Instant::now() }
    }

    fn tree_ticks(pid: u32) -> Option<u64> {
        cfg!(any(target_os = "linux", target_os = "macos")).then(|| with_descendants(vec![pid]).into_iter().map(cpu_ticks).sum())
    }

    /// Whether the tree has done nothing for too long; called while waiting.
    pub fn stalled(&mut self) -> bool {
        let ticks = Self::tree_ticks(self.pid);
        if ticks.is_some() && ticks != self.ticks {
            self.ticks = ticks;
            self.since = Instant::now();
        }
        let limit = if ticks.is_some() { IDLE_LIMIT_SECONDS } else { WALL_LIMIT_SECONDS };
        self.since.elapsed().as_secs() >= limit
    }

    pub fn idle_seconds(&self) -> u64 {
        self.since.elapsed().as_secs()
    }
}

/// What each process of a tree is doing, one line each (`pid state wait-channel command`), for the log of a hang.
pub fn describe_tree(pid: u32) -> String {
    if !cfg!(target_os = "linux") {
        // No /proc: one `ps` over the whole tree (only ever run when a driver hangs).
        let pids: Vec<String> = with_descendants(vec![pid]).into_iter().map(|pid| pid.to_string()).collect();
        let listed = std::process::Command::new("ps").args(["-o", "pid=,stat=,wchan=,command=", "-p", &pids.join(",")]).output().map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
        return listed.lines().map(|line| format!("  {}", line.trim().chars().take(200).collect::<String>())).collect::<Vec<_>>().join("\n");
    }
    let read = |pid: u32, file: &str| fs::read_to_string(format!("/proc/{pid}/{file}")).unwrap_or_default();
    with_descendants(vec![pid])
        .into_iter()
        .map(|pid| {
            let state = read(pid, "stat").rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned)).unwrap_or_default();
            let command: String = read(pid, "cmdline").replace('\0', " ").chars().take(160).collect();
            format!("  {pid} {state} {} {command}", read(pid, "wchan"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Ends a process and everything it started (forked workers, readers), so none is left waiting.
pub fn kill_tree(pid: u32) {
    let pids: Vec<String> = with_descendants(vec![pid]).into_iter().map(|pid| pid.to_string()).collect();
    if cfg!(unix) {
        let _ = std::process::Command::new("kill").arg("-KILL").args(&pids).status();
    }
}

pub struct QueryCost {
    started: Instant,
    /// CPU ticks of each process when the query started.
    ticks_before: Vec<(u32, u64)>,
}

impl QueryCost {
    pub fn start(driver_pids: Vec<u32>) -> Self {
        let mut pids = vec![std::process::id()];
        pids.extend(driver_pids);
        let ticks_before = with_descendants(pids).into_iter().map(|pid| (pid, cpu_ticks(pid))).collect();
        Self { started: Instant::now(), ticks_before }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    pub fn finish(&self, driver_pids: Vec<u32>) -> String {
        let wall_ms = self.started.elapsed().as_secs_f64() * 1000.0;
        let mut roots = vec![std::process::id()];
        roots.extend(driver_pids);
        let restarted = !roots.iter().all(|pid| self.ticks_before.iter().any(|(known, _)| known == pid));
        let pids = with_descendants(roots);
        // A process that ended during the query took its CPU time with it; one that started counts from zero.
        let cpu_ms_ticks: u64 = pids.iter().map(|&pid| cpu_ticks(pid).saturating_sub(self.ticks_before.iter().find(|(known, _)| *known == pid).map_or(0, |(_, ticks)| *ticks))).sum();
        let cpu_ms = cpu_ms_ticks * 1000 / CLOCK_TICKS_PER_SECOND;
        let memory_mb = pids.iter().map(|&pid| memory_kb(pid)).sum::<u64>() as f64 / 1024.0;
        let restarted = if restarted { ", a driver restarted" } else { "" };
        if cfg!(target_os = "linux") {
            format!("Cost: {wall_ms:.0} ms wall, ~{cpu_ms} ms CPU (10 ms resolution), {memory_mb:.0} MB memory (proportional: server, drivers and what they started){restarted}")
        } else if cfg!(target_os = "macos") {
            format!("Cost: {wall_ms:.0} ms wall, ~{cpu_ms} ms CPU (10 ms resolution), {memory_mb:.0} MB memory (resident: server, drivers and what they started; pages a fork shares count twice){restarted}")
        } else {
            // CPU and memory of other processes are read from /proc or libproc, which only Linux and macOS have.
            format!("Cost: {wall_ms:.0} ms wall (CPU and memory not measured on this OS){restarted}")
        }
    }
}

/// Appends one timing line to the file named by SCOUT_PROFILE, when set; used to measure, never to answer.
pub fn profile_phase(phase: &str, started: Instant) {
    if let Some(target) = std::env::var_os("SCOUT_PROFILE").filter(|target| !target.is_empty()) {
        let line = format!("core {phase:<28} {:7.1} ms\n", started.elapsed().as_secs_f64() * 1000.0);
        if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(target) {
            let _ = std::io::Write::write_all(&mut file, line.as_bytes());
        }
    }
}
