use crate::config::AppConfig;
use crate::event_log;
use crate::tools::{ProjectManager, ReloadCheck};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub fn start_config_watcher(
    manager: Arc<ProjectManager>,
    config_path: PathBuf,
    poll_interval: Duration,
) -> Result<JoinHandle<()>> {
    let poll_interval = poll_interval.max(Duration::from_secs(1));
    thread::Builder::new()
        .name("codebase-mcp-config".to_string())
        .spawn(move || watch_loop(manager, config_path, poll_interval))
        .context("failed to spawn config watcher thread")
}

fn watch_loop(manager: Arc<ProjectManager>, config_path: PathBuf, poll_interval: Duration) {
    let mut last_hash = read_config_hash(&config_path).ok();
    loop {
        thread::sleep(poll_interval);
        let current_hash = match read_config_hash(&config_path) {
            Ok(hash) => hash,
            Err(err) => {
                if last_hash.take().is_some() {
                    let started = Instant::now();
                    event_log::log_config_reload_detected("missing");
                    event_log::log_config_reload_finish_failure(started, &err.to_string());
                    eprintln!("codebase-mcp config reload skipped: {err:#}");
                }
                continue;
            }
        };
        if last_hash.as_deref() == Some(current_hash.as_str()) {
            continue;
        }
        last_hash = Some(current_hash.clone());
        event_log::log_config_reload_detected(&current_hash);
        let started = Instant::now();
        let config = match AppConfig::load(&config_path) {
            Ok(config) => config,
            Err(err) => {
                event_log::log_config_reload_finish_failure(started, &err.to_string());
                eprintln!("codebase-mcp config reload parse failed: {err:#}");
                continue;
            }
        };
        let new_options = config.index_options();
        let reason = if manager.options().cache_identity_eq(&new_options) {
            "config_changed_without_index_scope_change"
        } else {
            "index_scope_changed"
        };
        event_log::log_config_reload_start(reason);
        match manager.reload_options(new_options) {
            Ok(ReloadCheck::Unchanged) => {
                event_log::log_config_reload_finish_unchanged(started);
            }
            Ok(ReloadCheck::Reindexed(index)) => {
                let stats = index.stats();
                event_log::log_config_reload_finish_success(
                    started,
                    stats.files,
                    stats.chunks,
                    stats.symbols,
                    stats.cache,
                );
                eprintln!(
                    "codebase-mcp config reload indexed in {:.3}s: {} files, {} chunks, {} symbols",
                    started.elapsed().as_secs_f32(),
                    stats.files,
                    stats.chunks,
                    stats.symbols
                );
            }
            Err(err) => {
                event_log::log_config_reload_finish_failure(started, &err.to_string());
                eprintln!("codebase-mcp config reload reindex failed: {err:#}");
            }
        }
    }
}

fn read_config_hash(path: &PathBuf) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(blake3::hash(&bytes).to_hex()[..16].to_string())
}
