use crate::bm25::Bm25Index;
use crate::indexer::{IndexOptions, LightweightGraphStats, StorageOptions};
use crate::types::{Chunk, FileEntry, LanguageId, SemanticUnit, Symbol, WordIndex};
use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const CACHE_VERSION: u32 = 14;
const MANIFEST_FILE: &str = "manifest.json";
const PAYLOAD_FILE: &str = "index.bin";
const BM25_POSTINGS_FILE: &str = "bm25.postings";
const EMBEDDINGS_FILE: &str = "embeddings.bin";
const DEPS_FILE: &str = "deps.bin";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub path: String,
    pub byte_size: usize,
    pub modified_unix_ms: i128,
    pub content_hash: String,
}

pub struct ProjectCache {
    enabled: bool,
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
    files: Vec<SourceFingerprint>,
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
    pub word_index: WordIndex,
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
    word_index: &'a WordIndex,
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
    include_paths: &'a [String],
    skip_dirs: &'a [String],
}

impl ProjectCache {
    pub fn new(root: &Path, storage: &StorageOptions) -> Result<Self> {
        if !storage.enabled {
            return Ok(Self {
                enabled: false,
                dir: root.join(&storage.dir),
            });
        }
        let dir = local_storage_dir(root, &storage.dir)?;
        Ok(Self { enabled: true, dir })
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

    pub fn embeddings_path(&self) -> PathBuf {
        self.dir.join(EMBEDDINGS_FILE)
    }

    pub fn deps_path(&self) -> PathBuf {
        self.dir.join(DEPS_FILE)
    }

    pub fn load(
        &self,
        options: &IndexOptions,
        fingerprints: &[SourceFingerprint],
    ) -> Result<Option<CachedIndexPayload>> {
        if !self.enabled {
            return Ok(None);
        }
        let manifest_path = self.dir.join(MANIFEST_FILE);
        let payload_path = self.dir.join(PAYLOAD_FILE);
        let embeddings_path = self.dir.join(EMBEDDINGS_FILE);
        let deps_path = self.dir.join(DEPS_FILE);
        if !manifest_path.is_file()
            || !payload_path.is_file()
            || !embeddings_path.is_file()
            || !deps_path.is_file()
        {
            return Ok(None);
        }

        let manifest: CacheManifest = read_json(&manifest_path)?;
        if manifest.version != CACHE_VERSION
            || manifest.config_hash != config_hash(options)?
            || manifest.embedding_model != options.embedding_model
            || manifest.files != fingerprints
        {
            return Ok(None);
        }

        let mut payload: CachedIndexPayload = read_bin(&payload_path)?;
        if payload.files.len() != manifest.file_count
            || payload.chunks.len() != manifest.chunk_count
            || payload.semantic_units.len() != manifest.semantic_unit_count
            || payload.embedding_dims != manifest.embedding_dims
            || payload.vector_count != manifest.vector_count
            || payload.semantic_units.len() != payload.vector_count
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

    #[allow(clippy::too_many_arguments)]
    pub fn save(
        &self,
        options: &IndexOptions,
        files: &[FileEntry],
        chunks: &[Chunk],
        semantic_units: &[SemanticUnit],
        embeddings: &[Vec<f32>],
        bm25: &Bm25Index,
        word_index: &WordIndex,
        deps_forward: &std::collections::HashMap<String, Vec<String>>,
        graph_stats: LightweightGraphStats,
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
        let manifest = CacheManifest {
            version: CACHE_VERSION,
            created_unix_ms: now_ms(),
            config_hash: config_hash(options)?,
            embedding_model: options.embedding_model.clone(),
            embedding_dims: embedding_dims(embeddings),
            file_count: files.len(),
            chunk_count: chunks.len(),
            semantic_unit_count: semantic_units.len(),
            vector_count: embeddings.len(),
            files: fingerprints,
        };
        bm25.write_postings(&self.dir.join(BM25_POSTINGS_FILE))?;
        write_bin_atomic(&self.dir.join(EMBEDDINGS_FILE), embeddings)?;
        write_bin_atomic(&self.dir.join(DEPS_FILE), deps_forward)?;
        let payload = CachedIndexPayloadRef {
            files: files
                .iter()
                .map(CachedFileEntryRef::from_file_entry)
                .collect(),
            chunks,
            semantic_units,
            embedding_dims: embedding_dims(embeddings),
            vector_count: embeddings.len(),
            graph_stats,
            bm25,
            word_index,
        };
        write_bin_atomic(&self.dir.join(PAYLOAD_FILE), &payload)?;
        write_json_atomic(&self.dir.join(MANIFEST_FILE), &manifest)?;
        Ok(())
    }
}

pub fn read_embeddings(path: &Path) -> Result<Vec<Vec<f32>>> {
    read_bin(path)
}

pub fn read_deps_forward(path: &Path) -> Result<std::collections::HashMap<String, Vec<String>>> {
    read_bin(path)
}

fn embedding_dims(embeddings: &[Vec<f32>]) -> usize {
    embeddings
        .iter()
        .find(|vector| !vector.is_empty())
        .map_or(0, Vec::len)
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
        include_paths: &options.include_paths,
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

fn now_ms() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or(0)
}
