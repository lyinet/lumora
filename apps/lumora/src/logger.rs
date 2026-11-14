//! Minimal stderr logger (visible in Console.app when launched from a
//! bundle, and in the terminal during development). Level from
//! `LUMORA_LOG` (`error|warn|info|debug|trace`), default `info`.

use log::{Level, LevelFilter, Log, Metadata, Record};

struct Logger {
    level: LevelFilter,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let tag = match record.level() {
                Level::Error => "ERROR",
                Level::Warn => "WARN ",
                Level::Info => "INFO ",
                Level::Debug => "DEBUG",
                Level::Trace => "TRACE",
            };
            eprintln!("[lumora {tag}] {}: {}", record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    let level = match std::env::var("LUMORA_LOG").as_deref() {
        Ok("error") => LevelFilter::Error,
        Ok("warn") => LevelFilter::Warn,
        Ok("debug") => LevelFilter::Debug,
        Ok("trace") => LevelFilter::Trace,
        _ => LevelFilter::Info,
    };
    let logger = Box::leak(Box::new(Logger { level }));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(level);
    }
}
