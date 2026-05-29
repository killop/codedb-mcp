use crate::bm25::Bm25Index;
use crate::indexer::{IndexOptions, LightweightGraphStats, StorageOptions};
use crate::types::{Chunk, FileEntry, LanguageId, Scope, SemanticUnit, Symbol, WordIndex};
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_VERSION: u32 = 20;
const MANIFEST_FILE: &str = "manifest.json";
const FINGERPRINTS_FILE: &str = "fingerprints.bin";
const PAYLOAD_FILE: &str = "index.bin";
const BM25_POSTINGS_FILE: &str = "bm25.postings";
const WORD_INDEX_FILE: &str = "word_index.bin";
const WORD_HITS_FILE: &str = "word_hits.bin";
const EMBEDDINGS_FILE: &str = "embeddings.bin";
const DEPS_FILE: &str = "deps.bin";
const CALLERS_FILE: &str = "callers.bin";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub path: String,
    pub byte_size: usize,
    pub modified_unix_ms: i128,
    pub content_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceDirectoryFingerprint {
    pub path: String,
    pub modified_unix_ms: i128,
}

pub struct ProjectCache {
    enabled: bool,
    root: PathBuf,
    dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheManifest {
    version: u32,
    created_unix_ms: i128,
    config_hash: String,
    embedding_model: String,
    embedding_dims: usize,
    file_count: usize,
    chunk_count: usize,
    semantic_unit_count: usize,
    vector_count: usize,
    #[serde(default)]
    graph_stats: LightweightGraphStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFingerprints {
    files: Vec<SourceFingerprint>,
    dirs: Vec<SourceDirectoryFingerprint>,
}

#[derive(Debug, Clone)]
pub struct CachedStatusSnapshot {
    pub seq: i128,
    pub files: usize,
    pub chunks: usize,
    pub embedding_model: String,
    pub embedding_dims: usize,
    pub vector_count: usize,
    pub graph_stats: LightweightGraphStats,
    pub storage_dir: String,
}

#[derive(Debug, Clone)]
pub struct CachedDepsSnapshot {
    pub files: Vec<String>,
    pub deps_forward: std::collections::HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCallerEntry {
    pub name: String,
    pub path: String,
    pub line_start: usize,
    pub kind: String,
    pub hits: Vec<CachedCallerHit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCallerHit {
    pub path: String,
    pub line: usize,
    pub text: String,
    pub scope: Option<Scope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedCallers {
    source_seq: i128,
    entries: Vec<CachedCallerEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedIndexPayload {
    pub files: Vec<CachedFileEntry>,
    pub chunks: Vec<Chunk>,
    pub semantic_units: Vec<SemanticUnit>,
    pub embedding_dims: usize,
    pub vector_count: usize,
    pub graph_stats: LightweightGraphStats,
    pub bm25: Bm25Index,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CachedFileEntry {
    pub path: String,
    pub language: LanguageId,
    pub line_count: usize,
    pub byte_size: usize,
    pub modified_unix_ms: i128,
    pub content_hash: String,
    pub namespace: Option<String>,
    pub imports: Vec<String>,
    pub symbols: Vec<Symbol>,
}

#[derive(Serialize)]
struct CachedIndexPayloadRef<'a> {
    files: Vec<CachedFileEntryRef<'a>>,
    chunks: &'a [Chunk],
    semantic_units: &'a [SemanticUnit],
    embedding_dims: usize,
    vector_count: usize,
    graph_stats: LightweightGraphStats,
    bm25: &'a Bm25Index,
}

#[derive(Serialize)]
struct CachedFileEntryRef<'a> {
    path: &'a str,
    language: LanguageId,
    line_count: usize,
    byte_size: usize,
    modified_unix_ms: i128,
    content_hash: &'a str,
    namespace: &'a Option<String>,
    imports: &'a [String],
    symbols: &'a [Symbol],
}

#[derive(Serialize)]
struct CacheConfigSignature<'a> {
    extensions: &'a [String],
    max_file_bytes: u64,
    embedding_model: &'a str,
    respect_gitignore: bool,
    root_paths: &'a [String],
    include_paths: &'a [String],
    exclude_paths: &'a [String],
    skip_dirs: &'a [String],
}

impl ProjectCache {
    pub fn new(root: &Path, storage: &StorageOptions) -> Result<Self> {
        if !storage.enabled {
            return Ok(Self {
                enabled: false,
                root: root.to_path_buf(),
                dir: root.join(&storage.dir),
            });
        }
        let dir = local_storage_dir(root, &storage.dir)?;
        Ok(Self {
            enabled: true,
            root: root.to_path_buf(),
            dir,
        })
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn bm25_postings_path(&self) -> PathBuf {
        self.dir.join(BM25_POSTINGS_FILE)
    }

    pub fn word_hits_path(&self) -> PathBuf {
        self.dir.join(WORD_HITS_FILE)
    }

    pub fn word_index_path(&self) -> PathBuf {
        self.dir.join(WORD_INDEX_FILE)
    }

    pub fn embeddings_path(&self) -> PathBuf {
        self.dir.join(EMBEDDINGS_FILE)
    }

    pub fn deps_path(&self) -> PathBuf {
        self.dir.join(DEPS_FILE)
    }

    pub fn callers_path(&self) -> PathBuf {
        self.dir.join(CALLERS_FILE)
    }

    pub fn load(&self, options: &IndexOptions) -> Result<Option<CachedIndexPayload>> {
        if !self.enabled {
            return Ok(None);
        }
        let manifest_path = self.dir.join(MANIFEST_FILE);
        let fingerprints_path = self.dir.join(FINGERPRINTS_FILE);
        let payload_path = self.dir.join(PAYLOAD_FILE);
        let deps_path = self.dir.join(DEPS_FILE);
        if !manifest_path.is_file()
            || !fingerprints_path.is_file()
            || !payload_path.is_file()
            || !deps_path.is_file()
        {
            return Ok(None);
        }

        let manifest: CacheManifest = read_json(&manifest_path)?;
        let fingerprints: CacheFingerprints = read_bin(&fingerprints_path)?;
        if manifest.version != CACHE_VERSION
            || manifest.config_hash != config_hash(options)?
            || manifest.embedding_model != options.embedding_model
            || fingerprints.files.len() != manifest.file_count
            || !source_files_match_current(&self.root, &fingerprints.files, options.max_file_bytes)
            || !source_dirs_match_current(&self.root, &fingerprints.dirs)
        {
            return Ok(None);
        }

        let mut payload: CachedIndexPayload = read_bin(&payload_path)?;
        if payload.files.len() != manifest.file_count
            || payload.chunks.len() != manifest.chunk_count
            || payload.semantic_units.len() != manifest.semantic_unit_count
            || payload.embedding_dims != manifest.embedding_dims
            || payload.vector_count != manifest.vector_count
        {
            return Ok(None);
        }
        let postings_path = self.dir.join(BM25_POSTINGS_FILE);
        if !postings_path.is_file() {
            return Ok(None);
        }
        payload.bm25.use_postings_file(postings_path);
        Ok(Some(payload))
    }

    pub fn load_status(&self, options: &IndexOptions) -> Result<Option<CachedStatusSnapshot>> {
        let Some((manifest, _fingerprints)) = self.valid_manifest(options)? else {
            return Ok(None);
        };
        Ok(Some(CachedStatusSnapshot {
            seq: manifest.created_unix_ms,
            files: manifest.file_count,
            chunks: manifest.chunk_count,
            embedding_model: manifest.embedding_model,
            embedding_dims: manifest.embedding_dims,
            vector_count: manifest.vector_count,
            graph_stats: manifest.graph_stats,
            storage_dir: self.dir.display().to_string(),
        }))
    }

    pub fn load_file_list(&self, options: &IndexOptions) -> Result<Option<Vec<String>>> {
        let Some((_manifest, fingerprints)) = self.valid_manifest(options)? else {
            return Ok(None);
        };
        Ok(Some(
            fingerprints
                .files
                .into_iter()
                .map(|file| file.path)
                .collect(),
        ))
    }

    pub fn load_deps_snapshot(&self, options: &IndexOptions) -> Result<Option<CachedDepsSnapshot>> {
        let Some((_manifest, fingerprints)) = self.valid_manifest(options)? else {
            return Ok(None);
        };
        let mut files = fingerprints
            .files
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>();
        files.sort();
        let deps_forward = read_deps_forward(&self.dir.join(DEPS_FILE))?;
        Ok(Some(CachedDepsSnapshot {
            files,
            deps_forward,
        }))
    }

    pub fn load_caller_entry(
        &self,
        options: &IndexOptions,
        name: &str,
        path: &str,
        line_start: usize,
    ) -> Result<Option<CachedCallerEntry>> {
        let Some((manifest, _fingerprints)) = self.valid_manifest(options)? else {
            return Ok(None);
        };
        let callers_path = self.callers_path();
        if !callers_path.is_file() {
            return Ok(None);
        }
        let callers: CachedCallers = read_bin(&callers_path)?;
        if callers.source_seq != manifest.created_unix_ms {
            return Ok(None);
        }
        Ok(callers.entries.into_iter().find(|entry| {
            entry.name == name && entry.path == path && entry.line_start == line_start
        }))
    }

    pub fn save_caller_entry(&self, entry: CachedCallerEntry) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let manifest_path = self.dir.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            return Ok(());
        }
        let manifest: CacheManifest = read_json(&manifest_path)?;
        let callers_path = self.callers_path();
        let mut callers = if callers_path.is_file() {
            read_bin(&callers_path).unwrap_or(CachedCallers {
                source_seq: manifest.created_unix_ms,
                entries: Vec::new(),
            })
        } else {
            CachedCallers {
                source_seq: manifest.created_unix_ms,
                entries: Vec::new(),
            }
        };
        if callers.source_seq != manifest.created_unix_ms {
            callers.source_seq = manifest.created_unix_ms;
            callers.entries.clear();
        }
        callers.entries.retain(|current| {
            !(current.name == entry.name
                && current.path == entry.path
                && current.line_start == entry.line_start)
        });
        callers.entries.push(entry);
        callers.entries.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.line_start.cmp(&b.line_start))
        });
        write_bin_atomic(&callers_path, &callers)
    }

    fn valid_manifest(
        &self,
        options: &IndexOptions,
    ) -> Result<Option<(CacheManifest, CacheFingerprints)>> {
        if !self.enabled || !self.required_files_exist() {
            return Ok(None);
        }
        let manifest: CacheManifest = read_json(&self.dir.join(MANIFEST_FILE))?;
        let fingerprints: CacheFingerprints = read_bin(&self.dir.join(FINGERPRINTS_FILE))?;
        if manifest.version != CACHE_VERSION
            || manifest.config_hash != config_hash(options)?
            || manifest.embedding_model != options.embedding_model
            || fingerprints.files.len() != manifest.file_count
            || !source_files_match_current(&self.root, &fingerprints.files, options.max_file_bytes)
            || !source_dirs_match_current(&self.root, &fingerprints.dirs)
        {
            return Ok(None);
        }
        Ok(Some((manifest, fingerprints)))
    }

    fn required_files_exist(&self) -> bool {
        [
            MANIFEST_FILE,
            FINGERPRINTS_FILE,
            PAYLOAD_FILE,
            BM25_POSTINGS_FILE,
            DEPS_FILE,
        ]
        .into_iter()
        .all(|name| self.dir.join(name).is_file())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        options: &IndexOptions,
        files: &[FileEntry],
        chunks: &[Chunk],
        semantic_units: &[SemanticUnit],
        bm25: &Bm25Index,
        graph_stats: LightweightGraphStats,
        embedding_dims: usize,
        vector_count: usize,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("failed to create cache dir {}", self.dir.display()))?;
        let fingerprints = files
            .iter()
            .map(SourceFingerprint::from_file_entry)
            .collect::<Vec<_>>();
        let dirs = directory_fingerprints_from_files(&self.root, files)?;
        let cache_fingerprints = CacheFingerprints {
            files: fingerprints,
            dirs,
        };
        let manifest = CacheManifest {
            version: CACHE_VERSION,
            created_unix_ms: now_ms(),
            config_hash: config_hash(options)?,
            embedding_model: options.embedding_model.clone(),
            embedding_dims,
            file_count: files.len(),
            chunk_count: chunks.len(),
            semantic_unit_count: semantic_units.len(),
            vector_count,
            graph_stats,
        };
        write_bin_atomic(&self.dir.join(FINGERPRINTS_FILE), &cache_fingerprints)?;
        bm25.write_postings(&self.dir.join(BM25_POSTINGS_FILE))?;
        let payload = CachedIndexPayloadRef {
            files: files
                .iter()
                .map(CachedFileEntryRef::from_file_entry)
                .collect(),
            chunks,
            semantic_units,
            embedding_dims,
            vector_count,
            graph_stats,
            bm25,
        };
        write_bin_atomic(&self.dir.join(PAYLOAD_FILE), &payload)?;
        write_json_atomic(&self.dir.join(MANIFEST_FILE), &manifest)?;
        Ok(())
    }

    pub fn save_deps_forward(
        &self,
        deps_forward: &std::collections::HashMap<String, Vec<String>>,
    ) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("failed to create cache dir {}", self.dir.display()))?;
        write_bin_atomic(&self.dir.join(DEPS_FILE), deps_forward)
    }

    pub fn save_word_index(&self, word_index: &mut WordIndex) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("failed to create cache dir {}", self.dir.display()))?;
        let hits_path = self.word_hits_path();
        word_index.write_hits(&hits_path)?;
        word_index.use_hits_file(hits_path);
        write_bin_atomic(&self.word_index_path(), word_index)
    }
}

pub fn read_embeddings(path: &Path) -> Result<Vec<Vec<f32>>> {
    read_bin(path)
}

pub fn read_deps_forward(path: &Path) -> Result<std::collections::HashMap<String, Vec<String>>> {
    read_bin(path)
}

pub fn read_word_index(path: &Path, hits_path: &Path) -> Result<WordIndex> {
    let mut index: WordIndex = read_bin(path)?;
    index.use_hits_file(hits_path.to_path_buf());
    Ok(index)
}

impl SourceFingerprint {
    pub fn from_file_entry(file: &FileEntry) -> Self {
        Self {
            path: file.path.clone(),
            byte_size: file.byte_size,
            modified_unix_ms: file.modified_unix_ms,
            content_hash: file.content_hash.clone(),
        }
    }
}

impl CachedFileEntry {
    pub fn into_file_entry(self) -> FileEntry {
        FileEntry {
            path: self.path,
            language: self.language,
            line_count: self.line_count,
            byte_size: self.byte_size,
            modified_unix_ms: self.modified_unix_ms,
            content_hash: self.content_hash,
            namespace: self.namespace,
            imports: self.imports,
            symbols: self.symbols,
            content: String::new(),
        }
    }
}

fn directory_fingerprints_from_files(
    root: &Path,
    files: &[FileEntry],
) -> Result<Vec<SourceDirectoryFingerprint>> {
    let mut dirs = std::collections::BTreeSet::new();
    dirs.insert(String::new());
    for file in files {
        let mut current = Path::new(&file.path).parent();
        while let Some(dir) = current {
            let value = dir.to_string_lossy().replace('\\', "/");
            dirs.insert(value);
            current = dir.parent();
        }
    }
    dirs.into_iter()
        .map(|path| {
            let absolute = if path.is_empty() {
                root.to_path_buf()
            } else {
                root.join(&path)
            };
            let metadata = fs::metadata(&absolute)
                .with_context(|| format!("failed to stat directory {}", absolute.display()))?;
            Ok(SourceDirectoryFingerprint {
                path,
                modified_unix_ms: modified_unix_ms(&metadata),
            })
        })
        .collect()
}

fn source_files_match_current(
    root: &Path,
    cached: &[SourceFingerprint],
    max_file_bytes: u64,
) -> bool {
    cached
        .par_iter()
        .all(|fingerprint| source_file_matches_current(root, fingerprint, max_file_bytes))
}

fn source_file_matches_current(
    root: &Path,
    fingerprint: &SourceFingerprint,
    max_file_bytes: u64,
) -> bool {
    let path = root.join(&fingerprint.path);
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file()
        && metadata.len() <= max_file_bytes
        && metadata.len() as usize == fingerprint.byte_size
        && modified_unix_ms(&metadata) == fingerprint.modified_unix_ms
}

fn source_dirs_match_current(root: &Path, cached: &[SourceDirectoryFingerprint]) -> bool {
    !cached.is_empty()
        && cached
            .par_iter()
            .all(|fingerprint| source_dir_matches_current(root, fingerprint))
}

fn source_dir_matches_current(root: &Path, fingerprint: &SourceDirectoryFingerprint) -> bool {
    let path = if fingerprint.path.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&fingerprint.path)
    };
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_dir() && modified_unix_ms(&metadata) == fingerprint.modified_unix_ms
}

impl<'a> CachedFileEntryRef<'a> {
    fn from_file_entry(file: &'a FileEntry) -> Self {
        Self {
            path: &file.path,
            language: file.language,
            line_count: file.line_count,
            byte_size: file.byte_size,
            modified_unix_ms: file.modified_unix_ms,
            content_hash: &file.content_hash,
            namespace: &file.namespace,
            imports: &file.imports,
            symbols: &file.symbols,
        }
    }
}

fn local_storage_dir(root: &Path, configured: &str) -> Result<PathBuf> {
    let configured = configured.trim();
    if configured.is_empty() {
        return Err(anyhow!("storage.dir cannot be empty"));
    }
    let path = Path::new(configured);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(anyhow!(
            "storage.dir must be relative to the project root: {configured}"
        ));
    }
    Ok(root.join(path))
}

fn config_hash(options: &IndexOptions) -> Result<String> {
    let signature = CacheConfigSignature {
        extensions: &options.extensions,
        max_file_bytes: options.max_file_bytes,
        embedding_model: &options.embedding_model,
        respect_gitignore: options.respect_gitignore,
        root_paths: &options.root_paths,
        include_paths: &options.include_paths,
        exclude_paths: &options.exclude_paths,
        skip_dirs: &options.skip_dirs,
    };
    let bytes = serde_json::to_vec(&signature)?;
    Ok(blake3::hash(&bytes).to_hex()[..16].to_string())
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("failed to read {}", path.display()))
}

fn read_bin<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    bincode::deserialize_from(BufReader::new(file))
        .with_context(|| format!("failed to read {}", path.display()))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    let file = File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), value)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    replace_file(&tmp, path)
}

fn write_bin_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("bin.tmp");
    let file = File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    bincode::serialize_into(BufWriter::new(file), value)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    replace_file(&tmp, path)
}

fn replace_file(tmp: &Path, final_path: &Path) -> Result<()> {
    if final_path.exists() {
        fs::remove_file(final_path)
            .with_context(|| format!("failed to replace {}", final_path.display()))?;
    }
    fs::rename(tmp, final_path).with_context(|| {
        format!(
            "failed to move {} to {}",
            tmp.display(),
            final_path.display()
        )
    })
}

fn modified_unix_ms(metadata: &fs::Metadata) -> i128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or(0)
}

fn now_ms() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or(0)
}
