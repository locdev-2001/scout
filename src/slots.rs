//! Machine-wide work slots: the heavy background work of a server (reading a whole repository when it starts) waits
//! for one, so servers starting together share the machine instead of each taking all of it.
//!
//! A slot is an exclusive lock on one of a few files, released by the system when its holder ends, however it ends.
//! Every driver takes one the same way, whatever its language: it starts `scout hold-slot`, waits for its first line,
//! and keeps the helper's stdin open while it works. When the worker ends its end of the pipe closes, the helper
//! exits and the slot is free. A slot is also withheld while the machine is short of memory.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

/// Below this much available memory (MB) no slot is handed out: work waits rather than pushes the machine into swap.
const DEFAULT_MIN_AVAILABLE_MB: u64 = 1536;
const POLL: Duration = Duration::from_millis(200);

/// How many workers may run at once on this machine: `SCOUT_MAX_WORKERS` (0 means no limit), else half the cores and
/// at least one.
fn slot_count() -> usize {
    std::env::var("SCOUT_MAX_WORKERS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| (thread::available_parallelism().map_or(2, usize::from) / 2).max(1))
}

fn min_available_mb() -> u64 {
    std::env::var("SCOUT_MIN_AVAILABLE_MB").ok().and_then(|value| value.parse().ok()).unwrap_or(DEFAULT_MIN_AVAILABLE_MB)
}

fn slots_directory() -> PathBuf {
    crate::daemon::runtime_directory().join("work-slots")
}

/// What a driver is told to find the helper: nothing when slots are turned off (`SCOUT_MAX_WORKERS=0`).
pub fn driver_environment() -> Vec<(&'static str, String)> {
    if slot_count() == 0 {
        return Vec::new();
    }
    crate::daemon::own_binary().map(|binary| vec![("SCOUT_SLOT_HELPER", binary.to_string_lossy().into_owned())]).unwrap_or_default()
}

/// `scout hold-slot`: waits for a free slot (and enough memory), prints `slot <n>`, and holds it until stdin closes.
/// Ends at once when stdin closes first: the worker that wanted the slot is gone.
pub fn hold() -> u8 {
    thread::spawn(|| {
        let mut sink = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut sink);
        std::process::exit(0);
    });
    let count = slot_count();
    if count == 0 {
        println!("slot none");
        let _ = std::io::stdout().flush();
        thread::park();
        return 0;
    }
    let directory = slots_directory();
    let _ = fs::create_dir_all(&directory);
    let minimum = min_available_mb();
    loop {
        if crate::resources::system_memory_mb("MemAvailable").is_none_or(|available| available >= minimum) {
            for index in 0..count {
                let Ok(file) = File::options().create(true).truncate(false).write(true).open(directory.join(format!("slot-{index}"))) else { continue };
                if file.try_lock().is_ok() {
                    println!("slot {index}");
                    let _ = std::io::stdout().flush();
                    // Held for this process's life: stdin closing ends it, and the system releases the lock.
                    let _held = file;
                    loop {
                        thread::park();
                    }
                }
            }
        }
        thread::sleep(POLL);
    }
}

/// Slots in use right now, and how many there are, for `scout status`.
pub fn usage() -> (usize, usize) {
    let count = slot_count();
    let directory = slots_directory();
    let busy = (0..count)
        .filter(|index| File::options().write(true).open(directory.join(format!("slot-{index}"))).is_ok_and(|file| file.try_lock().is_err()))
        .count();
    (busy, count)
}
