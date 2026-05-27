use crate::indexer::{DiagnosticsOptions, IndexOptions, StorageOptions};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub scan: ScanConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,
    #[serde(default)]
    pub watch: WatchConfig,
    #[serde(default)]
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanConfig {
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,
    #[serde(default = "default_max_file_bytes")]
    pub max_file_bytes: u64,
    #[serde(default = "default_true")]
    pub respect_gitignore: bool,
    #[serde(default = "default_include_paths")]
    pub include_paths: Vec<String>,
    #[serde(default = "default_skip_dirs")]
    pub skip_dirs: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default = "default_embedding_model")]
    pub model: String,
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_storage_dir")]
    pub dir: String,
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
            embedding_model: self.embedding.model.clone(),
            respect_gitignore: self.scan.respect_gitignore,
            include_paths: self
                .scan
                .include_paths
                .iter()
                .map(|item| item.replace('\\', "/").trim_matches('/').to_string())
                .filter(|item| !item.is_empty())
                .collect(),
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

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            extensions: default_extensions(),
            max_file_bytes: default_max_file_bytes(),
            respect_gitignore: true,
            include_paths: default_include_paths(),
            skip_dirs: default_skip_dirs(),
        }
    }
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            model: default_embedding_model(),
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
        Self { enabled: true }
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

fn default_extensions() -> Vec<String> {
    [
        "cs", "java", "rs", "py", "pyw", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h", "cc",
        "cpp", "cxx", "hpp", "hh", "hxx",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_include_paths() -> Vec<String> {
    vec!["Library/PackageCache".to_string()]
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
        "library",
        "temp",
        "logs",
        "obj",
        "bin",
        "build",
        "builds",
        "usersettings",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn default_max_file_bytes() -> u64 {
    50_000_000
}

fn default_embedding_model() -> String {
    default_embedding_model_path()
}

#[cfg(windows)]
fn default_embedding_model_path() -> String {
    if let Some(path) = default_hf_embedding_model_path() {
        return path_to_config_string(&path);
    }
    let drives = (b'C'..=b'Z')
        .filter_map(|letter| {
            let root = format!("{}:/", letter as char);
            Path::new(&root).exists().then_some(letter as char)
        })
        .collect::<Vec<_>>();
    let drive = drives
        .get(1)
        .or_else(|| drives.first())
        .copied()
        .unwrap_or('C');
    format!("{drive}:/codedb-mcp-cache/models/potion-code-16M")
}

#[cfg(windows)]
fn default_hf_embedding_model_path() -> Option<PathBuf> {
    let hub = default_hf_hub_dir()?;
    if let Some(snapshot) = existing_hf_model_snapshot(&hub) {
        return Some(snapshot);
    }
    hub.exists().then(|| {
        hub.join("codedb-mcp")
            .join("models")
            .join("potion-code-16M")
    })
}

#[cfg(windows)]
fn default_hf_hub_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(
        PathBuf::from(home)
            .join(".cache")
            .join("huggingface")
            .join("hub"),
    )
}

#[cfg(windows)]
fn existing_hf_model_snapshot(hub: &Path) -> Option<PathBuf> {
    let repo = hub.join("models--minishlab--potion-code-16M");
    let refs_main = repo.join("refs").join("main");
    if let Ok(commit) = fs::read_to_string(&refs_main) {
        let snapshot = repo.join("snapshots").join(commit.trim());
        if is_model_dir(&snapshot) {
            return Some(snapshot);
        }
    }
    let snapshots = repo.join("snapshots");
    let mut entries = fs::read_dir(snapshots)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && is_model_dir(path))
        .collect::<Vec<_>>();
    entries.sort();
    entries.into_iter().next()
}

#[cfg(windows)]
fn is_model_dir(path: &Path) -> bool {
    path.join("tokenizer.json").exists()
        && path.join("model.safetensors").exists()
        && (path.join("config.json").exists()
            || path.join("config_sentence_transformers.json").exists())
}

#[cfg(windows)]
fn path_to_config_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(not(windows))]
fn default_embedding_model_path() -> String {
    ".codedb-mcp/models/potion-code-16M".to_string()
}

fn default_storage_dir() -> String {
    ".codedb-mcp".to_string()
}

fn default_true() -> bool {
    true
}
