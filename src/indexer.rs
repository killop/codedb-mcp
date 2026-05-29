use crate::bm25::{Bm25Builder, Bm25Index, SpillingBm25Builder};
use crate::cache::{
    CachedIndexPayload, ProjectCache, SourceFingerprint, read_deps_forward, read_embeddings,
    read_word_index,
};
use crate::embedding::MinishEmbeddingModel;
use crate::graph::CodeGraph;
use crate::language::{analyze_source, chunk_source_metadata, language_for_extension};
use crate::tokens::{raw_identifiers, split_identifier};
use crate::types::{Chunk, FileEntry, SearchHit, SemanticUnit, Symbol, WordHit, WordIndex};
use crate::vector_store::MinishVectorStore;
use anyhow::{Context, Result, anyhow};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};
use rayon::prelude::*;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
use crate::language::{analyze_symbols, chunk_source, parse_imports, parse_namespace};

const DEFAULT_MAX_FILE_BYTES: u64 = 50_000_000;

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub extensions: Vec<String>,
    pub max_file_bytes: u64,
    pub embedding_model: String,
    pub respect_gitignore: bool,
    pub root_paths: Vec<String>,
    pub include_paths: Vec<String>,
    pub exclude_paths: Vec<String>,
    pub skip_dirs: Vec<String>,
    pub diagnostics: DiagnosticsOptions,
    pub storage: StorageOptions,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsOptions {
    pub timing: bool,
    pub slow_file_ms: u64,
}

#[derive(Debug, Clone)]
pub struct StorageOptions {
    pub enabled: bool,
    pub dir: String,
}

fn default_source_extensions() -> Vec<String> {
    [
        "cs", "java", "rs", "py", "pyw", "lua", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h",
        "cc", "cpp", "cxx", "hpp", "hh", "hxx",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

impl Default for IndexOptions {
    fn default() -> Self {
        Self {
            extensions: default_source_extensions(),
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            embedding_model: default_embedding_model_path(),
            respect_gitignore: true,
            root_paths: Vec::new(),
            include_paths: vec!["Library/PackageCache".to_string()],
            exclude_paths: Vec::new(),
            skip_dirs: vec![
                ".git".to_string(),
                ".hg".to_string(),
                ".svn".to_string(),
                ".vs".to_string(),
                ".idea".to_string(),
                ".gradle".to_string(),
                "node_modules".to_string(),
                "target".to_string(),
                "dist".to_string(),
                ".next".to_string(),
                ".svelte-kit".to_string(),
                "coverage".to_string(),
                "out".to_string(),
                ".codedb-mcp".to_string(),
                "library".to_string(),
                "temp".to_string(),
                "logs".to_string(),
                "obj".to_string(),
                "bin".to_string(),
                "build".to_string(),
                "builds".to_string(),
                "usersettings".to_string(),
            ],
            diagnostics: DiagnosticsOptions {
                timing: false,
                slow_file_ms: 0,
            },
            storage: StorageOptions {
                enabled: true,
                dir: ".codedb-mcp".to_string(),
            },
        }
    }
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

#[derive(Debug, Clone, Serialize)]
pub struct IndexStats {
    pub root: String,
    pub files: usize,
    pub chunks: usize,
    pub symbols: usize,
    pub seq: u64,
    pub scan: &'static str,
    pub extensions: Vec<String>,
    pub graph_nodes: usize,
    pub graph_edges: usize,
    pub graph_communities: usize,
    pub embedding_model: String,
    pub embedding_dims: usize,
    pub vector_count: usize,
    pub vector_units: &'static str,
    pub storage_dir: Option<String>,
    pub cache: &'static str,
}

#[derive(Debug, Clone, Copy, Default, Serialize, serde::Deserialize)]
pub struct LightweightGraphStats {
    pub nodes: usize,
    pub edges: usize,
    pub communities: usize,
}

pub struct Codebase {
    pub root: PathBuf,
    pub options: IndexOptions,
    pub seq: u64,
    pub file_paths: Vec<String>,
    pub files: BTreeMap<String, FileEntry>,
    pub chunks: Vec<Chunk>,
    pub semantic_units: Vec<SemanticUnit>,
    pub chunk_indices_by_file: HashMap<String, Vec<usize>>,
    pub symbol_definition_chunks: HashMap<String, Vec<usize>>,
    pub word_index: parking_lot::RwLock<Option<WordIndex>>,
    pub word_index_path: Option<PathBuf>,
    pub word_hits_path: Option<PathBuf>,
    pub deps_forward: parking_lot::RwLock<Option<HashMap<String, Vec<String>>>>,
    pub deps_path: Option<PathBuf>,
    pub deps_reverse: parking_lot::RwLock<Option<HashMap<String, Vec<String>>>>,
    pub graph_stats: LightweightGraphStats,
    pub graph: parking_lot::RwLock<Option<Arc<CodeGraph>>>,
    pub bm25: Bm25Index,
    pub embeddings: parking_lot::RwLock<Option<Vec<Vec<f32>>>>,
    pub embeddings_path: Option<PathBuf>,
    pub vectors: parking_lot::RwLock<Option<Arc<MinishVectorStore>>>,
    pub model: parking_lot::RwLock<Option<Arc<MinishEmbeddingModel>>>,
    pub embedding_model_id: String,
    pub embedding_dims: usize,
    pub vector_count: usize,
    pub changed_files: Vec<ChangedFile>,
    pub storage_dir: Option<String>,
    pub cache_status: &'static str,
    pub louvain_communities: parking_lot::RwLock<Option<Vec<crate::graph::GraphCommunity>>>,
    pub louvain_subcommunities:
        parking_lot::RwLock<HashMap<usize, Vec<crate::graph::GraphCommunity>>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub op: &'static str,
    pub size: usize,
}

impl Codebase {
    pub fn index(root: impl AsRef<Path>, options: IndexOptions) -> Result<Self> {
        let timing = options.diagnostics.timing;
        let total_start = Instant::now();
        let root = root.as_ref().canonicalize().with_context(|| {
            format!("failed to resolve project root {}", root.as_ref().display())
        })?;
        if !root.is_dir() {
            return Err(anyhow!(
                "project root is not a directory: {}",
                root.display()
            ));
        }

        let project_cache = ProjectCache::new(&root, &options.storage)?;
        let storage_dir = project_cache
            .enabled()
            .then(|| project_cache.dir().display().to_string());
        if project_cache.enabled() {
            let stage = Instant::now();
            match project_cache.load(&options) {
                Ok(Some(payload)) => {
                    log_timing(timing, "load_project_cache", stage);
                    let embeddings_path = project_cache.embeddings_path();
                    return Self::from_cached(
                        root,
                        options,
                        payload,
                        storage_dir,
                        Some(project_cache.word_index_path()),
                        Some(project_cache.word_hits_path()),
                        embeddings_path.is_file().then_some(embeddings_path),
                        Some(project_cache.deps_path()),
                        total_start,
                    );
                }
                Ok(None) => {
                    log_timing(timing, "load_project_cache_miss", stage);
                }
                Err(err) => {
                    eprintln!(
                        "codebase-mcp cache ignored at {}: {err:#}",
                        project_cache.dir().display()
                    );
                    log_timing(timing, "load_project_cache_error", stage);
                }
            }
        }

        let stage = Instant::now();
        let paths = collect_paths(&root, &options)?;
        log_timing(timing, "collect_paths", stage);

        let stage = Instant::now();
        let mut indexed_files: Vec<IndexedFileSource> = paths
            .par_iter()
            .filter_map(|path| {
                read_indexed_file_source(
                    &root,
                    path,
                    options.max_file_bytes,
                    options.diagnostics.slow_file_ms,
                )
                .ok()
            })
            .collect();
        indexed_files.sort_by(|a, b| a.file.path.cmp(&b.file.path));
        let mut files = Vec::with_capacity(indexed_files.len());
        let mut chunks = Vec::new();
        for mut indexed in indexed_files {
            chunks.append(&mut indexed.chunks);
            files.push(indexed.file);
        }
        let file_paths = files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        drop(paths);
        log_timing(timing, "read_parse_files", stage);

        let stage = Instant::now();
        for (id, chunk) in chunks.iter_mut().enumerate() {
            chunk.id = id;
        }
        assign_chunk_file_ids(&mut chunks, &file_paths);
        let chunk_indices_by_file = build_chunk_indices_by_file(&chunks, &file_paths);
        let symbol_definition_chunks =
            build_symbol_definition_chunks(&files, &chunks, &chunk_indices_by_file);
        log_timing(timing, "chunk_files", stage);

        let stage = Instant::now();
        let dependency_symbols = build_dependency_symbols(&files);
        let dependency_identifiers =
            build_dependency_references_from_sources(&root, &files, &dependency_symbols);
        log_timing(timing, "dependency_references", stage);

        let stage = Instant::now();
        let (mut deps_forward, _deps_reverse) = build_dependencies(
            Some(&root),
            &files,
            &chunks,
            &chunk_indices_by_file,
            &dependency_symbols,
            &dependency_identifiers,
        );
        drop(dependency_symbols);
        drop(dependency_identifiers);
        log_timing(timing, "dependencies", stage);

        let graph_stats = estimate_graph_stats(&files, &deps_forward);
        let mut deps_path = None;
        if project_cache.enabled() {
            let stage = Instant::now();
            match project_cache.save_deps_forward(&deps_forward) {
                Ok(()) => {
                    deps_path = Some(project_cache.deps_path());
                    deps_forward.clear();
                    deps_forward.shrink_to_fit();
                }
                Err(err) => eprintln!(
                    "codebase-mcp dependency sidecar save failed at {}: {err:#}",
                    project_cache.deps_path().display()
                ),
            }
            log_timing(timing, "save_deps_sidecar", stage);
        }

        let stage = Instant::now();
        if project_cache.enabled() {
            fs::create_dir_all(project_cache.dir()).with_context(|| {
                format!(
                    "failed to create cache dir {}",
                    project_cache.dir().display()
                )
            })?;
        }
        let mut bm25 = if project_cache.enabled() {
            let mut builder = SpillingBm25Builder::new(project_cache.dir().join("bm25-build"))?;
            add_bm25_documents_from_sources(
                &root,
                &chunks,
                &file_paths,
                &chunk_indices_by_file,
                |document| builder.add_document(document),
            )?;
            builder.finish_to_postings_file(&project_cache.bm25_postings_path())?
        } else {
            let mut bm25_builder = Bm25Builder::new();
            add_bm25_documents_from_sources(
                &root,
                &chunks,
                &file_paths,
                &chunk_indices_by_file,
                |document| {
                    bm25_builder.add_document(document);
                    Ok(())
                },
            )?;
            bm25_builder.finish()
        };
        if project_cache.enabled() {
            bm25.use_postings_file(project_cache.bm25_postings_path());
        }
        strip_chunk_contents(&mut chunks);
        log_timing(timing, "bm25", stage);

        let stage = Instant::now();
        let mut semantic_units = files
            .iter()
            .enumerate()
            .map(|(id, file)| SemanticUnit {
                id,
                file_path: file.path.clone(),
                text: semantic_text_for_file(file),
            })
            .collect::<Vec<_>>();
        for (id, unit) in semantic_units.iter_mut().enumerate() {
            unit.id = id;
        }
        log_timing(timing, "semantic_units", stage);

        let embedding_model_id = options.embedding_model.clone();
        let embedding_dims = 0;
        let vector_count = semantic_units.len();
        let embeddings = Vec::<Vec<f32>>::new();
        let embeddings_path = None;
        let mut word_index_path = None;
        let mut word_hits_path = None;
        strip_semantic_unit_text(&mut semantic_units);
        strip_chunk_contents(&mut chunks);
        strip_chunk_paths(&mut chunks);
        if project_cache.enabled() && deps_path.is_some() {
            let stage = Instant::now();
            let save_result = project_cache.save(
                &options,
                &files,
                &chunks,
                &semantic_units,
                &bm25,
                graph_stats,
                embedding_dims,
                vector_count,
            );
            if let Err(err) = save_result {
                eprintln!(
                    "codebase-mcp cache save failed at {}: {err:#}",
                    project_cache.dir().display()
                );
            } else {
                bm25.use_postings_file(project_cache.bm25_postings_path());
            }
            log_timing(timing, "save_project_cache", stage);
        }
        if project_cache.enabled() {
            word_index_path = Some(project_cache.word_index_path());
            word_hits_path = Some(project_cache.word_hits_path());
        }
        let mut file_map = files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        log_timing(timing, "total", total_start);
        strip_file_contents(&mut file_map);

        Ok(Self {
            root,
            options,
            seq: now_ms() as u64,
            file_paths,
            files: file_map,
            chunks,
            semantic_units,
            chunk_indices_by_file,
            symbol_definition_chunks,
            word_index: parking_lot::RwLock::new(None),
            word_index_path,
            word_hits_path,
            deps_forward: parking_lot::RwLock::new(if deps_path.is_some() {
                None
            } else {
                Some(deps_forward)
            }),
            deps_path,
            deps_reverse: parking_lot::RwLock::new(None),
            graph_stats,
            graph: parking_lot::RwLock::new(None),
            bm25,
            embeddings: parking_lot::RwLock::new(if embeddings_path.is_some() {
                None
            } else if embeddings.is_empty() {
                None
            } else {
                Some(embeddings)
            }),
            embeddings_path,
            vectors: parking_lot::RwLock::new(None),
            model: parking_lot::RwLock::new(None),
            embedding_model_id,
            embedding_dims,
            vector_count,
            changed_files: Vec::new(),
            storage_dir,
            cache_status: if project_cache.enabled() {
                "miss"
            } else {
                "disabled"
            },
            louvain_communities: parking_lot::RwLock::new(None),
            louvain_subcommunities: parking_lot::RwLock::new(HashMap::new()),
        })
    }

    fn from_cached(
        root: PathBuf,
        options: IndexOptions,
        payload: CachedIndexPayload,
        storage_dir: Option<String>,
        word_index_path: Option<PathBuf>,
        word_hits_path: Option<PathBuf>,
        embeddings_path: Option<PathBuf>,
        deps_path: Option<PathBuf>,
        total_start: Instant,
    ) -> Result<Self> {
        let timing = options.diagnostics.timing;
        let stage = Instant::now();
        let embedding_dims = payload.embedding_dims;
        let vector_count = payload.vector_count;
        let files = payload
            .files
            .into_iter()
            .map(|file| file.into_file_entry())
            .collect::<Vec<_>>();
        let file_paths = files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        let mut chunks = payload.chunks;
        assign_chunk_file_ids(&mut chunks, &file_paths);
        let chunk_indices_by_file = build_chunk_indices_by_file(&chunks, &file_paths);
        strip_chunk_paths(&mut chunks);
        let symbol_definition_chunks =
            build_symbol_definition_chunks(&files, &chunks, &chunk_indices_by_file);
        log_timing(timing, "restore_cached_files", stage);

        let bm25 = payload.bm25;
        let embedding_model_id = options.embedding_model.clone();
        let mut file_map = files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let graph_stats = payload.graph_stats;
        log_timing(timing, "total", total_start);
        strip_file_contents(&mut file_map);

        Ok(Self {
            root,
            options,
            seq: now_ms() as u64,
            file_paths,
            files: file_map,
            chunks,
            semantic_units: payload.semantic_units,
            chunk_indices_by_file,
            symbol_definition_chunks,
            word_index: parking_lot::RwLock::new(None),
            word_index_path,
            word_hits_path,
            deps_forward: parking_lot::RwLock::new(None),
            deps_path,
            deps_reverse: parking_lot::RwLock::new(None),
            graph_stats,
            graph: parking_lot::RwLock::new(None),
            bm25,
            embeddings: parking_lot::RwLock::new(None),
            embeddings_path,
            vectors: parking_lot::RwLock::new(None),
            model: parking_lot::RwLock::new(None),
            embedding_model_id,
            embedding_dims,
            vector_count,
            changed_files: Vec::new(),
            storage_dir,
            cache_status: "hit",
            louvain_communities: parking_lot::RwLock::new(None),
            louvain_subcommunities: parking_lot::RwLock::new(HashMap::new()),
        })
    }

    pub fn stats(&self) -> IndexStats {
        IndexStats {
            root: self.root.display().to_string(),
            files: self.files.len(),
            chunks: self.chunks.len(),
            symbols: self.files.values().map(|file| file.symbols.len()).sum(),
            seq: self.seq,
            scan: "ready",
            extensions: self.options.extensions.clone(),
            graph_nodes: self.graph_summary().nodes,
            graph_edges: self.graph_summary().edges,
            graph_communities: self.graph_summary().communities,
            embedding_model: self.embedding_model_id.clone(),
            embedding_dims: self.embedding_dims,
            vector_count: self.vector_count,
            vector_units: "files",
            storage_dir: self.storage_dir.clone(),
            cache: self.cache_status,
        }
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        let normalized = normalize_rel_path(path);
        self.files.get(&normalized)
    }

    pub fn file_by_id(&self, file_id: u32) -> Option<&FileEntry> {
        self.file_paths
            .get(file_id as usize)
            .and_then(|path| self.files.get(path))
    }

    pub fn file_content(&self, file: &FileEntry) -> Result<String> {
        if !file.content.is_empty() {
            return Ok(file.content.clone());
        }
        let path = self.root.join(&file.path);
        let bytes =
            fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        Ok(String::from_utf8_lossy(&bytes).to_string())
    }

    pub fn chunk_file_path<'a>(&'a self, chunk: &'a Chunk) -> &'a str {
        chunk_file_path(chunk, &self.file_paths)
    }

    pub fn chunk_content_cached(
        &self,
        chunk: &Chunk,
        content_by_file: &mut HashMap<String, String>,
    ) -> Result<String> {
        if !chunk.content.is_empty() {
            return Ok(chunk.content.clone());
        }
        let file_path = self.chunk_file_path(chunk);
        if !content_by_file.contains_key(file_path) {
            let file = self
                .file(file_path)
                .ok_or_else(|| anyhow!("chunk file not indexed: {}", file_path))?;
            content_by_file.insert(file_path.to_string(), self.file_content(file)?);
        }
        let content = content_by_file
            .get(file_path)
            .ok_or_else(|| anyhow!("chunk file not cached: {}", file_path))?;
        Ok(extract_content_lines(
            content,
            chunk.start_line,
            chunk.end_line,
        ))
    }

    pub fn graph(&self) -> Arc<CodeGraph> {
        if let Some(graph) = self.graph.read().as_ref().cloned() {
            return graph;
        }
        let mut guard = self.graph.write();
        if let Some(graph) = guard.as_ref().cloned() {
            return graph;
        }
        let deps_forward = self.deps_forward_snapshot();
        let graph = Arc::new(CodeGraph::build(&self.files, &deps_forward));
        *guard = Some(graph.clone());
        graph
    }

    pub fn graph_summary(&self) -> LightweightGraphStats {
        if let Some(graph) = self.graph.read().as_ref() {
            return LightweightGraphStats {
                nodes: graph.nodes.len(),
                edges: graph.edges.len(),
                communities: graph.communities.len(),
            };
        }
        self.graph_stats
    }

    pub fn embedding_model(&self) -> Result<Arc<MinishEmbeddingModel>> {
        if let Some(model) = self.model.read().as_ref().cloned() {
            return Ok(model);
        }
        let mut guard = self.model.write();
        if let Some(model) = guard.as_ref().cloned() {
            return Ok(model);
        }
        let model = Arc::new(load_embedding_model(
            &self.root,
            &self.options.embedding_model,
        )?);
        *guard = Some(model.clone());
        Ok(model)
    }

    pub fn vector_store(&self) -> Result<Arc<MinishVectorStore>> {
        if let Some(vectors) = self.vectors.read().as_ref().cloned() {
            return Ok(vectors);
        }
        let mut guard = self.vectors.write();
        if let Some(vectors) = guard.as_ref().cloned() {
            return Ok(vectors);
        }
        let embeddings = if let Some(embeddings) = self.embeddings.write().take() {
            embeddings
        } else if let Some(path) = &self.embeddings_path
            && path.is_file()
        {
            read_embeddings(path)?
        } else {
            let model = self.embedding_model()?;
            let texts = self
                .semantic_units
                .iter()
                .map(|unit| {
                    if !unit.text.is_empty() {
                        unit.text.clone()
                    } else {
                        self.file(&unit.file_path)
                            .map(semantic_text_for_file)
                            .unwrap_or_else(|| unit.file_path.clone())
                    }
                })
                .collect::<Vec<_>>();
            model.encode(&texts)
        };
        let vectors = Arc::new(MinishVectorStore::build(&embeddings, self.embedding_dims)?);
        *guard = Some(vectors.clone());
        Ok(vectors)
    }

    pub fn reverse_deps_for(&self, path: &str) -> Vec<String> {
        self.ensure_deps_reverse();
        self.deps_reverse
            .read()
            .as_ref()
            .and_then(|reverse| reverse.get(path).cloned())
            .unwrap_or_default()
    }

    pub fn deps_for(&self, path: &str) -> Vec<String> {
        self.ensure_deps_forward();
        self.deps_forward
            .read()
            .as_ref()
            .and_then(|deps| deps.get(path).cloned())
            .unwrap_or_default()
    }

    pub fn deps_forward_snapshot(&self) -> HashMap<String, Vec<String>> {
        self.ensure_deps_forward();
        self.deps_forward
            .read()
            .as_ref()
            .cloned()
            .unwrap_or_default()
    }

    pub fn deps_reverse_snapshot(&self) -> HashMap<String, Vec<String>> {
        self.ensure_deps_reverse();
        self.deps_reverse
            .read()
            .as_ref()
            .cloned()
            .unwrap_or_default()
    }

    pub fn word_hits(&self, word: &str) -> Result<Vec<WordHit>> {
        self.ensure_word_index();
        let guard = self.word_index.read();
        let Some(index) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        index.hits(word)
    }

    fn ensure_word_index(&self) {
        if self.word_index.read().is_some() {
            return;
        }
        let mut guard = self.word_index.write();
        if guard.is_some() {
            return;
        }
        let index = match (&self.word_index_path, &self.word_hits_path) {
            (Some(index_path), Some(hits_path)) if index_path.is_file() && hits_path.is_file() => {
                read_word_index(index_path, hits_path).unwrap_or_else(|err| {
                    eprintln!(
                        "codebase-mcp word index cache load failed at {}: {err:#}",
                        index_path.display()
                    );
                    build_word_index_from_sources(&self.root, &self.file_paths).unwrap_or_default()
                })
            }
            _ => build_word_index_from_sources(&self.root, &self.file_paths).unwrap_or_default(),
        };
        let mut index = index;
        if let (Some(index_path), Some(hits_path)) = (&self.word_index_path, &self.word_hits_path) {
            if !index_path.is_file() || !hits_path.is_file() {
                if let Ok(cache) = ProjectCache::new(&self.root, &self.options.storage) {
                    if let Err(err) = cache.save_word_index(&mut index) {
                        eprintln!("codebase-mcp word index cache save failed: {err:#}");
                    }
                }
            }
        }
        *guard = Some(index);
    }

    fn ensure_deps_forward(&self) {
        if self.deps_forward.read().is_some() {
            return;
        }
        let mut guard = self.deps_forward.write();
        if guard.is_some() {
            return;
        }
        let deps = if let Some(path) = &self.deps_path {
            read_deps_forward(path).unwrap_or_else(|err| {
                eprintln!(
                    "codebase-mcp deps cache load failed at {}: {err:#}",
                    path.display()
                );
                HashMap::new()
            })
        } else {
            HashMap::new()
        };
        *guard = Some(deps);
    }

    fn ensure_deps_reverse(&self) {
        if self.deps_reverse.read().is_some() {
            return;
        }
        let mut guard = self.deps_reverse.write();
        if guard.is_some() {
            return;
        }
        let deps_forward = self.deps_forward_snapshot();
        let mut reverse: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (source, targets) in &deps_forward {
            for target in targets {
                reverse
                    .entry(target.clone())
                    .or_default()
                    .insert(source.clone());
            }
        }
        *guard = Some(
            reverse
                .into_iter()
                .map(|(path, sources)| (path, sources.into_iter().collect()))
                .collect(),
        );
    }

    pub fn symbols_named(&self, name: &str) -> Vec<(&FileEntry, &Symbol)> {
        let mut results = Vec::new();
        for file in self.files.values() {
            for symbol in &file.symbols {
                if symbol.name == name {
                    results.push((file, symbol));
                }
            }
        }
        results
    }

    pub fn path_selector(&self, glob: Option<&str>) -> Vec<usize> {
        let Some(glob) = glob else {
            return (0..self.chunks.len()).collect();
        };
        let Ok(globset) = build_globset(glob) else {
            return Vec::new();
        };
        self.chunks
            .iter()
            .enumerate()
            .filter_map(|(idx, chunk)| globset.is_match(self.chunk_file_path(chunk)).then_some(idx))
            .collect()
    }

    pub fn line_hits(
        &self,
        query: &str,
        max_results: usize,
        regex: bool,
        path_glob: Option<&str>,
        compact: bool,
        include_scope: bool,
    ) -> Result<Vec<SearchHit>> {
        let globset = match path_glob {
            Some(glob) => Some(build_globset(glob)?),
            None => None,
        };
        let re = if regex {
            Some(crate::language::regex_case_insensitive(query)?)
        } else {
            None
        };
        let lowered = query.to_ascii_lowercase();
        let mut hits = Vec::new();
        for file in self.files.values() {
            if globset
                .as_ref()
                .is_some_and(|glob| !glob.is_match(&file.path))
            {
                continue;
            }
            let content = self.file_content(file)?;
            for (idx, line) in content.lines().enumerate() {
                if compact && crate::language::is_comment_or_blank(line) {
                    continue;
                }
                let matched = if let Some(re) = &re {
                    re.is_match(line)
                } else {
                    line.to_ascii_lowercase().contains(&lowered)
                };
                if matched {
                    let line_no = idx + 1;
                    let scope = include_scope
                        .then(|| crate::language::scope_for_line(&file.symbols, line_no))
                        .flatten();
                    hits.push(SearchHit {
                        path: file.path.clone(),
                        line: line_no,
                        text: line.trim().to_string(),
                        scope,
                    });
                    if hits.len() >= max_results {
                        return Ok(hits);
                    }
                }
            }
        }
        Ok(hits)
    }
}

fn log_timing(enabled: bool, stage: &str, start: Instant) {
    if enabled {
        eprintln!(
            "codebase-mcp timing {stage}: {:.3}s",
            start.elapsed().as_secs_f32()
        );
    }
}

fn strip_file_contents(files: &mut BTreeMap<String, FileEntry>) {
    for file in files.values_mut() {
        file.content.clear();
        file.content.shrink_to_fit();
    }
}

fn strip_chunk_contents(chunks: &mut [Chunk]) {
    for chunk in chunks {
        chunk.content.clear();
        chunk.content.shrink_to_fit();
    }
}

fn strip_chunk_paths(chunks: &mut [Chunk]) {
    for chunk in chunks {
        chunk.file_path.clear();
        chunk.file_path.shrink_to_fit();
    }
}

fn strip_semantic_unit_text(units: &mut [SemanticUnit]) {
    for unit in units {
        unit.text.clear();
        unit.text.shrink_to_fit();
    }
}

fn assign_chunk_file_ids(chunks: &mut [Chunk], file_paths: &[String]) {
    let path_to_id = file_paths
        .iter()
        .enumerate()
        .map(|(id, path)| (path.as_str(), id as u32))
        .collect::<HashMap<_, _>>();
    for chunk in chunks {
        if let Some(file_id) = path_to_id.get(chunk.file_path.as_str()) {
            chunk.file_id = *file_id;
        }
    }
}

fn chunk_file_path<'a>(chunk: &'a Chunk, file_paths: &'a [String]) -> &'a str {
    if !chunk.file_path.is_empty() {
        return &chunk.file_path;
    }
    file_paths
        .get(chunk.file_id as usize)
        .map(String::as_str)
        .unwrap_or("")
}

fn extract_content_lines(content: &str, start: usize, end: usize) -> String {
    let mut out = String::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if line_no < start {
            continue;
        }
        if line_no > end {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

fn estimate_graph_stats(
    files: &[FileEntry],
    deps_forward: &HashMap<String, Vec<String>>,
) -> LightweightGraphStats {
    let namespaces = files
        .iter()
        .filter_map(|file| file.namespace.as_ref())
        .collect::<BTreeSet<_>>();
    let namespace_edges = files.iter().filter(|file| file.namespace.is_some()).count();
    let indexed_paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<HashSet<_>>();
    let mut dep_edges = 0usize;
    let mut adjacency = HashMap::<&str, Vec<&str>>::new();
    for path in &indexed_paths {
        adjacency.entry(*path).or_default();
    }
    for (source, targets) in deps_forward {
        let source = source.as_str();
        if !indexed_paths.contains(source) {
            continue;
        }
        for target in targets {
            let target = target.as_str();
            if !indexed_paths.contains(target) {
                continue;
            }
            dep_edges += 1;
            adjacency.entry(source).or_default().push(target);
            adjacency.entry(target).or_default().push(source);
        }
    }

    let mut communities = 0usize;
    let mut visited = HashSet::<&str>::new();
    for path in &indexed_paths {
        if !visited.insert(*path) {
            continue;
        }
        communities += 1;
        let mut stack = vec![*path];
        while let Some(current) = stack.pop() {
            for next in adjacency.get(&current).into_iter().flatten() {
                if visited.insert(*next) {
                    stack.push(*next);
                }
            }
        }
    }

    LightweightGraphStats {
        nodes: files.len() + namespaces.len(),
        edges: namespace_edges + dep_edges,
        communities,
    }
}

fn build_chunk_indices_by_file(
    chunks: &[Chunk],
    file_paths: &[String],
) -> HashMap<String, Vec<usize>> {
    let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        by_file
            .entry(chunk_file_path(chunk, file_paths).to_string())
            .or_default()
            .push(idx);
    }
    by_file
}

fn build_symbol_definition_chunks(
    files: &[FileEntry],
    chunks: &[Chunk],
    chunk_indices_by_file: &HashMap<String, Vec<usize>>,
) -> HashMap<String, Vec<usize>> {
    let mut by_symbol: HashMap<String, Vec<usize>> = HashMap::new();
    for file in files {
        let Some(file_chunks) = chunk_indices_by_file.get(&file.path) else {
            continue;
        };
        for symbol in &file.symbols {
            let Some(&chunk_idx) = file_chunks.iter().find(|&&idx| {
                let chunk = &chunks[idx];
                chunk.start_line <= symbol.line_start && symbol.line_start <= chunk.end_line
            }) else {
                continue;
            };
            by_symbol
                .entry(symbol.name.to_ascii_lowercase())
                .or_default()
                .push(chunk_idx);
        }
    }
    for indices in by_symbol.values_mut() {
        indices.sort_unstable();
        indices.dedup();
    }
    by_symbol
}

pub fn fingerprint_project(
    root: impl AsRef<Path>,
    options: &IndexOptions,
) -> Result<BTreeMap<String, String>> {
    let root = root
        .as_ref()
        .canonicalize()
        .with_context(|| format!("failed to resolve project root {}", root.as_ref().display()))?;
    let paths = collect_paths(&root, options)?;
    let fingerprints = fingerprint_paths(&root, &paths, options.max_file_bytes)?;
    Ok(fingerprints
        .into_iter()
        .map(|fingerprint| (fingerprint.path, fingerprint.content_hash))
        .collect())
}

fn fingerprint_paths(
    root: &Path,
    paths: &[PathBuf],
    max_file_bytes: u64,
) -> Result<Vec<SourceFingerprint>> {
    let mut fingerprints = paths
        .par_iter()
        .filter_map(|path| fingerprint_path(root, path, max_file_bytes).ok())
        .collect::<Vec<_>>();
    fingerprints.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(fingerprints)
}

fn fingerprint_path(root: &Path, path: &Path, max_file_bytes: u64) -> Result<SourceFingerprint> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > max_file_bytes {
        return Err(anyhow!("file too large: {}", path.display()));
    }
    let bytes = fs::read(path)?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err(anyhow!("binary file skipped: {}", path.display()));
    }
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or(0);
    Ok(SourceFingerprint {
        path: rel,
        byte_size: bytes.len(),
        modified_unix_ms,
        content_hash: hash_bytes(&bytes),
    })
}

fn collect_paths(root: &Path, options: &IndexOptions) -> Result<Vec<PathBuf>> {
    let extensions: HashSet<String> = options
        .extensions
        .iter()
        .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
        .collect();

    let exclude_globs = build_optional_globset(&options.exclude_paths)?.map(Arc::new);
    let mut paths = BTreeSet::new();
    if options.root_paths.is_empty() {
        collect_paths_from(
            root,
            root,
            &extensions,
            options.respect_gitignore,
            options,
            None,
            exclude_globs.clone(),
            &mut paths,
        )?;
    } else {
        for scan_root in &options.root_paths {
            let scan_root = root.join(scan_root);
            if scan_root.is_dir() {
                collect_paths_from(
                    root,
                    &scan_root,
                    &extensions,
                    options.respect_gitignore,
                    options,
                    Some(&scan_root),
                    exclude_globs.clone(),
                    &mut paths,
                )?;
            }
        }
    }

    for include_path in &options.include_paths {
        let include_path = root.join(include_path);
        if include_path.is_dir() {
            collect_paths_from(
                root,
                &include_path,
                &extensions,
                false,
                options,
                Some(&include_path),
                exclude_globs.clone(),
                &mut paths,
            )?;
        }
    }

    Ok(paths.into_iter().collect())
}

fn collect_paths_from(
    root: &Path,
    start: &Path,
    extensions: &HashSet<String>,
    respect_gitignore: bool,
    options: &IndexOptions,
    include_root: Option<&Path>,
    exclude_globs: Option<Arc<GlobSet>>,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let filter_root = root.to_path_buf();
    let filter_include_root = include_root.map(Path::to_path_buf);
    let filter_options = options.clone();
    let filter_exclude_globs = exclude_globs.clone();
    let mut builder = WalkBuilder::new(start);
    builder
        .hidden(false)
        .parents(false)
        .git_ignore(respect_gitignore)
        .git_exclude(false)
        .git_global(false)
        .filter_entry(move |entry| {
            !is_skipped_entry(
                &filter_root,
                entry.path(),
                entry
                    .file_type()
                    .is_some_and(|file_type| file_type.is_dir()),
                filter_include_root.as_deref(),
                &filter_options,
                filter_exclude_globs.as_deref(),
            )
        });
    let collected = Arc::new(StdMutex::new(Vec::new()));
    let errors = Arc::new(StdMutex::new(Vec::new()));
    builder.build_parallel().run(|| {
        let collected = collected.clone();
        let errors = errors.clone();
        let extensions = extensions.clone();
        let exclude_globs = exclude_globs.clone();
        let root = root.to_path_buf();
        Box::new(move |entry| {
            match entry {
                Ok(entry) => {
                    if entry.file_type().is_some_and(|ft| ft.is_file()) {
                        let path = entry.path();
                        if let Some(ext) = path.extension().and_then(|ext| ext.to_str())
                            && extensions.contains(&ext.to_ascii_lowercase())
                            && !is_excluded_path(&root, path, false, exclude_globs.as_deref())
                        {
                            collected
                                .lock()
                                .expect("path collector poisoned")
                                .push(path.to_path_buf());
                        }
                    }
                }
                Err(err) => errors
                    .lock()
                    .expect("path error collector poisoned")
                    .push(err.to_string()),
            }
            WalkState::Continue
        })
    });
    let errors = errors.lock().expect("path error collector poisoned");
    if let Some(error) = errors.first() {
        return Err(anyhow!("failed to walk source tree: {error}"));
    }
    let mut collected = collected.lock().expect("path collector poisoned");
    for path in collected.drain(..) {
        paths.insert(path);
    }
    Ok(())
}

fn is_skipped_entry(
    root: &Path,
    path: &Path,
    is_dir: bool,
    include_root: Option<&Path>,
    options: &IndexOptions,
    exclude_globs: Option<&GlobSet>,
) -> bool {
    if path == root || include_root.is_some_and(|include_root| path == include_root) {
        return false;
    }

    if is_excluded_path(root, path, is_dir, exclude_globs) {
        return true;
    }

    let relative = include_root
        .and_then(|include_root| path.strip_prefix(include_root).ok())
        .unwrap_or_else(|| path.strip_prefix(root).unwrap_or(path));
    let parts = relative
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .map(|part| part.to_ascii_lowercase())
        .collect::<Vec<_>>();

    for part in parts {
        if matches!(
            part.as_str(),
            ".git" | ".hg" | ".svn" | ".vs" | ".idea" | ".gradle" | ".codedb-mcp" | "node_modules"
        ) {
            return true;
        }

        if options.skip_dirs.iter().any(|skip| skip == &part) {
            return true;
        }
    }

    false
}

fn build_optional_globset(patterns: &[String]) -> Result<Option<GlobSet>> {
    let mut builder = GlobSetBuilder::new();
    let mut added = false;
    for pattern in patterns {
        let pattern = normalize_rel_path(pattern.trim());
        if pattern.is_empty() {
            continue;
        }
        builder.add(Glob::new(&pattern)?);
        added = true;
    }
    if added {
        Ok(Some(builder.build()?))
    } else {
        Ok(None)
    }
}

fn is_excluded_path(
    root: &Path,
    path: &Path,
    is_dir: bool,
    exclude_globs: Option<&GlobSet>,
) -> bool {
    let Some(exclude_globs) = exclude_globs else {
        return false;
    };
    let Some(relative) = project_relative_path(root, path) else {
        return false;
    };
    if relative.is_empty() || exclude_globs.is_match(relative.as_str()) {
        return !relative.is_empty();
    }
    if is_dir {
        let child_probe = format!("{relative}/__codedb_dir_probe__");
        if exclude_globs.is_match(child_probe.as_str()) {
            return true;
        }
    }
    false
}

fn project_relative_path(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
}

fn load_embedding_model(root: &Path, model_id: &str) -> Result<MinishEmbeddingModel> {
    let configured_path = Path::new(model_id);
    let path = if configured_path.is_absolute() {
        configured_path.to_path_buf()
    } else {
        root.join(configured_path)
    };
    if path.exists() {
        MinishEmbeddingModel::load_local(&path)
    } else if configured_path.components().count() > 1
        || model_id.starts_with('.')
        || model_id.contains('\\')
    {
        Err(anyhow!(
            "configured local embedding model path does not exist: {}",
            path.display()
        ))
    } else {
        MinishEmbeddingModel::load(model_id)
    }
}

struct IndexedFileSource {
    file: FileEntry,
    chunks: Vec<Chunk>,
}

fn read_indexed_file_source(
    root: &Path,
    path: &Path,
    max_file_bytes: u64,
    slow_file_ms: u64,
) -> Result<IndexedFileSource> {
    let mut file = read_file_entry(root, path, max_file_bytes, slow_file_ms)?;
    let chunks = chunk_source_metadata(
        file.language.as_str(),
        &file.content,
        &file.path,
        &file.symbols,
    );
    file.content.clear();
    file.content.shrink_to_fit();
    Ok(IndexedFileSource { file, chunks })
}

fn read_file_entry(
    root: &Path,
    path: &Path,
    max_file_bytes: u64,
    slow_file_ms: u64,
) -> Result<FileEntry> {
    let started = (slow_file_ms > 0).then(Instant::now);
    let metadata = fs::metadata(path)?;
    if metadata.len() > max_file_bytes {
        return Err(anyhow!("file too large: {}", path.display()));
    }
    let bytes = fs::read(path)?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err(anyhow!("binary file skipped: {}", path.display()));
    }
    let content = String::from_utf8_lossy(&bytes).to_string();
    let content_hash = hash_bytes(&bytes);
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let language = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(language_for_extension)
        .ok_or_else(|| anyhow!("unsupported source extension: {}", path.display()))?;
    let parsed = analyze_source(language, &content);
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or(0);

    let entry = FileEntry {
        path: rel,
        language: language.into(),
        line_count: content.lines().count(),
        byte_size: bytes.len(),
        modified_unix_ms,
        content_hash,
        namespace: parsed.namespace,
        imports: parsed.imports,
        symbols: parsed.symbols,
        content,
    };
    if let Some(started) = started {
        let elapsed = started.elapsed();
        if elapsed.as_millis() >= slow_file_ms as u128 {
            eprintln!(
                "codebase-mcp slow file {:.3}s {}",
                elapsed.as_secs_f32(),
                path.display()
            );
        }
    }
    Ok(entry)
}

fn build_dependency_references_from_sources(
    root: &Path,
    files: &[FileEntry],
    dependency_symbols: &HashMap<String, Vec<SymbolDefinition>>,
) -> HashMap<String, DependencyReferences> {
    files
        .par_iter()
        .fold(HashMap::new, |mut references_by_file, file| {
            let content = read_source_content(root, &file.path).unwrap_or_default();
            let mut dependency_references = DependencyReferences::default();
            let aliases =
                (file.language == "csharp").then(|| parse_using_aliases_from_iter(content.lines()));
            if file.language == "csharp" {
                collect_static_using_dependency_references_from_iter(
                    content.lines(),
                    dependency_symbols,
                    &mut dependency_references,
                );
            }
            for line in content.lines() {
                let identifiers = raw_identifiers(line);
                let code = strip_strings_and_line_comment(line);
                let dependency_line = is_dependency_reference_line(file.language.as_str(), &code);
                if dependency_line {
                    collect_qualified_dependency_references(
                        &code,
                        dependency_symbols,
                        &mut dependency_references,
                    );
                    if let Some(aliases) = &aliases {
                        collect_alias_dependency_references(
                            &code,
                            aliases,
                            dependency_symbols,
                            &mut dependency_references,
                        );
                        collect_attribute_dependency_references(
                            &code,
                            dependency_symbols,
                            &mut dependency_references,
                        );
                    } else if file.language == "java" {
                        collect_java_annotation_dependency_references(
                            &code,
                            dependency_symbols,
                            &mut dependency_references,
                        );
                    }
                }
                for raw in identifiers {
                    if dependency_line && dependency_symbols.contains_key(&raw) {
                        dependency_references.identifiers.insert(raw);
                    }
                }
            }
            references_by_file.insert(file.path.clone(), dependency_references);
            references_by_file
        })
        .reduce(HashMap::new, |mut left, right| {
            left.extend(right);
            left
        })
}

fn build_word_index_from_sources(root: &Path, file_paths: &[String]) -> Result<WordIndex> {
    let index = file_paths
        .par_iter()
        .enumerate()
        .fold(
            HashMap::<String, Vec<WordHit>>::new,
            |mut index, (file_id, rel_path)| {
                let path = root.join(rel_path);
                let Ok(bytes) = fs::read(path) else {
                    return index;
                };
                if bytes.iter().take(8192).any(|b| *b == 0) {
                    return index;
                }
                let content = String::from_utf8_lossy(&bytes);
                for (line_idx, line) in content.lines().enumerate() {
                    let mut seen = HashSet::new();
                    for raw in raw_identifiers(line) {
                        if seen.insert(raw.clone()) {
                            index.entry(raw).or_default().push(WordHit {
                                file_id: file_id as u32,
                                line: line_idx as u32 + 1,
                            });
                        }
                    }
                }
                index
            },
        )
        .reduce(HashMap::<String, Vec<WordHit>>::new, |mut left, right| {
            for (word, mut hits) in right {
                left.entry(word).or_default().append(&mut hits);
            }
            left
        });
    Ok(WordIndex::from_map(index))
}

#[cfg(test)]
fn build_word_index(
    files: &[FileEntry],
    chunks: &[Chunk],
    chunk_indices_by_file: &HashMap<String, Vec<usize>>,
    dependency_symbols: &HashMap<String, Vec<SymbolDefinition>>,
) -> (WordIndex, HashMap<String, DependencyReferences>) {
    #[derive(Default)]
    struct WordIndexBuild {
        index: HashMap<String, Vec<WordHit>>,
        references_by_file: HashMap<String, DependencyReferences>,
    }

    let built = files
        .par_iter()
        .enumerate()
        .fold(WordIndexBuild::default, |mut built, (file_id, file)| {
            let mut dependency_references = DependencyReferences::default();
            let lines = file_chunk_lines(file, chunks, chunk_indices_by_file);
            let aliases =
                (file.language == "csharp").then(|| parse_using_aliases_from_lines(&lines));
            if file.language == "csharp" {
                collect_static_using_dependency_references_from_lines(
                    &lines,
                    dependency_symbols,
                    &mut dependency_references,
                );
            }
            for (line_no, line) in lines {
                let mut seen = HashSet::new();
                let identifiers = raw_identifiers(line);
                let code = strip_strings_and_line_comment(line);
                let dependency_line = is_dependency_reference_line(file.language.as_str(), &code);
                if dependency_line {
                    collect_qualified_dependency_references(
                        &code,
                        dependency_symbols,
                        &mut dependency_references,
                    );
                    if let Some(aliases) = &aliases {
                        collect_alias_dependency_references(
                            &code,
                            aliases,
                            dependency_symbols,
                            &mut dependency_references,
                        );
                        collect_attribute_dependency_references(
                            &code,
                            dependency_symbols,
                            &mut dependency_references,
                        );
                    } else if file.language == "java" {
                        collect_java_annotation_dependency_references(
                            &code,
                            dependency_symbols,
                            &mut dependency_references,
                        );
                    }
                }
                for raw in identifiers {
                    if dependency_line && dependency_symbols.contains_key(&raw) {
                        dependency_references.identifiers.insert(raw.clone());
                    }
                    if seen.insert(raw.clone()) {
                        built.index.entry(raw).or_default().push(WordHit {
                            file_id: file_id as u32,
                            line: line_no as u32,
                        });
                    }
                }
            }
            built
                .references_by_file
                .insert(file.path.clone(), dependency_references);
            built
        })
        .reduce(WordIndexBuild::default, |mut left, right| {
            for (word, mut hits) in right.index {
                left.index.entry(word).or_default().append(&mut hits);
            }
            left.references_by_file.extend(right.references_by_file);
            left
        });

    (WordIndex::from_map(built.index), built.references_by_file)
}

fn file_chunk_lines<'a>(
    file: &FileEntry,
    chunks: &'a [Chunk],
    chunk_indices_by_file: &HashMap<String, Vec<usize>>,
) -> Vec<(usize, &'a str)> {
    let Some(indices) = chunk_indices_by_file.get(&file.path) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for &chunk_idx in indices {
        let Some(chunk) = chunks.get(chunk_idx) else {
            continue;
        };
        for (offset, line) in chunk.content.lines().enumerate() {
            lines.push((chunk.start_line + offset, line));
        }
    }
    lines
}

#[derive(Clone)]
struct SymbolDefinition {
    name: String,
    path: String,
    namespace: Option<String>,
    module_path: Option<String>,
}

#[derive(Default)]
struct DependencyReferences {
    identifiers: HashSet<String>,
    qualified_names: HashSet<String>,
}

fn build_dependency_symbols(files: &[FileEntry]) -> HashMap<String, Vec<SymbolDefinition>> {
    let mut symbols_by_name: HashMap<String, Vec<SymbolDefinition>> = HashMap::new();
    for file in files {
        let type_symbols = file
            .symbols
            .iter()
            .filter(|symbol| is_dependency_symbol_kind(symbol.kind.as_str()))
            .collect::<Vec<_>>();
        if file.language == "rust" {
            for symbol in type_symbols {
                push_dependency_symbol(&mut symbols_by_name, file, symbol);
            }
            continue;
        }
        let has_primary_type = type_symbols.iter().any(|symbol| {
            is_dependency_symbol_kind(symbol.kind.as_str())
                && is_primary_symbol_for_file(&file.path, &symbol.name)
        });
        let include_single_non_primary_type = !has_primary_type && type_symbols.len() == 1;
        for symbol in type_symbols {
            if has_primary_type {
                if !is_primary_symbol_for_file(&file.path, &symbol.name) {
                    continue;
                }
            } else if !include_single_non_primary_type {
                continue;
            }
            push_dependency_symbol(&mut symbols_by_name, file, symbol);
        }
    }
    symbols_by_name
}

fn push_dependency_symbol(
    symbols_by_name: &mut HashMap<String, Vec<SymbolDefinition>>,
    file: &FileEntry,
    symbol: &Symbol,
) {
    symbols_by_name
        .entry(symbol.name.clone())
        .or_default()
        .push(SymbolDefinition {
            name: symbol.name.clone(),
            path: file.path.clone(),
            namespace: file.namespace.clone(),
            module_path: (file.language == "rust").then(|| rust_module_path_from_file(&file.path)),
        });
}

fn build_dependencies(
    root: Option<&Path>,
    files: &[FileEntry],
    chunks: &[Chunk],
    chunk_indices_by_file: &HashMap<String, Vec<usize>>,
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references_by_file: &HashMap<String, DependencyReferences>,
) -> (HashMap<String, Vec<String>>, HashMap<String, Vec<String>>) {
    let mut forward: HashMap<String, BTreeSet<String>> = HashMap::new();
    let indexed_paths = files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<HashSet<_>>();
    for file in files {
        let deps = forward.entry(file.path.clone()).or_default();
        if file.language == "rust" {
            collect_rust_module_file_dependencies(
                root,
                file,
                chunks,
                chunk_indices_by_file,
                &indexed_paths,
                deps,
            );
        }
        if file.language == "lua" {
            collect_lua_require_file_dependencies(file, &indexed_paths, deps);
        }
        let Some(references) = references_by_file.get(&file.path) else {
            continue;
        };
        for identifier in &references.identifiers {
            let Some(candidates) = symbols_by_name.get(identifier) else {
                continue;
            };
            for candidate in candidates {
                if candidate.path == file.path {
                    continue;
                }
                if can_reference_symbol_definition(file, candidate)
                    || references_qualified_symbol(references, candidate, identifier)
                {
                    deps.insert(candidate.path.clone());
                }
            }
        }
    }

    let mut forward_vec = HashMap::new();
    let mut reverse: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (path, deps) in forward {
        let values: Vec<String> = deps.into_iter().collect();
        for dep in &values {
            reverse.entry(dep.clone()).or_default().insert(path.clone());
        }
        forward_vec.insert(path, values);
    }
    let reverse_vec = reverse
        .into_iter()
        .map(|(path, values)| (path, values.into_iter().collect()))
        .collect();
    (forward_vec, reverse_vec)
}

fn is_dependency_symbol_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class" | "interface" | "struct" | "enum" | "record" | "union" | "trait" | "type_alias"
    )
}

fn is_primary_symbol_for_file(path: &str, symbol_name: &str) -> bool {
    let stem = Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let stem = stem.replace(['_', '-', '.'], "");
    let symbol = symbol_name.to_ascii_lowercase();
    stem == symbol || (symbol.len() >= 4 && stem.starts_with(&symbol))
}

fn is_dependency_reference_line(language: &str, line: &str) -> bool {
    let trimmed = line.trim_start();
    !(trimmed.is_empty()
        || is_import_or_namespace_line(language, trimmed)
        || trimmed.starts_with("//")
        || trimmed.starts_with("/*")
        || trimmed.starts_with('*'))
}

fn is_import_or_namespace_line(language: &str, trimmed: &str) -> bool {
    match language {
        "java" => trimmed.starts_with("import ") || trimmed.starts_with("package "),
        "rust" => {
            trimmed.starts_with("use ")
                || trimmed.starts_with("pub use ")
                || rust_module_name(trimmed).is_some()
        }
        _ => trimmed.starts_with("using ") || trimmed.starts_with("namespace "),
    }
}

fn collect_rust_module_file_dependencies(
    root: Option<&Path>,
    file: &FileEntry,
    chunks: &[Chunk],
    chunk_indices_by_file: &HashMap<String, Vec<usize>>,
    indexed_paths: &HashSet<&str>,
    deps: &mut BTreeSet<String>,
) {
    if !file.content.is_empty() {
        collect_rust_module_file_dependencies_from_lines(
            file.content.lines(),
            file,
            indexed_paths,
            deps,
        );
        return;
    }
    if let Some(root) = root
        && let Ok(content) = read_source_content(root, &file.path)
    {
        collect_rust_module_file_dependencies_from_lines(
            content.lines(),
            file,
            indexed_paths,
            deps,
        );
        return;
    }
    let lines = file_chunk_lines(file, chunks, chunk_indices_by_file);
    collect_rust_module_file_dependencies_from_lines(
        lines.into_iter().map(|(_, line)| line),
        file,
        indexed_paths,
        deps,
    );
}

fn collect_rust_module_file_dependencies_from_lines<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    file: &FileEntry,
    indexed_paths: &HashSet<&str>,
    deps: &mut BTreeSet<String>,
) {
    for line in lines {
        let code = strip_strings_and_line_comment(line);
        let Some(name) = rust_module_name(code.trim_start()) else {
            continue;
        };
        for candidate in rust_module_dependency_candidates(&file.path, &name) {
            if candidate != file.path && indexed_paths.contains(candidate.as_str()) {
                deps.insert(candidate);
                break;
            }
        }
    }
}

fn rust_module_name(trimmed: &str) -> Option<String> {
    let captures = rust_module_re().captures(trimmed)?;
    Some(captures[1].to_string())
}

fn rust_module_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
            .expect("valid rust module regex")
    })
}

fn rust_module_dependency_candidates(path: &str, name: &str) -> Vec<String> {
    let normalized = path.replace('\\', "/");
    let (dir, file_name) = normalized
        .rsplit_once('/')
        .map(|(dir, file)| (dir.to_string(), file))
        .unwrap_or_else(|| (String::new(), normalized.as_str()));
    let stem = file_name.strip_suffix(".rs").unwrap_or(file_name);
    let base = if matches!(stem, "main" | "lib" | "mod") {
        dir
    } else if dir.is_empty() {
        stem.to_string()
    } else {
        format!("{dir}/{stem}")
    };
    let prefix = if base.is_empty() {
        name.to_string()
    } else {
        format!("{base}/{name}")
    };
    vec![format!("{prefix}.rs"), format!("{prefix}/mod.rs")]
}

fn collect_lua_require_file_dependencies(
    file: &FileEntry,
    indexed_paths: &HashSet<&str>,
    deps: &mut BTreeSet<String>,
) {
    for module in &file.imports {
        for candidate in lua_require_dependency_candidates(module) {
            if candidate == file.path {
                continue;
            }
            if indexed_paths.contains(candidate.as_str()) {
                deps.insert(candidate);
                break;
            }
            if let Some(indexed) = indexed_paths
                .iter()
                .find(|path| path.ends_with(&format!("/{candidate}")))
            {
                deps.insert((*indexed).to_string());
                break;
            }
        }
    }
}

fn lua_require_dependency_candidates(module: &str) -> Vec<String> {
    let module_path = module
        .replace('\\', "/")
        .replace('.', "/")
        .trim_matches('/')
        .to_string();
    if module_path.is_empty() {
        return Vec::new();
    }
    vec![
        format!("{module_path}.lua"),
        format!("{module_path}/init.lua"),
    ]
}

fn rust_module_path_from_file(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let src_path = normalized.strip_prefix("src/").unwrap_or(&normalized);
    path_to_rust_module_path(src_path)
}

fn path_to_rust_module_path(path: &str) -> String {
    let without_ext = path.strip_suffix(".rs").unwrap_or(path);
    let parts = without_ext
        .split('/')
        .filter(|part| !part.is_empty())
        .filter(|part| *part != "main" && *part != "lib" && *part != "mod")
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "crate".to_string()
    } else {
        format!("crate.{}", parts.join("."))
    }
}

fn collect_qualified_dependency_references(
    line: &str,
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references: &mut DependencyReferences,
) {
    for matched in qualified_name_re().find_iter(line) {
        let parts = matched
            .as_str()
            .replace("::", ".")
            .split('.')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .filter(|part| *part != "global")
            .map(str::to_string)
            .collect::<Vec<_>>();
        for idx in 1..parts.len() {
            let name = &parts[idx];
            if symbols_by_name.contains_key(name) {
                references.identifiers.insert(name.to_string());
                references.qualified_names.insert(parts[..=idx].join("."));
            }
        }
    }
}

fn qualified_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(?:global\s*::\s*)?[A-Za-z_][A-Za-z0-9_]*(?:(?:\s*\.\s*|\s*::\s*)[A-Za-z_][A-Za-z0-9_]*)+")
            .expect("valid qualified name regex")
    })
}

#[cfg(test)]
fn parse_using_aliases_from_lines(lines: &[(usize, &str)]) -> HashMap<String, String> {
    parse_using_aliases_from_iter(lines.iter().map(|(_, line)| *line))
}

fn parse_using_aliases_from_iter<'a>(
    lines: impl IntoIterator<Item = &'a str>,
) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for line in lines {
        if let Some(caps) = using_alias_re().captures(line) {
            aliases.insert(caps[1].to_string(), normalize_qualified_name(&caps[2]));
        }
    }
    aliases
}

fn using_alias_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*using\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*((?:global\s*::\s*)?[A-Za-z_][A-Za-z0-9_]*(?:(?:\s*\.\s*|\s*::\s*)[A-Za-z_][A-Za-z0-9_]*)*)\s*;")
            .expect("valid using alias regex")
    })
}

fn static_using_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*using\s+static\s+((?:global\s*::\s*)?[A-Za-z_][A-Za-z0-9_]*(?:(?:\s*\.\s*|\s*::\s*)[A-Za-z_][A-Za-z0-9_]*)*)\s*;")
            .expect("valid static using regex")
    })
}

#[cfg(test)]
fn collect_static_using_dependency_references_from_lines(
    lines: &[(usize, &str)],
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references: &mut DependencyReferences,
) {
    collect_static_using_dependency_references_from_iter(
        lines.iter().map(|(_, line)| *line),
        symbols_by_name,
        references,
    );
}

fn collect_static_using_dependency_references_from_iter<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references: &mut DependencyReferences,
) {
    for line in lines {
        if let Some(caps) = static_using_re().captures(line) {
            collect_qualified_dependency_reference(
                &normalize_qualified_name(&caps[1]),
                symbols_by_name,
                references,
            );
        }
    }
}

fn collect_alias_dependency_references(
    line: &str,
    aliases: &HashMap<String, String>,
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references: &mut DependencyReferences,
) {
    for identifier in raw_identifiers(line) {
        if let Some(qualified) = aliases.get(&identifier) {
            collect_qualified_dependency_reference(qualified, symbols_by_name, references);
        }
    }
}

fn collect_attribute_dependency_references(
    line: &str,
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references: &mut DependencyReferences,
) {
    let mut rest = line.trim_start();
    while rest.starts_with('[') {
        let Some((content, consumed)) = attribute_bracket_content(rest) else {
            break;
        };
        for item in split_attribute_items(content) {
            let Some((qualified, name)) = attribute_type_name(item) else {
                continue;
            };
            let suffixed = format!("{name}Attribute");
            if symbols_by_name.contains_key(&suffixed) {
                references.identifiers.insert(suffixed.clone());
                if let Some(prefix) = qualified.strip_suffix(&name) {
                    references
                        .qualified_names
                        .insert(format!("{prefix}{suffixed}"));
                }
            }
            if symbols_by_name.contains_key(&name) {
                references.identifiers.insert(name.clone());
                references.qualified_names.insert(qualified);
            }
        }
        rest = rest[consumed..].trim_start();
    }
}

fn collect_java_annotation_dependency_references(
    line: &str,
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references: &mut DependencyReferences,
) {
    for matched in java_annotation_re().captures_iter(line) {
        let qualified = normalize_qualified_name(&matched[1]);
        let Some(name) = qualified.rsplit('.').next() else {
            continue;
        };
        if symbols_by_name.contains_key(name) {
            references.identifiers.insert(name.to_string());
            references.qualified_names.insert(qualified);
        }
    }
}

fn java_annotation_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"@([A-Za-z_$][A-Za-z0-9_$]*(?:\.[A-Za-z_$][A-Za-z0-9_$]*)*)")
            .expect("valid Java annotation regex")
    })
}

fn attribute_bracket_content(line: &str) -> Option<(&str, usize)> {
    if !line.starts_with('[') {
        return None;
    }
    let end = line
        .char_indices()
        .find_map(|(idx, ch)| (ch == ']').then_some(idx))?;
    Some((&line[1..end], end + 1))
}

fn split_attribute_items(content: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0usize;
    let mut paren_depth = 0usize;
    for (idx, ch) in content.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            ',' if paren_depth == 0 => {
                let item = content[start..idx].trim();
                if !item.is_empty() {
                    items.push(item);
                }
                start = idx + 1;
            }
            _ => {}
        }
    }
    let item = content[start..].trim();
    if !item.is_empty() {
        items.push(item);
    }
    items
}

fn attribute_type_name(item: &str) -> Option<(String, String)> {
    let mut item = item.trim();
    if let Some((target, value)) = item.split_once(':') {
        if !target.contains('(') {
            item = value.trim_start();
        }
    }
    let head = item
        .split(|ch: char| ch == '(' || ch == '=' || ch.is_whitespace())
        .next()
        .unwrap_or_default();
    if head.is_empty() {
        return None;
    }
    let qualified = normalize_qualified_name(head);
    let name = qualified.rsplit('.').next()?.to_string();
    (!name.is_empty()).then_some((qualified, name))
}

fn collect_qualified_dependency_reference(
    qualified: &str,
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references: &mut DependencyReferences,
) {
    let parts = qualified
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    for idx in 1..parts.len() {
        let name = parts[idx];
        if symbols_by_name.contains_key(name) {
            references.identifiers.insert(name.to_string());
            references.qualified_names.insert(parts[..=idx].join("."));
        }
    }
}

fn normalize_qualified_name(value: &str) -> String {
    value
        .replace("::", ".")
        .split('.')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .filter(|part| *part != "global")
        .collect::<Vec<_>>()
        .join(".")
}

fn strip_strings_and_line_comment(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    let mut in_char = false;
    let mut verbatim = false;
    while let Some(ch) = chars.next() {
        if !in_string && !in_char && ch == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if !in_string && !in_char && ch == '@' && chars.peek() == Some(&'"') {
            verbatim = true;
            in_string = true;
            out.push(' ');
            out.push(' ');
            chars.next();
            continue;
        }
        if !in_string && !in_char && ch == '"' {
            in_string = true;
            verbatim = false;
            out.push(' ');
            continue;
        }
        if !in_string && !in_char && ch == '\'' {
            in_char = true;
            out.push(' ');
            continue;
        }
        if in_string {
            if ch == '"' {
                if verbatim && chars.peek() == Some(&'"') {
                    chars.next();
                } else {
                    in_string = false;
                    verbatim = false;
                }
            }
            out.push(' ');
            continue;
        }
        if in_char {
            if ch == '\\' {
                chars.next();
            } else if ch == '\'' {
                in_char = false;
            }
            out.push(' ');
            continue;
        }
        out.push(ch);
    }
    out
}

fn can_reference_symbol_definition(file: &FileEntry, candidate: &SymbolDefinition) -> bool {
    if file.language == "rust" && candidate.module_path.is_some() {
        return rust_imports_symbol_module(&file.imports, candidate);
    }
    let Some(candidate_namespace) = candidate.namespace.as_deref() else {
        return file.namespace.is_none();
    };
    file.namespace.as_deref() == Some(candidate_namespace)
        || imports_symbol_namespace(&file.imports, candidate_namespace, &candidate.name)
}

fn rust_imports_symbol_module(imports: &[String], candidate: &SymbolDefinition) -> bool {
    let Some(module_path) = candidate.module_path.as_deref() else {
        return false;
    };
    let fully_qualified = format!("{module_path}.{}", candidate.name);
    let wildcard = format!("{module_path}.*");
    imports
        .iter()
        .any(|import| import == &fully_qualified || import == &wildcard)
}

fn imports_symbol_namespace(imports: &[String], namespace: &str, name: &str) -> bool {
    let fully_qualified = format!("{namespace}.{name}");
    let wildcard = format!("{namespace}.*");
    imports
        .iter()
        .any(|import| import == namespace || import == &fully_qualified || import == &wildcard)
}

fn references_qualified_symbol(
    references: &DependencyReferences,
    candidate: &SymbolDefinition,
    identifier: &str,
) -> bool {
    if let Some(module_path) = candidate.module_path.as_deref() {
        return references
            .qualified_names
            .contains(&format!("{module_path}.{identifier}"));
    }
    let Some(namespace) = candidate.namespace.as_deref() else {
        return false;
    };
    references
        .qualified_names
        .contains(&format!("{namespace}.{identifier}"))
}

pub fn build_globset(pattern: &str) -> Result<GlobSet> {
    let normalized = normalize_rel_path(pattern);
    let promoted = if !normalized.contains('/') && !normalized.starts_with("**/") {
        format!("**/{normalized}")
    } else {
        normalized
    };
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(&promoted)?);
    Ok(builder.build()?)
}

pub fn normalize_rel_path(path: &str) -> String {
    path.trim_start_matches("./").replace('\\', "/")
}

pub fn hash_content(content: &str) -> String {
    let hash = blake3::hash(content.as_bytes());
    hash.to_hex()[..16].to_string()
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    let hash = blake3::hash(bytes);
    hash.to_hex()[..16].to_string()
}

pub fn now_ms() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or(0)
}

fn add_bm25_documents_from_sources(
    root: &Path,
    chunks: &[Chunk],
    file_paths: &[String],
    chunk_indices_by_file: &HashMap<String, Vec<usize>>,
    mut add_document: impl FnMut(Vec<String>) -> Result<()>,
) -> Result<()> {
    let mut next_chunk_idx = 0usize;
    for rel_path in file_paths {
        let Some(indices) = chunk_indices_by_file.get(rel_path) else {
            continue;
        };
        if indices.is_empty() {
            continue;
        }
        let content = read_source_content(root, rel_path)?;
        let lines = content.lines().collect::<Vec<_>>();
        for &idx in indices {
            while next_chunk_idx < idx {
                add_document(Vec::<String>::new())?;
                next_chunk_idx += 1;
            }
            let Some(chunk) = chunks.get(idx) else {
                continue;
            };
            add_document(bm25_tokens_for_chunk(chunk, rel_path, &lines))?;
            next_chunk_idx += 1;
        }
    }
    while next_chunk_idx < chunks.len() {
        add_document(Vec::<String>::new())?;
        next_chunk_idx += 1;
    }
    Ok(())
}

fn read_source_content(root: &Path, rel_path: &str) -> Result<String> {
    let path = root.join(rel_path);
    let bytes =
        fs::read(&path).with_context(|| format!("failed to read source {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn bm25_tokens_for_chunk(chunk: &Chunk, file_path: &str, lines: &[&str]) -> Vec<String> {
    let mut tokens = Vec::new();
    append_path_tokens(&mut tokens, file_path);
    if lines.is_empty() || chunk.start_line == 0 || chunk.end_line < chunk.start_line {
        return tokens;
    }
    let start = chunk.start_line.saturating_sub(1).min(lines.len());
    let end = chunk.end_line.min(lines.len());
    for line in &lines[start..end] {
        append_text_tokens(&mut tokens, line);
    }
    tokens
}

fn append_path_tokens(tokens: &mut Vec<String>, file_path: &str) {
    let path = Path::new(file_path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let parent = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    append_identifier_tokens(tokens, stem, false);
    append_identifier_tokens(tokens, stem, false);
    append_identifier_tokens(tokens, parent, false);
}

fn append_text_tokens(tokens: &mut Vec<String>, text: &str) {
    for raw in raw_identifiers(text) {
        append_identifier_tokens(tokens, &raw, true);
    }
}

fn append_identifier_tokens(tokens: &mut Vec<String>, identifier: &str, filter_stopwords: bool) {
    tokens.extend(
        split_identifier(identifier)
            .into_iter()
            .filter(|token| is_bm25_token(token, filter_stopwords)),
    );
}

fn is_bm25_token(token: &str, filter_stopwords: bool) -> bool {
    token.len() > 1 && (!filter_stopwords || !is_bm25_code_stopword(token))
}

fn is_bm25_code_stopword(token: &str) -> bool {
    matches!(
        token,
        "abstract"
            | "and"
            | "as"
            | "async"
            | "await"
            | "base"
            | "bool"
            | "boolean"
            | "break"
            | "by"
            | "case"
            | "catch"
            | "char"
            | "class"
            | "const"
            | "continue"
            | "default"
            | "delegate"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "event"
            | "extends"
            | "extern"
            | "false"
            | "final"
            | "finally"
            | "float"
            | "for"
            | "foreach"
            | "from"
            | "get"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "int"
            | "interface"
            | "internal"
            | "is"
            | "let"
            | "long"
            | "namespace"
            | "new"
            | "null"
            | "object"
            | "or"
            | "out"
            | "override"
            | "package"
            | "params"
            | "partial"
            | "private"
            | "protected"
            | "public"
            | "readonly"
            | "ref"
            | "return"
            | "sealed"
            | "set"
            | "short"
            | "static"
            | "string"
            | "struct"
            | "switch"
            | "this"
            | "throw"
            | "throws"
            | "true"
            | "try"
            | "using"
            | "var"
            | "virtual"
            | "void"
            | "when"
            | "where"
            | "while"
            | "yield"
    )
}

fn semantic_text_for_file(file: &FileEntry) -> String {
    let path = Path::new(&file.path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let parent = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut text = String::new();
    text.push_str(&file.path);
    text.push('\n');
    text.push_str(stem);
    text.push(' ');
    text.push_str(parent);
    text.push('\n');
    if let Some(namespace) = &file.namespace {
        text.push_str("namespace ");
        text.push_str(namespace);
        text.push('\n');
    }
    if !file.imports.is_empty() {
        text.push_str("imports ");
        text.push_str(&file.imports.join(" "));
        text.push('\n');
    }
    text.push_str("symbols ");
    for symbol in file.symbols.iter().take(256) {
        text.push_str(symbol.kind.as_str());
        text.push(' ');
        text.push_str(&symbol.name);
        text.push(' ');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, content: &str) -> FileEntry {
        file_with_language(path, "csharp", content)
    }

    fn java_file(path: &str, content: &str) -> FileEntry {
        file_with_language(path, "java", content)
    }

    fn rust_file(path: &str, content: &str) -> FileEntry {
        file_with_language(path, "rust", content)
    }

    fn lua_file(path: &str, content: &str) -> FileEntry {
        file_with_language(path, "lua", content)
    }

    fn file_with_language(path: &str, language: &str, content: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            language: language.into(),
            line_count: content.lines().count(),
            byte_size: content.len(),
            modified_unix_ms: 0,
            content_hash: hash_content(content),
            namespace: parse_namespace(language, content),
            imports: parse_imports(language, content),
            symbols: analyze_symbols(language, content),
            content: content.to_string(),
        }
    }

    fn dependency_paths(files: Vec<FileEntry>, path: &str) -> Vec<String> {
        let mut chunks = Vec::new();
        for file in &files {
            chunks.extend(chunk_source(
                file.language.as_str(),
                &file.content,
                &file.path,
                &file.symbols,
            ));
        }
        for (id, chunk) in chunks.iter_mut().enumerate() {
            chunk.id = id;
        }
        let file_paths = files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        assign_chunk_file_ids(&mut chunks, &file_paths);
        let chunk_indices_by_file = build_chunk_indices_by_file(&chunks, &file_paths);
        let symbols = build_dependency_symbols(&files);
        let (_, references) = build_word_index(&files, &chunks, &chunk_indices_by_file, &symbols);
        let (forward, _) = build_dependencies(
            None,
            &files,
            &chunks,
            &chunk_indices_by_file,
            &symbols,
            &references,
        );
        forward.get(path).cloned().unwrap_or_default()
    }

    #[test]
    fn include_paths_override_skipped_parent_dirs() {
        let root = std::env::temp_dir().join(format!("codebase_mcp_include_paths_{}", now_ms()));
        let package_cache = root.join("Library").join("PackageCache");
        let other_library = root.join("Library").join("Other");
        std::fs::create_dir_all(&package_cache).unwrap();
        std::fs::create_dir_all(&other_library).unwrap();
        std::fs::write(
            package_cache.join("Included.cs"),
            "public class Included {}",
        )
        .unwrap();
        std::fs::write(other_library.join("Skipped.cs"), "public class Skipped {}").unwrap();

        let mut options = IndexOptions::default();
        options.extensions = vec!["cs".to_string()];
        options.include_paths = vec!["Library/PackageCache".to_string()];
        options.skip_dirs = vec!["library".to_string()];

        let paths = collect_paths(&root, &options).unwrap();
        let rel_paths = paths
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        assert!(rel_paths.contains(&"Library/PackageCache/Included.cs".to_string()));
        assert!(!rel_paths.contains(&"Library/Other/Skipped.cs".to_string()));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn root_paths_limit_scan_scope() {
        let root = std::env::temp_dir().join(format!("codebase_mcp_root_paths_{}", now_ms()));
        std::fs::create_dir_all(root.join("Assets")).unwrap();
        std::fs::create_dir_all(root.join("Packages")).unwrap();
        std::fs::create_dir_all(root.join("Docs")).unwrap();
        std::fs::write(
            root.join("Assets").join("Runtime.cs"),
            "public class Runtime {}",
        )
        .unwrap();
        std::fs::write(
            root.join("Packages").join("Package.cs"),
            "public class Package {}",
        )
        .unwrap();
        std::fs::write(
            root.join("Docs").join("Ignored.cs"),
            "public class Ignored {}",
        )
        .unwrap();

        let mut options = IndexOptions::default();
        options.extensions = vec!["cs".to_string()];
        options.root_paths = vec!["Assets".to_string(), "Packages".to_string()];
        options.include_paths = Vec::new();
        options.skip_dirs = vec![".git".to_string()];

        let paths = collect_paths(&root, &options).unwrap();
        let rel_paths = paths
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        assert!(rel_paths.contains(&"Assets/Runtime.cs".to_string()));
        assert!(rel_paths.contains(&"Packages/Package.cs".to_string()));
        assert!(!rel_paths.contains(&"Docs/Ignored.cs".to_string()));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unity_runtime_scope_keeps_runtime_roots_and_excludes_editor_dirs() {
        let root = std::env::temp_dir().join(format!("codebase_mcp_unity_runtime_{}", now_ms()));
        std::fs::create_dir_all(root.join("Assets").join("Scripts").join("Editor")).unwrap();
        std::fs::create_dir_all(root.join("Packages").join("GamePackage")).unwrap();
        std::fs::create_dir_all(
            root.join("Library")
                .join("PackageCache")
                .join("UnityPackage"),
        )
        .unwrap();
        std::fs::create_dir_all(root.join("Library").join("Other")).unwrap();
        std::fs::write(
            root.join("Assets").join("Scripts").join("Runtime.cs"),
            "public class Runtime {}",
        )
        .unwrap();
        std::fs::write(
            root.join("Assets")
                .join("Scripts")
                .join("Editor")
                .join("EditorOnly.cs"),
            "public class EditorOnly {}",
        )
        .unwrap();
        std::fs::write(
            root.join("Packages")
                .join("GamePackage")
                .join("PackageRuntime.cs"),
            "public class PackageRuntime {}",
        )
        .unwrap();
        std::fs::write(
            root.join("Library")
                .join("PackageCache")
                .join("UnityPackage")
                .join("PackageCacheRuntime.cs"),
            "public class PackageCacheRuntime {}",
        )
        .unwrap();
        std::fs::write(
            root.join("Library").join("Other").join("Skipped.cs"),
            "public class Skipped {}",
        )
        .unwrap();

        let mut options = IndexOptions::default();
        options.extensions = vec!["cs".to_string()];
        options.root_paths = vec![
            "Assets".to_string(),
            "Packages".to_string(),
            "Library/PackageCache".to_string(),
        ];
        options.include_paths = Vec::new();
        options.exclude_paths = vec!["**/Editor".to_string(), "**/Editor/**".to_string()];
        options.skip_dirs = vec!["library".to_string(), ".git".to_string()];

        let paths = collect_paths(&root, &options).unwrap();
        let rel_paths = paths
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        assert!(rel_paths.contains(&"Assets/Scripts/Runtime.cs".to_string()));
        assert!(rel_paths.contains(&"Packages/GamePackage/PackageRuntime.cs".to_string()));
        assert!(
            rel_paths
                .contains(&"Library/PackageCache/UnityPackage/PackageCacheRuntime.cs".to_string())
        );
        assert!(!rel_paths.contains(&"Assets/Scripts/Editor/EditorOnly.cs".to_string()));
        assert!(!rel_paths.contains(&"Library/Other/Skipped.cs".to_string()));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nested_git_worktree_is_scanned_as_source_tree() {
        let root = std::env::temp_dir().join(format!("codebase_mcp_submodule_{}", now_ms()));
        let nested = root.join("Packages").join("NestedRepo");
        std::fs::create_dir_all(nested.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git").join("info")).unwrap();
        std::fs::write(
            nested.join(".git"),
            "gitdir: ../../.git/modules/Packages/NestedRepo",
        )
        .unwrap();
        std::fs::write(
            root.join(".git").join("info").join("exclude"),
            "Packages/NestedRepo/\n",
        )
        .unwrap();
        std::fs::write(
            nested.join("src").join("Nested.cs"),
            "public class Nested {}",
        )
        .unwrap();

        let mut options = IndexOptions::default();
        options.extensions = vec!["cs".to_string()];
        options.include_paths = Vec::new();
        options.skip_dirs = vec![".git".to_string()];
        options.respect_gitignore = true;

        let paths = collect_paths(&root, &options).unwrap();
        let rel_paths = paths
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        assert!(rel_paths.contains(&"Packages/NestedRepo/src/Nested.cs".to_string()));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependencies_include_fully_qualified_types() {
        let files = vec![
            file(
                "Packages/Common/ResourceTypeDefine.cs",
                r#"
namespace ELEX.Resource
{
    public enum ResourceType
    {
        ModelPrefab
    }
}
"#,
            ),
            file(
                "Packages/Common/AssetsManager.cs",
                r#"
namespace libx
{
    public sealed class AssetsManager
    {
    }
}
"#,
            ),
            file(
                "Assets/GameObjectPoolMgr.cs",
                r#"
namespace Game
{
    public class GameObjectPoolMgr
    {
        public void Spawn(ELEX.Resource.ResourceType resourceType)
        {
            libx.AssetsManager.LoadAssetAsync("name", resourceType);
        }
    }
}
"#,
            ),
        ];

        let deps = dependency_paths(files, "Assets/GameObjectPoolMgr.cs");
        assert!(deps.contains(&"Packages/Common/ResourceTypeDefine.cs".to_string()));
        assert!(deps.contains(&"Packages/Common/AssetsManager.cs".to_string()));
    }

    #[test]
    fn dependencies_include_alias_static_using_and_attribute_suffix() {
        let files = vec![
            file(
                "Packages/Lib/Service.cs",
                r#"
namespace Lib
{
    public class Service
    {
    }
}
"#,
            ),
            file(
                "Packages/Lib/StaticUtil.cs",
                r#"
namespace Lib
{
    public static class StaticUtil
    {
    }
}
"#,
            ),
            file(
                "Packages/Meta/FooAttribute.cs",
                r#"
namespace Game.Meta
{
    public sealed class FooAttribute : System.Attribute
    {
    }
}
"#,
            ),
            file(
                "Assets/Consumer.cs",
                r#"
using AliasService = Lib.Service;
using static Lib.StaticUtil;
using Game.Meta;

namespace Game.App
{
    [Foo]
    public class Consumer : AliasService
    {
        public void M()
        {
            AliasService.Run();
            Helper();
        }
    }
}
"#,
            ),
        ];

        let deps = dependency_paths(files, "Assets/Consumer.cs");
        assert!(deps.contains(&"Packages/Lib/Service.cs".to_string()));
        assert!(deps.contains(&"Packages/Lib/StaticUtil.cs".to_string()));
        assert!(deps.contains(&"Packages/Meta/FooAttribute.cs".to_string()));
    }

    #[test]
    fn dependencies_include_types_whose_names_do_not_match_file_stem() {
        let files = vec![
            file(
                "Assets/Services/BetaService.cs",
                r#"
namespace Game.Core
{
    public class DeltaService
    {
    }
}
"#,
            ),
            file(
                "Assets/Consumer.cs",
                r#"
using Game.Core;

namespace Game.App
{
    public class Consumer
    {
        private DeltaService service;
    }
}
"#,
            ),
        ];

        let deps = dependency_paths(files, "Assets/Consumer.cs");
        assert!(deps.contains(&"Assets/Services/BetaService.cs".to_string()));
    }

    #[test]
    fn java_dependencies_include_imported_same_package_and_qualified_types() {
        let files = vec![
            java_file(
                "src/main/java/com/acme/core/UserService.java",
                r#"
package com.acme.core;

public class UserService {
}
"#,
            ),
            java_file(
                "src/main/java/com/acme/core/InternalType.java",
                r#"
package com.acme.core;

public class InternalType {
}
"#,
            ),
            java_file(
                "src/main/java/com/acme/app/App.java",
                r#"
package com.acme.app;

import com.acme.core.UserService;

public class App {
    private UserService service;
    private com.acme.core.InternalType internalType;
}
"#,
            ),
        ];

        let deps = dependency_paths(files, "src/main/java/com/acme/app/App.java");
        assert!(deps.contains(&"src/main/java/com/acme/core/UserService.java".to_string()));
        assert!(deps.contains(&"src/main/java/com/acme/core/InternalType.java".to_string()));
    }

    #[test]
    fn java_dependencies_include_wildcard_imports() {
        let files = vec![
            java_file(
                "src/main/java/com/acme/core/Widget.java",
                r#"
package com.acme.core;

public class Widget {
}
"#,
            ),
            java_file(
                "src/main/java/com/acme/app/App.java",
                r#"
package com.acme.app;

import com.acme.core.*;

public class App {
    private Widget widget;
}
"#,
            ),
        ];

        let deps = dependency_paths(files, "src/main/java/com/acme/app/App.java");
        assert!(deps.contains(&"src/main/java/com/acme/core/Widget.java".to_string()));
    }

    #[test]
    fn rust_dependencies_include_use_declarations() {
        let files = vec![
            rust_file(
                "src/core.rs",
                r#"
pub struct Engine;
pub trait Runner {
    fn run(&self);
}
"#,
            ),
            rust_file(
                "src/app.rs",
                r#"
use crate::core::{Engine, Runner};

pub struct App {
    engine: Engine,
}

impl Runner for App {
    fn run(&self) {}
}
"#,
            ),
        ];

        let deps = dependency_paths(files, "src/app.rs");
        assert!(deps.contains(&"src/core.rs".to_string()));
    }

    #[test]
    fn rust_dependencies_include_mod_file_declarations() {
        let files = vec![
            rust_file(
                "src/lib.rs",
                r#"
pub mod guide;
"#,
            ),
            rust_file(
                "src/guide.rs",
                r#"
pub struct GuideType;
"#,
            ),
        ];

        let deps = dependency_paths(files, "src/lib.rs");
        assert!(deps.contains(&"src/guide.rs".to_string()));
    }

    #[test]
    fn lua_dependencies_include_required_modules() {
        let files = vec![
            lua_file(
                "scripts/main.lua",
                r#"
local player = require("game.player")

local function start()
    return player.new()
end
"#,
            ),
            lua_file(
                "scripts/game/player.lua",
                r#"
local M = {}

function M.new()
    return M
end

return M
"#,
            ),
        ];

        let deps = dependency_paths(files, "scripts/main.lua");
        assert!(deps.contains(&"scripts/game/player.lua".to_string()));
    }
}
