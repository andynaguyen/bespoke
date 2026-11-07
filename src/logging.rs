use std::cell::Cell;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use env_filter::Builder as EnvFilterBuilder;
use log::{LevelFilter, Log, Metadata, Record};
use serde::Serialize;

use crate::app_handle::AppHandle;

pub static FILE_LOG_LEVEL: AtomicU8 = AtomicU8::new(LevelFilter::Debug as u8);

pub static LOG_STREAMING: AtomicBool = AtomicBool::new(false);

pub const LOG_EVENT: &str = "log://log";
const LOG_FILE_NAME: &str = "bespoke.log";
const MAX_FILE_SIZE: u64 = 500_000;

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
pub struct LogEventPayload {
    pub message: String,
    pub level: u8,
}

fn level_filter_from_u8(value: u8) -> LevelFilter {
    match value {
        0 => LevelFilter::Off,
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn,
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        _ => LevelFilter::Trace,
    }
}

fn file_level() -> LevelFilter {
    level_filter_from_u8(FILE_LOG_LEVEL.load(Ordering::Relaxed))
}

fn build_console_filter() -> env_filter::Filter {
    let mut builder = EnvFilterBuilder::new();

    match std::env::var("RUST_LOG") {
        Ok(spec) if !spec.trim().is_empty() => {
            if let Err(err) = builder.try_parse(&spec) {
                eprintln!(
                    "Ignoring invalid RUST_LOG value '{}': {}. Falling back to info-level console logging",
                    spec, err
                );
                builder.filter_level(LevelFilter::Info);
            }
        }
        _ => {
            builder.filter_level(LevelFilter::Info);
        }
    }

    builder.build()
}

struct LogFile {
    path: PathBuf,
    file: Option<File>,
}

impl LogFile {
    fn write_line(&mut self, line: &str) {
        if self.file.is_none() {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            self.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();
        }
        let too_big = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|meta| meta.len() > MAX_FILE_SIZE);
        if too_big {
            self.file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&self.path)
                .ok();
        }
        if let Some(file) = self.file.as_mut() {
            let _ = writeln!(file, "{line}");
        }
    }
}

struct BespokeLogger {
    console: env_filter::Filter,
    to_stderr: bool,
    file: Mutex<LogFile>,
}

thread_local! {
    static FORWARDING: Cell<bool> = const { Cell::new(false) };
}

impl Log for BespokeLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        self.console.enabled(metadata) || metadata.level() <= file_level()
    }

    fn log(&self, record: &Record) {
        let now = chrono::Local::now();
        let line = format!(
            "[{}][{}][{}] {}",
            now.format("%Y-%m-%d][%H:%M:%S"),
            record.target(),
            record.level(),
            record.args()
        );

        if self.console.matches(record) {
            if self.to_stderr {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }

        if record.level() <= file_level() {
            if let Ok(mut file) = self.file.lock() {
                file.write_line(&line);
            }

            if LOG_STREAMING.load(Ordering::Relaxed) && !FORWARDING.get() {
                FORWARDING.set(true);
                if let Some(app) = AppHandle::try_global() {
                    let level = match record.level() {
                        log::Level::Trace => 1,
                        log::Level::Debug => 2,
                        log::Level::Info => 3,
                        log::Level::Warn => 4,
                        log::Level::Error => 5,
                    };
                    let _ = app.emit(
                        LOG_EVENT,
                        LogEventPayload {
                            message: record.args().to_string(),
                            level,
                        },
                    );
                }
                FORWARDING.set(false);
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock()
            && let Some(file) = file.file.as_mut()
        {
            let _ = file.flush();
        }
    }
}

pub fn init(log_dir: PathBuf, headless: bool) {
    let logger = BespokeLogger {
        console: build_console_filter(),
        to_stderr: headless,
        file: Mutex::new(LogFile {
            path: log_dir.join(LOG_FILE_NAME),
            file: None,
        }),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(LevelFilter::Trace);
    }
}
