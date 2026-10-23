//! A small log file for bug reports: failed operations, load errors, agent errors and crashes
//! (with a backtrace). Linux `$XDG_STATE_HOME/branchblitz/branchblitz.log` (default
//! `~/.local/state`); elsewhere next to the settings. Only the windowed app logs (`init`): tests
//! and the MCP server leave the user's log alone. Kept under 1 MB (the previous one becomes `.old`).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static FILE: Mutex<()> = Mutex::new(());
const MAX: u64 = 1024 * 1024;

pub fn dir() -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::home_dir().map(|h| h.join(".local/state")))
            .map(|d| d.join("branchblitz"))
    } else {
        eframe::storage_dir("branchblitz")
    }
}

pub fn path() -> Option<PathBuf> {
    dir().map(|d| d.join("branchblitz.log"))
}

/// Marks that the app crashed, for the next start.
fn crash_marker() -> Option<PathBuf> {
    dir().map(|d| d.join("crashed"))
}

/// Starts logging and installs the crash handler. Returns whether the previous run crashed.
pub fn init() -> bool {
    let Some(dir) = dir() else { return false };
    let _ = std::fs::create_dir_all(&dir);
    if let Some(p) = path()
        && std::fs::metadata(&p).is_ok_and(|m| m.len() > MAX)
    {
        let _ = std::fs::rename(&p, p.with_extension("log.old"));
    }
    ENABLED.store(true, Ordering::Relaxed);
    let crashed = crash_marker().is_some_and(|m| std::fs::remove_file(m).is_ok());
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        write("CRASH", &format!("{info}\n{backtrace}"));
        if let Some(m) = crash_marker() {
            let _ = std::fs::write(m, "");
        }
        default(info);
    }));
    write(
        "INFO",
        &format!(
            "BranchBlitz {} started ({} {})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    );
    crashed
}

/// Appends a line (or block) with a timestamp; does nothing when logging is off.
pub fn write(level: &str, msg: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(p) = path() else { return };
    let _guard = FILE.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "{now} {level} {msg}");
    }
}
