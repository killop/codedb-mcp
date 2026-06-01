use crate::bm25::Bm25Index;
use crate::indexer::{IndexOptions, LightweightGraphStats, StorageOptions};
use crate::text_search::{TextSearchIndex, write_text_search_index};
use crate::types::{Chunk, FileEntry, LanguageId, Scope, SemanticUnit, Symbol, WordIndex};
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_VERSION: u32 = 23;
const MANIFEST_FILE: &str = "manifest.json";
const FINGERPRINTS_FILE: &str = "fingerprints.bin";
const PAYLOAD_FILE: &str = "index.bin";
const OUTLINES_FILE: &str = "outlines.bin";
const OUTLINES_INDEX_FILE: &str = "outlines_index.bin";
const BM25_POSTINGS_FILE: &str = "bm25.postings";
const WORD_INDEX_FILE: &str = "word_index.bin";
const WORD_HITS_FILE: &str = "word_hits.bin";
const TEXT_SEARCH_INDEX_FILE: &str = "text_search_index.bin";
const EMBEDDINGS_FILE: &str = "embeddings.bin";
const DEPS_FILE: &str = "deps.bin";
const CALLERS_FILE: &str = "callers.bin";
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
    #[serde(default = "default_payload_file")]
    payload_file: String,
    #[serde(default = "default_outlines_file")]
    outlines_file: String,
    #[serde(default = "default_outlines_index_file")]
    outlines_index_file: String,
    #[serde(default = "default_fingerprints_file")]
    fingerprints_file: String,
    #[serde(default = "default_bm25_postings_file")]
    bm25_postings_file: String,
    #[serde(default = "default_deps_file")]
    deps_file: String,
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

pub struct CacheWriteTransaction {
    payload_file: String,
    outlines_file: String,
    outlines_index_file: String,
    fingerprints_file: String,
    bm25_postings_file: String,
    deps_file: String,
    payload_path: PathBuf,
    outlines_path: PathBuf,
    outlines_index_path: PathBuf,
    fingerprints_path: PathBuf,
    bm25_postings_path: PathBuf,
    deps_path: PathBuf,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedOutlineIndexEntry {
    path: String,
    offset: u64,
    len: u32,
}

fn default_payload_file() -> String {
    PAYLOAD_FILE.to_string()
}

fn default_outlines_file() -> String {
    OUTLINES_FILE.to_string()
}

fn default_outlines_index_file() -> String {
    OUTLINES_INDEX_FILE.to_string()
}

fn default_fingerprints_file() -> String {
    FINGERPRINTS_FILE.to_string()
}

fn default_bm25_postings_file() -> String {
    BM25_POSTINGS_FILE.to_string()
}

fn default_deps_file() -> String {
    DEPS_FILE.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

impl CacheWriteTransaction {
    fn new(dir: &Path) -> Self {
        let generation = format!(
            "{}.{}.{}",
            now_ms(),
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let payload_file = format!("index.{generation}.bin");
        let outlines_file = format!("outlines.{generation}.bin");
        let outlines_index_file = format!("outlines_index.{generation}.bin");
        let fingerprints_file = format!("fingerprints.{generation}.bin");
        let bm25_postings_file = format!("bm25.{generation}.postings");
        let deps_file = format!("deps.{generation}.bin");
        Self {
            payload_path: dir.join(&payload_file),
            outlines_path: dir.join(&outlines_file),
            outlines_index_path: dir.join(&outlines_index_file),
            fingerprints_path: dir.join(&fingerprints_file),
            bm25_postings_path: dir.join(&bm25_postings_file),
            deps_path: dir.join(&deps_file),
            payload_file,
            outlines_file,
            outlines_index_file,
            fingerprints_file,
            bm25_postings_file,
            deps_file,
        }
    }

    pub fn bm25_postings_path(&self) -> &Path {
        &self.bm25_postings_path
    }

    pub fn deps_path(&self) -> &Path {
        &self.deps_path
    }

    pub fn save_deps_forward(
        &self,
        deps_forward: &std::collections::HashMap<String, Vec<String>>,
    ) -> Result<()> {
        write_bin_atomic(&self.deps_path, deps_forward)
    }
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
        let cache = Self {
            enabled: true,
            root: root.to_path_buf(),
            dir,
        };
        cache.recover_interrupted_manifest_replace()?;
        Ok(cache)
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

    pub fn begin_write(&self) -> Result<Option<CacheWriteTransaction>> {
        if !self.enabled {
            return Ok(None);
        }
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("failed to create cache dir {}", self.dir.display()))?;
        Ok(Some(CacheWriteTransaction::new(&self.dir)))
    }

    pub fn word_hits_path(&self) -> PathBuf {
        self.dir.join(WORD_HITS_FILE)
    }

    pub fn word_index_path(&self) -> PathBuf {
        self.dir.join(WORD_INDEX_FILE)
    }

    pub fn text_search_index_path(&self) -> PathBuf {
        self.dir.join(TEXT_SEARCH_INDEX_FILE)
    }

    pub fn embeddings_path(&self) -> PathBuf {
        self.dir.join(EMBEDDINGS_FILE)
    }

    pub fn current_deps_path(&self) -> Result<Option<PathBuf>> {
        if !self.enabled {
            return Ok(None);
        }
        self.recover_interrupted_manifest_replace()?;
        let manifest_path = self.manifest_path();
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let manifest: CacheManifest = read_json(&manifest_path)?;
        let path = self.manifest_file_path(&manifest.deps_file);
        Ok(path.is_file().then_some(path))
    }

    pub fn callers_path(&self) -> PathBuf {
        self.dir.join(CALLERS_FILE)
    }

    fn manifest_path(&self) -> PathBuf {
        self.dir.join(MANIFEST_FILE)
    }

    fn manifest_file_path(&self, file_name: &str) -> PathBuf {
        self.dir.join(file_name)
    }

    fn recover_interrupted_manifest_replace(&self) -> Result<()> {
        recover_interrupted_replace(&self.manifest_path())
    }

    pub fn load(&self, options: &IndexOptions) -> Result<Option<CachedIndexPayload>> {
        if !self.enabled {
            return Ok(None);
        }
        self.recover_interrupted_manifest_replace()?;
        let manifest_path = self.manifest_path();
        if !manifest_path.is_file() {
            return Ok(None);
        }

        let manifest: CacheManifest = read_json(&manifest_path)?;
        if !self.required_files_exist_for(&manifest) {
            return Ok(None);
        }
        let fingerprints_path = self.manifest_file_path(&manifest.fingerprints_file);
        let payload_path = self.manifest_file_path(&manifest.payload_file);
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
        let postings_path = self.manifest_file_path(&manifest.bm25_postings_file);
        if !postings_path.is_file() {
            return Ok(None);
        }
        payload.bm25.use_postings_file(postings_path);
        Ok(Some(payload))
    }

    pub fn load_incremental_base(
        &self,
        options: &IndexOptions,
    ) -> Result<Option<CachedIndexPayload>> {
        if !self.enabled {
            return Ok(None);
        }
        self.recover_interrupted_manifest_replace()?;
        let manifest_path = self.manifest_path();
        if !manifest_path.is_file() {
            return Ok(None);
        }

        let manifest: CacheManifest = read_json(&manifest_path)?;
        if manifest.version != CACHE_VERSION
            || manifest.config_hash != config_hash(options)?
            || manifest.embedding_model != options.embedding_model
            || !self.required_files_exist_for(&manifest)
        {
            return Ok(None);
        }

        let mut payload: CachedIndexPayload =
            read_bin(&self.manifest_file_path(&manifest.payload_file))?;
        if payload.files.len() != manifest.file_count
            || payload.chunks.len() != manifest.chunk_count
            || payload.semantic_units.len() != manifest.semantic_unit_count
            || payload.embedding_dims != manifest.embedding_dims
            || payload.vector_count != manifest.vector_count
        {
            return Ok(None);
        }
        let postings_path = self.manifest_file_path(&manifest.bm25_postings_file);
        if !postings_path.is_file() {
            return Ok(None);
        }
        payload.bm25.use_postings_file(postings_path);
        Ok(Some(payload))
    }

    pub fn load_incremental_deps(
        &self,
        options: &IndexOptions,
    ) -> Result<Option<std::collections::HashMap<String, Vec<String>>>> {
        if !self.enabled {
            return Ok(None);
        }
        self.recover_interrupted_manifest_replace()?;
        let manifest_path = self.manifest_path();
        if !manifest_path.is_file() {
            return Ok(None);
        }

        let manifest: CacheManifest = read_json(&manifest_path)?;
        if manifest.version != CACHE_VERSION
            || manifest.config_hash != config_hash(options)?
            || manifest.embedding_model != options.embedding_model
            || !self.required_files_exist_for(&manifest)
        {
            return Ok(None);
        }

        read_deps_forward(&self.manifest_file_path(&manifest.deps_file)).map(Some)
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

    pub fn load_outline_file(
        &self,
        options: &IndexOptions,
        path: &str,
    ) -> Result<Option<CachedFileEntry>> {
        if !self.enabled {
            return Ok(None);
        }
        self.recover_interrupted_manifest_replace()?;
        let manifest_path = self.manifest_path();
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let manifest: CacheManifest = read_json(&manifest_path)?;
        if manifest.version != CACHE_VERSION
            || manifest.config_hash != config_hash(options)?
            || manifest.embedding_model != options.embedding_model
            || !self.required_files_exist_for(&manifest)
        {
            return Ok(None);
        }
        let normalized = crate::indexer::normalize_rel_path(path);
        let index_entries: Vec<CachedOutlineIndexEntry> =
            read_bin(&self.manifest_file_path(&manifest.outlines_index_file))?;
        let Ok(idx) = index_entries.binary_search_by(|entry| entry.path.as_str().cmp(&normalized))
        else {
            return Ok(None);
        };
        let entry = &index_entries[idx];
        let file = read_outline_record(&self.manifest_file_path(&manifest.outlines_file), entry)?;
        if !source_file_matches_current(
            &self.root,
            &SourceFingerprint::from_cached_file_entry(&file),
            options.max_file_bytes,
        ) {
            return Ok(None);
        }
        Ok(Some(file))
    }

    pub fn load_deps_snapshot(&self, options: &IndexOptions) -> Result<Option<CachedDepsSnapshot>> {
        let Some((manifest, fingerprints)) = self.valid_manifest(options)? else {
            return Ok(None);
        };
        let mut files = fingerprints
            .files
            .into_iter()
            .map(|file| file.path)
            .collect::<Vec<_>>();
        files.sort();
        let deps_forward = read_deps_forward(&self.manifest_file_path(&manifest.deps_file))?;
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
        if !self.enabled {
            return Ok(None);
        }
        self.recover_interrupted_manifest_replace()?;
        let manifest_path = self.manifest_path();
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let manifest: CacheManifest = read_json(&manifest_path)?;
        if !self.required_files_exist_for(&manifest) {
            return Ok(None);
        }
        let fingerprints: CacheFingerprints =
            read_bin(&self.manifest_file_path(&manifest.fingerprints_file))?;
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

    fn required_files_exist_for(&self, manifest: &CacheManifest) -> bool {
        [
            manifest.fingerprints_file.as_str(),
            manifest.payload_file.as_str(),
            manifest.outlines_file.as_str(),
            manifest.outlines_index_file.as_str(),
            manifest.bm25_postings_file.as_str(),
            manifest.deps_file.as_str(),
        ]
        .into_iter()
        .all(|name| self.manifest_file_path(name).is_file())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        transaction: CacheWriteTransaction,
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
            payload_file: transaction.payload_file.clone(),
            outlines_file: transaction.outlines_file.clone(),
            outlines_index_file: transaction.outlines_index_file.clone(),
            fingerprints_file: transaction.fingerprints_file.clone(),
            bm25_postings_file: transaction.bm25_postings_file.clone(),
            deps_file: transaction.deps_file.clone(),
        };
        write_bin_atomic(&transaction.fingerprints_path, &cache_fingerprints)?;
        write_outline_sidecars(
            &transaction.outlines_path,
            &transaction.outlines_index_path,
            files,
        )?;
        bm25.write_postings(&transaction.bm25_postings_path)?;
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
        write_bin_atomic(&transaction.payload_path, &payload)?;
        self.remove_lazy_sidecars();
        write_json_atomic(&self.manifest_path(), &manifest)?;
        self.cleanup_old_generation_files(&manifest);
        Ok(())
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

    pub fn save_text_search_index(&self, index: &TextSearchIndex) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("failed to create cache dir {}", self.dir.display()))?;
        write_text_search_index(&self.text_search_index_path(), index)
    }

    fn cleanup_old_generation_files(&self, keep: &CacheManifest) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let keep = [
            keep.payload_file.as_str(),
            keep.outlines_file.as_str(),
            keep.outlines_index_file.as_str(),
            keep.fingerprints_file.as_str(),
            keep.bm25_postings_file.as_str(),
            keep.deps_file.as_str(),
        ];
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|item| item.to_str()) else {
                continue;
            };
            let generated = (name.starts_with("index.") && name.ends_with(".bin"))
                || (name.starts_with("outlines.") && name.ends_with(".bin"))
                || (name.starts_with("outlines_index.") && name.ends_with(".bin"))
                || (name.starts_with("fingerprints.") && name.ends_with(".bin"))
                || (name.starts_with("deps.") && name.ends_with(".bin"))
                || (name.starts_with("bm25.") && name.ends_with(".postings"));
            if generated && !keep.contains(&name) {
                let _ = fs::remove_file(path);
            }
        }
    }

    fn remove_lazy_sidecars(&self) {
        for file_name in [
            WORD_INDEX_FILE,
            WORD_HITS_FILE,
            TEXT_SEARCH_INDEX_FILE,
            EMBEDDINGS_FILE,
            CALLERS_FILE,
        ] {
            let _ = fs::remove_file(self.dir.join(file_name));
        }
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

    fn from_cached_file_entry(file: &CachedFileEntry) -> Self {
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

fn read_outline_record(path: &Path, entry: &CachedOutlineIndexEntry) -> Result<CachedFileEntry> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    file.seek(SeekFrom::Start(entry.offset))?;
    let mut bytes = vec![0u8; entry.len as usize];
    file.read_exact(&mut bytes)?;
    bincode::deserialize(&bytes).with_context(|| {
        format!(
            "failed to read outline record {} at {}",
            entry.path,
            path.display()
        )
    })
}

fn write_outline_sidecars(
    outlines_path: &Path,
    index_path: &Path,
    files: &[FileEntry],
) -> Result<()> {
    let tmp = unique_tmp_path(outlines_path);
    let file = File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    let mut writer = BufWriter::new(file);
    let mut offset = 0u64;
    let mut index = Vec::with_capacity(files.len());
    for file in files {
        let entry = CachedFileEntryRef::from_file_entry(file);
        let bytes = bincode::serialize(&entry)?;
        writer.write_all(&bytes)?;
        index.push(CachedOutlineIndexEntry {
            path: file.path.clone(),
            offset,
            len: bytes.len() as u32,
        });
        offset = offset.saturating_add(bytes.len() as u64);
    }
    writer.flush()?;
    replace_file(&tmp, outlines_path)?;
    index.sort_by(|a, b| a.path.cmp(&b.path));
    write_bin_atomic(index_path, &index)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = unique_tmp_path(path);
    let file = File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    serde_json::to_writer_pretty(BufWriter::new(file), value)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    replace_file(&tmp, path)
}

fn write_bin_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    let tmp = unique_tmp_path(path);
    let file = File::create(&tmp).with_context(|| format!("failed to create {}", tmp.display()))?;
    bincode::serialize_into(BufWriter::new(file), value)
        .with_context(|| format!("failed to write {}", tmp.display()))?;
    replace_file(&tmp, path)
}

fn replace_file(tmp: &Path, final_path: &Path) -> Result<()> {
    let backup = backup_path(final_path);
    if backup.exists() {
        let _ = fs::remove_file(&backup);
    }
    if final_path.exists() {
        fs::rename(final_path, &backup).with_context(|| {
            format!(
                "failed to stage existing {} as {}",
                final_path.display(),
                backup.display()
            )
        })?;
    }
    match fs::rename(tmp, final_path) {
        Ok(()) => {
            let _ = fs::remove_file(&backup);
            Ok(())
        }
        Err(err) => {
            if backup.exists() && !final_path.exists() {
                let _ = fs::rename(&backup, final_path);
            }
            Err(err).with_context(|| {
                format!(
                    "failed to move {} to {}",
                    tmp.display(),
                    final_path.display()
                )
            })
        }
    }
}

fn recover_interrupted_replace(final_path: &Path) -> Result<()> {
    let backup = backup_path(final_path);
    if final_path.exists() {
        if backup.exists() {
            let _ = fs::remove_file(backup);
        }
        return Ok(());
    }
    if backup.exists() {
        fs::rename(&backup, final_path).with_context(|| {
            format!(
                "failed to restore {} from {}",
                final_path.display(),
                backup.display()
            )
        })?;
    }
    Ok(())
}

fn unique_tmp_path(path: &Path) -> PathBuf {
    let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("cache-file");
    path.with_file_name(format!("{file_name}.{}.{}.tmp", std::process::id(), seq))
}

fn backup_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("cache-file");
    path.with_file_name(format!("{file_name}.bak"))
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
