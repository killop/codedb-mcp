use anyhow::{Context, Result};
use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

static LOGGER: OnceLock<AsyncEventLogger> = OnceLock::new();

pub struct EventLogConfig {
    pub enabled: bool,
    pub file: String,
    pub queue_capacity: usize,
    pub flush_interval_ms: u64,
}

#[derive(Clone, Copy)]
pub struct ToolLogContext {
    mode: ToolLogMode,
    bundle_index: Option<usize>,
}

#[derive(Clone, Copy)]
enum ToolLogMode {
    Direct,
    Bundle,
}

struct AsyncEventLogger {
    sender: SyncSender<LogMessage>,
    dropped: Arc<AtomicUsize>,
}

enum LogMessage {
    Line(String),
    Flush(SyncSender<()>),
}

pub fn init(root: &Path, config: &EventLogConfig) -> Result<()> {
    if !config.enabled || LOGGER.get().is_some() {
        return Ok(());
    }
    let path = resolve_log_path(root, &config.file);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create log directory {}", parent.display()))?;
    }
    let capacity = config.queue_capacity.max(1);
    let flush_interval = Duration::from_millis(config.flush_interval_ms.max(100));
    let (sender, receiver) = mpsc::sync_channel::<LogMessage>(capacity);
    let dropped = Arc::new(AtomicUsize::new(0));
    let worker_dropped = dropped.clone();
    thread::Builder::new()
        .name("codebase-mcp-log".to_string())
        .spawn(move || {
            if let Err(err) = write_loop(path, receiver, worker_dropped, flush_interval) {
                eprintln!("codebase-mcp tool log stopped: {err:#}");
            }
        })
        .context("failed to spawn tool log thread")?;
    let _ = LOGGER.set(AsyncEventLogger { sender, dropped });
    Ok(())
}

impl ToolLogContext {
    pub fn direct() -> Self {
        Self {
            mode: ToolLogMode::Direct,
            bundle_index: None,
        }
    }

    pub fn bundle(bundle_index: usize) -> Self {
        Self {
            mode: ToolLogMode::Bundle,
            bundle_index: Some(bundle_index),
        }
    }
}

pub fn enabled() -> bool {
    LOGGER.get().is_some()
}

pub fn emit<F>(build: F)
where
    F: FnOnce() -> String,
{
    drop(build);
}

pub fn log_tool_result(name: &str, context: ToolLogContext, start: Instant, output: &str) {
    if !enabled() {
        return;
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    let status = if output.starts_with("error:") {
        "failure"
    } else {
        "success"
    };
    let failure_reason = if status == "failure" {
        format!(
            " failure_reason={}",
            sanitize_log_value(&failure_reason(output))
        )
    } else {
        String::new()
    };
    push_line(|| {
        format!(
            "kind=mcp_tool_call tool={} mode={}{} status={} elapsed_ms={:.3} output_bytes={}{}",
            sanitize_log_value(name),
            context.mode.as_str(),
            context
                .bundle_index
                .map(|index| format!(" bundle_index={index}"))
                .unwrap_or_default(),
            status,
            elapsed_ms,
            output.len(),
            failure_reason
        )
    });
}

pub fn log_tool_failure(name: &str, context: ToolLogContext, start: Instant, reason: &str) {
    if !enabled() {
        return;
    }
    let output = format!("error: {reason}");
    log_tool_result(name, context, start, &output);
}

pub fn log_file_watch_start(roots: &str, poll_interval_ms: u128, extensions: &str) {
    if !enabled() {
        return;
    }
    push_line(|| {
        format!(
            "kind=file_watch state=start roots={} poll_interval_ms={} extensions={}",
            sanitize_log_value(roots),
            poll_interval_ms,
            sanitize_log_value(extensions)
        )
    });
}

pub fn log_file_watch_error(error_type: &str, reason: &str) {
    if !enabled() {
        return;
    }
    push_line(|| {
        format!(
            "kind=file_watch_error error_type={} failure_reason={}",
            sanitize_log_value(error_type),
            sanitize_log_value(reason)
        )
    });
}

pub fn log_file_watch_reconfigure(reason: &str) {
    if !enabled() {
        return;
    }
    push_line(|| {
        format!(
            "kind=file_watch state=reconfigure reason={}",
            sanitize_log_value(reason)
        )
    });
}

pub fn log_file_watch_digest_queued(
    raw_events: usize,
    pending_changed: usize,
    pending_deleted: usize,
) {
    if !enabled() {
        return;
    }
    push_line(|| {
        format!(
            "kind=file_watch_digest phase=queued raw_events={} pending_changed={} pending_deleted={}",
            raw_events, pending_changed, pending_deleted
        )
    });
}

pub fn log_file_watch_digest_start(changed: usize, deleted: usize) {
    if !enabled() {
        return;
    }
    push_line(|| {
        format!(
            "kind=file_watch_digest phase=start changed={} deleted={}",
            changed, deleted
        )
    });
}

pub fn log_file_watch_digest_finish(
    start: Instant,
    changed: usize,
    deleted: usize,
    files: usize,
    chunks: usize,
    symbols: usize,
    cache: &str,
) {
    if !enabled() {
        return;
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    push_line(|| {
        format!(
            "kind=file_watch_digest phase=finish status=success elapsed_ms={:.3} changed={} deleted={} files={} chunks={} symbols={} cache={}",
            elapsed_ms,
            changed,
            deleted,
            files,
            chunks,
            symbols,
            sanitize_log_value(cache)
        )
    });
}

pub fn log_file_watch_digest_unchanged(start: Instant, changed: usize, deleted: usize) {
    if !enabled() {
        return;
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    push_line(|| {
        format!(
            "kind=file_watch_digest phase=finish status=unchanged elapsed_ms={:.3} changed={} deleted={}",
            elapsed_ms, changed, deleted
        )
    });
}

pub fn log_file_watch_digest_failure(start: Instant, changed: usize, deleted: usize, reason: &str) {
    if !enabled() {
        return;
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    push_line(|| {
        format!(
            "kind=file_watch_digest phase=finish status=failure elapsed_ms={:.3} changed={} deleted={} failure_reason={}",
            elapsed_ms,
            changed,
            deleted,
            sanitize_log_value(reason)
        )
    });
}

pub fn log_config_reload_detected(config_hash: &str) {
    if !enabled() {
        return;
    }
    push_line(|| {
        format!(
            "kind=config_reload phase=detected config_hash={}",
            sanitize_log_value(config_hash)
        )
    });
}

pub fn log_config_reload_start(reason: &str) {
    if !enabled() {
        return;
    }
    push_line(|| {
        format!(
            "kind=config_reload phase=start reason={}",
            sanitize_log_value(reason)
        )
    });
}

pub fn log_config_reload_finish_unchanged(start: Instant) {
    if !enabled() {
        return;
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    push_line(|| {
        format!("kind=config_reload phase=finish status=unchanged elapsed_ms={elapsed_ms:.3}")
    });
}

pub fn log_config_reload_finish_success(
    start: Instant,
    files: usize,
    chunks: usize,
    symbols: usize,
    cache: &str,
) {
    if !enabled() {
        return;
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    push_line(|| {
        format!(
            "kind=config_reload phase=finish status=success elapsed_ms={:.3} files={} chunks={} symbols={} cache={}",
            elapsed_ms,
            files,
            chunks,
            symbols,
            sanitize_log_value(cache)
        )
    });
}

pub fn log_config_reload_finish_failure(start: Instant, reason: &str) {
    if !enabled() {
        return;
    }
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    push_line(|| {
        format!(
            "kind=config_reload phase=finish status=failure elapsed_ms={:.3} failure_reason={}",
            elapsed_ms,
            sanitize_log_value(reason)
        )
    });
}

fn push_line<F>(build: F)
where
    F: FnOnce() -> String,
{
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let line = LogMessage::Line(format!("{} {}", timestamp(), build()));
    match logger.sender.try_send(line) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            logger.dropped.fetch_add(1, Ordering::Relaxed);
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

pub fn flush(timeout: Duration) -> bool {
    let Some(logger) = LOGGER.get() else {
        return true;
    };
    let (ack_tx, ack_rx) = mpsc::sync_channel(0);
    if logger.sender.try_send(LogMessage::Flush(ack_tx)).is_err() {
        return false;
    }
    ack_rx.recv_timeout(timeout).is_ok()
}

pub fn timing(scope: &str, stage: &str, start: Instant) {
    let _ = (scope, stage, start);
}

fn resolve_log_path(root: &Path, configured: &str) -> PathBuf {
    let configured = configured.trim();
    if configured.is_empty() {
        return root.join(".codedb-mcp").join("codedb-mcp.log");
    }
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn write_loop(
    path: PathBuf,
    receiver: mpsc::Receiver<LogMessage>,
    dropped: Arc<AtomicUsize>,
    flush_interval: Duration,
) -> Result<()> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open tool log {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    loop {
        match receiver.recv_timeout(flush_interval) {
            Ok(message) => {
                handle_message(&mut writer, &dropped, message)?;
                while let Ok(message) = receiver.try_recv() {
                    handle_message(&mut writer, &dropped, message)?;
                }
                write_dropped(&mut writer, &dropped)?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                write_dropped(&mut writer, &dropped)?;
                writer.flush()?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                write_dropped(&mut writer, &dropped)?;
                writer.flush()?;
                return Ok(());
            }
        }
    }
}

fn handle_message(
    writer: &mut BufWriter<fs::File>,
    dropped: &AtomicUsize,
    message: LogMessage,
) -> Result<()> {
    match message {
        LogMessage::Line(line) => write_line(writer, &line),
        LogMessage::Flush(ack) => {
            write_dropped(writer, dropped)?;
            writer.flush()?;
            let _ = ack.send(());
            Ok(())
        }
    }
}

fn write_line(writer: &mut BufWriter<fs::File>, line: &str) -> Result<()> {
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")?;
    Ok(())
}

fn write_dropped(writer: &mut BufWriter<fs::File>, dropped: &AtomicUsize) -> Result<()> {
    let count = dropped.swap(0, Ordering::Relaxed);
    if count > 0 {
        write_line(
            writer,
            &format!("{} kind=log_dropped count={count}", timestamp()),
        )?;
    }
    Ok(())
}

impl ToolLogMode {
    fn as_str(self) -> &'static str {
        match self {
            ToolLogMode::Direct => "direct",
            ToolLogMode::Bundle => "bundle",
        }
    }
}

fn failure_reason(output: &str) -> String {
    let first_line = output.lines().next().unwrap_or("unknown_error").trim();
    first_line
        .strip_prefix("error:")
        .unwrap_or(first_line)
        .trim()
        .chars()
        .take(240)
        .collect::<String>()
}

fn sanitize_log_value(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        return "empty".to_string();
    }
    value
        .chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' | ' ' => '_',
            '\\' => '/',
            '=' => ':',
            _ => ch,
        })
        .collect()
}

fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
