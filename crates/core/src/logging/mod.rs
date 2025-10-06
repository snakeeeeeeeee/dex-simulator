use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::Local;
use log::LevelFilter;
use once_cell::sync::OnceCell;
use tracing::{subscriber, Level, Metadata};
use tracing_log::LogTracer;
use tracing_subscriber::{
    filter::EnvFilter,
    fmt::{self, format::Writer, time::FormatTime, writer::MakeWriter},
    layer::SubscriberExt,
    registry,
};

use crate::config::LoggingConfig;

static INIT_GUARD: OnceCell<()> = OnceCell::new();

/// 初始化 tracing 日志系统，支持终端与按级别分流的文件日志。
pub fn init_logging(config: &LoggingConfig) -> Result<(), LogInitError> {
    if INIT_GUARD.get().is_some() {
        return Ok(());
    }

    if let Err(err) = LogTracer::builder()
        .with_max_level(LevelFilter::Trace)
        .init()
    {
        if !err.to_string().contains("already") {
            return Err(LogInitError::Bridge(err.to_string()));
        }
    }

    let directive = resolve_directive(config)?;
    let env_filter = EnvFilter::try_new(directive)
        .or_else(|_| EnvFilter::try_new(config.level.trim()))
        .map_err(|err| LogInitError::InvalidFilter(err.to_string()))?;

    let log_dir = Path::new(&config.directory);
    if !log_dir.exists() {
        fs::create_dir_all(log_dir)
            .map_err(|err| LogInitError::Io(format!("创建日志目录失败: {}", err)))?;
    }

    let max_size_bytes = if config.max_size_mb == 0 {
        u64::MAX
    } else {
        config.max_size_mb.saturating_mul(1024 * 1024)
    };
    let max_age = if config.retention_hours == 0 {
        Duration::MAX
    } else {
        Duration::from_secs(config.retention_hours.saturating_mul(3600))
    };

    let info_path = log_dir.join("info.log");
    let error_path = log_dir.join("error.log");
    let info_writer = RotatingFileWriter::new(info_path, max_size_bytes, max_age)
        .map_err(|err| LogInitError::Io(format!("初始化 info.log 失败: {}", err)))?;
    let error_writer = RotatingFileWriter::new(error_path, max_size_bytes, max_age)
        .map_err(|err| LogInitError::Io(format!("初始化 error.log 失败: {}", err)))?;

    let split_writer = SplitLogWriter::new(info_writer, error_writer);

    let shared_format = fmt::format()
        .with_timer(LocalTimer)
        .with_level(true)
        .with_target(true)
        .with_thread_names(true)
        .with_source_location(true);

    let console_layer = fmt::layer()
        .event_format(shared_format.clone())
        .with_writer(std::io::stdout)
        .with_ansi(true);

    let file_layer = fmt::layer()
        .event_format(shared_format)
        .with_ansi(false)
        .with_writer(split_writer);

    let subscriber = registry::Registry::default()
        .with(env_filter)
        .with(console_layer)
        .with(file_layer);

    match subscriber::set_global_default(subscriber) {
        Ok(()) => {
            let _ = INIT_GUARD.set(());
            Ok(())
        }
        Err(err) => {
            if err.to_string().contains("already") {
                let _ = INIT_GUARD.set(());
                Ok(())
            } else {
                Err(LogInitError::Init(err.to_string()))
            }
        }
    }
}

fn resolve_directive(config: &LoggingConfig) -> Result<String, LogInitError> {
    if let Some(path) = &config.config_file {
        let cfg_path = Path::new(path);
        if cfg_path.exists() {
            let raw = fs::read_to_string(cfg_path)
                .map_err(|err| LogInitError::ConfigFile(err.to_string()))?;
            let directives: Vec<&str> = raw
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .collect();
            if !directives.is_empty() {
                return Ok(directives.join(","));
            }
        }
    }

    Ok(config.level.trim().to_string())
}

#[derive(Clone)]
struct RotatingFileWriter {
    inner: Arc<RotatingFileInner>,
}

#[derive(Clone)]
struct SplitLogWriter {
    info: RotatingFileWriter,
    error: RotatingFileWriter,
}

#[derive(Clone)]
enum SplitWriter {
    Info(RotatingFileWriter),
    Error(RotatingFileWriter),
}

impl SplitLogWriter {
    fn new(info: RotatingFileWriter, error: RotatingFileWriter) -> Self {
        Self { info, error }
    }
}

struct RotatingFileInner {
    path: PathBuf,
    base_name: String,
    max_size: u64,
    max_age: Duration,
    state: Mutex<FileState>,
}

struct FileState {
    file: File,
    created_at: SystemTime,
    size: u64,
}

impl<'a> MakeWriter<'a> for SplitLogWriter {
    type Writer = SplitWriter;

    fn make_writer(&'a self) -> Self::Writer {
        SplitWriter::Info(self.info.clone())
    }

    fn make_writer_for(&'a self, metadata: &Metadata<'_>) -> Self::Writer {
        if *metadata.level() == Level::ERROR {
            SplitWriter::Error(self.error.clone())
        } else {
            SplitWriter::Info(self.info.clone())
        }
    }
}

impl Write for SplitWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            SplitWriter::Info(writer) => writer.write(buf),
            SplitWriter::Error(writer) => writer.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            SplitWriter::Info(writer) => writer.flush(),
            SplitWriter::Error(writer) => writer.flush(),
        }
    }
}

impl RotatingFileWriter {
    fn new(path: PathBuf, max_size: u64, max_age: Duration) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .open(&path)?;
        let metadata = file.metadata()?;
        let created_at = metadata.modified().unwrap_or_else(|_| SystemTime::now());
        let state = FileState {
            file,
            created_at,
            size: metadata.len(),
        };
        let base_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("log")
            .to_string();
        Ok(Self {
            inner: Arc::new(RotatingFileInner {
                path,
                base_name,
                max_size,
                max_age,
                state: Mutex::new(state),
            }),
        })
    }
}

#[derive(Clone)]
struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        let now = Local::now();
        write!(w, "{}", now.format("%Y-%m-%d %H:%M:%S%.3f"))
    }
}

impl RotatingFileInner {
    fn write_bytes(&self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self.state.lock().expect("日志文件锁失效");
        if self.should_rotate(&state, buf.len()) {
            self.rotate(&mut state)?;
        }
        let written = state.file.write(buf)?;
        state.size = state.size.saturating_add(written as u64);
        Ok(written)
    }

    fn flush_writer(&self) -> io::Result<()> {
        let mut state = self.state.lock().expect("日志文件锁失效");
        state.file.flush()
    }

    fn should_rotate(&self, state: &FileState, incoming: usize) -> bool {
        let size_exceeded =
            self.max_size != u64::MAX && state.size.saturating_add(incoming as u64) > self.max_size;
        let age_exceeded = self.max_age != Duration::MAX
            && state.created_at.elapsed().unwrap_or_default() >= self.max_age;
        size_exceeded || age_exceeded
    }

    fn rotate(&self, state: &mut FileState) -> io::Result<()> {
        state.file.flush()?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let rotated_name = format!("{}.{}", self.base_name, timestamp);
        let rotated_path = self
            .path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(rotated_name);
        if self.path.exists() {
            fs::rename(&self.path, &rotated_path)?;
        }
        let new_file = OpenOptions::new()
            .create(true)
            .append(true)
            .write(true)
            .open(&self.path)?;
        state.file = new_file;
        state.size = 0;
        state.created_at = SystemTime::now();
        self.cleanup_old_files()?;
        Ok(())
    }

    fn cleanup_old_files(&self) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            let keep_since = SystemTime::now()
                .checked_sub(self.max_age)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                if path == self.path || !path.is_file() {
                    continue;
                }
                let file_name = match path.file_name().and_then(|name| name.to_str()) {
                    Some(name) => name,
                    None => continue,
                };
                if !file_name.starts_with(&self.base_name) {
                    continue;
                }
                let modified = entry
                    .metadata()?
                    .modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                if modified < keep_since {
                    let _ = fs::remove_file(path);
                }
            }
        }
        Ok(())
    }
}

impl Write for RotatingFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write_bytes(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush_writer()
    }
}

/// 日志初始化相关错误。
#[derive(thiserror::Error, Debug)]
pub enum LogInitError {
    #[error("日志桥接初始化失败: {0}")]
    Bridge(String),
    #[error("日志过滤规则解析失败: {0}")]
    InvalidFilter(String),
    #[error("日志配置文件读取失败: {0}")]
    ConfigFile(String),
    #[error("日志系统初始化失败: {0}")]
    Init(String),
    #[error("日志 IO 操作失败: {0}")]
    Io(String),
}
