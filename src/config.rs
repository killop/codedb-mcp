use crate::event_log::EventLogConfig;
use crate::indexer::{DiagnosticsOptions, IndexOptions, StorageOptions};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub scan: ScanConfig,
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,
    #[serde(default)]
    pub watch: WatchConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanConfig {
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_true")]
    pub respect_gitignore: bool,
    #[serde(
        default = "default_root_paths",
        alias = "roots",
        alias = "source_roots"
    )]
    pub root_paths: Vec<String>,
    #[serde(default = "default_include_paths")]
    pub include_paths: Vec<String>,
    #[serde(default = "default_exclude_paths", alias = "exclude_globs")]
    pub exclude_paths: Vec<String>,
    #[serde(default = "default_skip_dirs")]
    pub skip_dirs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiagnosticsConfig {
    #[serde(default)]
    pub timing: bool,
    #[serde(default)]
    pub slow_file_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WatchConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_watch_poll_interval_seconds")]
    pub poll_interval_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_storage_dir")]
    pub dir: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoggingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_log_file")]
    pub file: String,
    #[serde(default = "default_log_queue_capacity")]
    pub queue_capacity: usize,
    #[serde(default = "default_log_flush_interval_ms")]
    pub flush_interval_ms: u64,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse config file {}", path.display()))
    }

    pub fn index_options(&self) -> IndexOptions {
        IndexOptions {
            extensions: self
                .scan
                .extensions
                .iter()
                .flat_map(|item| item.split(','))
                .map(|item| item.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|item| !item.is_empty())
                .collect(),
            max_file_bytes: self.scan.max_file_bytes,
            respect_gitignore: self.scan.respect_gitignore,
            root_paths: normalize_config_paths(&self.scan.root_paths),
            include_paths: self
                .scan
                .include_paths
                .iter()
                .map(|item| item.replace('\\', "/").trim_matches('/').to_string())
                .filter(|item| !item.is_empty())
                .collect(),
            exclude_paths: normalize_config_paths(&self.scan.exclude_paths),
            skip_dirs: self
                .scan
                .skip_dirs
                .iter()
                .map(|item| item.to_ascii_lowercase())
                .collect(),
            diagnostics: DiagnosticsOptions {
                timing: self.diagnostics.timing,
                slow_file_ms: self.diagnostics.slow_file_ms,
            },
            storage: StorageOptions {
                enabled: self.storage.enabled,
                dir: self
                    .storage
                    .dir
                    .replace('\\', "/")
                    .trim_matches('/')
                    .to_string(),
            },
        }
    }
}

impl LoggingConfig {
    pub fn event_log_config(&self) -> EventLogConfig {
        EventLogConfig {
            enabled: self.enabled,
            file: self.file.replace('\\', "/"),
            queue_capacity: self.queue_capacity,
            flush_interval_ms: self.flush_interval_ms,
        }
    }
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            extensions: default_extensions(),
            max_file_bytes: default_max_file_bytes(),
            respect_gitignore: true,
            root_paths: default_root_paths(),
            include_paths: default_include_paths(),
            exclude_paths: default_exclude_paths(),
            skip_dirs: default_skip_dirs(),
        }
    }
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            timing: false,
            slow_file_ms: 0,
        }
    }
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_seconds: default_watch_poll_interval_seconds(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: default_storage_dir(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            file: default_log_file(),
            queue_capacity: default_log_queue_capacity(),
            flush_interval_ms: default_log_flush_interval_ms(),
        }
    }
}

fn default_extensions() -> Vec<String> {
    [
        "cs", "java", "rs", "py", "pyw", "lua", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h",
        "cc", "cpp", "cxx", "hpp", "hh", "hxx",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_root_paths() -> Vec<String> {
    Vec::new()
}

fn default_include_paths() -> Vec<String> {
    Vec::new()
}

fn default_exclude_paths() -> Vec<String> {
    Vec::new()
}

fn default_skip_dirs() -> Vec<String> {
    [
        ".git",
        ".hg",
        ".svn",
        ".vs",
        ".idea",
        ".gradle",
        "node_modules",
        "target",
        "dist",
        ".next",
        ".svelte-kit",
        "coverage",
        "out",
        ".codedb-mcp",
        "temp",
        "logs",
        "obj",
        "bin",
        "build",
        "builds",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn normalize_config_paths(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .map(|item| item.replace('\\', "/").trim_matches('/').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn default_max_file_bytes() -> u64 {
    50_000_000
}

fn default_watch_poll_interval_seconds() -> u64 {
    5
}

fn default_storage_dir() -> String {
    ".codedb-mcp".to_string()
}

fn default_log_file() -> String {
    ".codedb-mcp/codedb-mcp.log".to_string()
}

fn default_log_queue_capacity() -> usize {
    8192
}

fn default_log_flush_interval_ms() -> u64 {
    500
}

fn default_true() -> bool {
    true
}
