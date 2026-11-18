//! Logging to stderr (Console.app / the terminal) and, on macOS, to
//! `~/Library/Logs/Lumora/lumora.log` so a user can send a trace after
//! "nothing happened" or a crash.
//!
//! Level: `LUMORA_LOG` (`error|warn|info|debug|trace`) wins; otherwise
//! `debug` when `LUMORA_DEBUG=1` or `RUST_LOG` is set; otherwise `info`.
//! The file is rotated to `lumora.log.1` once it passes 5 MB at launch.
//! [`init`] also installs a panic hook that writes the panic (with a
//! backtrace) to the same file before the process aborts.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

struct Logger {
    level: LevelFilter,
    file: Option<Mutex<File>>,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let tag = match record.level() {
            Level::Error => "ERROR",
            Level::Warn => "WARN ",
            Level::Info => "INFO ",
            Level::Debug => "DEBUG",
            Level::Trace => "TRACE",
        };
        eprintln!("[lumora {tag}] {}: {}", record.target(), record.args());
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = writeln!(f, "{} {tag} {}: {}", timestamp(), record.target(), record.args());
            }
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            if let Ok(mut f) = file.lock() {
                let _ = f.flush();
            }
        }
    }
}

/// Pick the level from the environment (see the module docs).
fn level_from(lumora_log: Option<&str>, lumora_debug: Option<&str>, rust_log: bool) -> LevelFilter {
    match lumora_log {
        Some("error") => return LevelFilter::Error,
        Some("warn") => return LevelFilter::Warn,
        Some("info") => return LevelFilter::Info,
        Some("debug") => return LevelFilter::Debug,
        Some("trace") => return LevelFilter::Trace,
        _ => {}
    }
    let debug_flag = lumora_debug.is_some_and(|v| !v.is_empty() && v != "0");
    if debug_flag || rust_log {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    }
}

/// `~/Library/Logs/Lumora` (macOS only).
pub fn log_dir() -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Logs/Lumora"))
}

/// `~/Library/Logs/Lumora/lumora.log`.
pub fn log_path() -> Option<PathBuf> {
    log_dir().map(|d| d.join("lumora.log"))
}

fn open_log_file() -> Option<File> {
    let dir = log_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("lumora.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("lumora.log.1"));
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// UTC `YYYY-MM-DD HH:MM:SS` (no date crate needed).
fn timestamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format_utc(secs)
}

fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn init() {
    let level = level_from(
        std::env::var("LUMORA_LOG").ok().as_deref(),
        std::env::var("LUMORA_DEBUG").ok().as_deref(),
        std::env::var_os("RUST_LOG").is_some(),
    );
    let file = open_log_file().map(Mutex::new);
    let has_file = file.is_some();
    let logger = Box::leak(Box::new(Logger { level, file }));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
    install_panic_hook();
    if has_file {
        log::info!("---- Lumora {} log ({level}) ----", env!("CARGO_PKG_VERSION"));
    }
}

/// Log panics (message, location, backtrace) to the log file and stderr
/// before the default hook runs. Release builds abort on panic, so this is
/// the only trace a crash leaves besides the system crash report.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_string();
        let msg = format!("PANIC on thread {thread}: {info}\n{bt}");
        if let Some(mut f) = log_path().and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok()) {
            let _ = writeln!(f, "{} ERROR {msg}", timestamp());
            let _ = f.flush();
        }
        eprintln!("[lumora ERROR] {msg}");
        default(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_selection() {
        assert_eq!(level_from(None, None, false), LevelFilter::Info);
        assert_eq!(level_from(None, Some("1"), false), LevelFilter::Debug);
        assert_eq!(level_from(None, Some("0"), false), LevelFilter::Info);
        assert_eq!(level_from(None, None, true), LevelFilter::Debug);
        assert_eq!(level_from(Some("warn"), Some("1"), true), LevelFilter::Warn);
        assert_eq!(level_from(Some("trace"), None, false), LevelFilter::Trace);
        assert_eq!(level_from(Some("bogus"), None, false), LevelFilter::Info);
    }

    #[test]
    fn utc_formatting() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00:00Z");
        assert_eq!(format_utc(1_795_000_000), "2026-11-18 11:06:40Z");
    }
}
