use crate::bm25::{Bm25Builder, Bm25Index};
use crate::cache::{
    CacheWriteTransaction, CachedIndexPayload, ProjectCache, SourceFingerprint, read_deps_forward,
    read_word_index,
};
use crate::event_log;
use crate::graph::CodeGraph;
use crate::language::{
    analyze_source, chunk_source_metadata, language_for_extension, mask_comments,
};
use crate::text_search::{
    TextSearchIndex, read_text_search_index, source_hash as text_search_source_hash,
};
use crate::tokens::{raw_identifiers, split_identifier};
use crate::types::{Chunk, FileEntry, SearchHit, Symbol, WordHit, WordIndex};
use anyhow::{Context, Result, anyhow};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{WalkBuilder, WalkState};
use rayon::prelude::*;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
use crate::language::{analyze_symbols, chunk_source, parse_imports, parse_namespace};

const DEFAULT_MAX_FILE_BYTES: u64 = 50_000_000;
const TEXT_LINE_CACHE_LIMIT: usize = 64;
const TEXT_LINE_CACHE_MAX_HITS_PER_ENTRY: usize = 2_048;

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub extensions: Vec<String>,
    pub max_file_bytes: u64,
    pub respect_gitignore: bool,
    pub root_paths: Vec<String>,
    pub include_paths: Vec<String>,
    pub exclude_paths: Vec<String>,
    pub skip_dirs: Vec<String>,
    pub diagnostics: DiagnosticsOptions,
    pub storage: StorageOptions,
}

impl IndexOptions {
    pub fn cache_identity_eq(&self, other: &Self) -> bool {
        self.extensions == other.extensions
            && self.max_file_bytes == other.max_file_bytes
            && self.respect_gitignore == other.respect_gitignore
            && self.root_paths == other.root_paths
            && self.include_paths == other.include_paths
            && self.exclude_paths == other.exclude_paths
            && self.skip_dirs == other.skip_dirs
            && self.storage.enabled == other.storage.enabled
            && self.storage.dir == other.storage.dir
    }
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
            respect_gitignore: true,
            root_paths: Vec::new(),
            include_paths: Vec::new(),
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
                "temp".to_string(),
                "logs".to_string(),
                "obj".to_string(),
                "bin".to_string(),
                "build".to_string(),
                "builds".to_string(),
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
    symbol_lookup: HashMap<String, Vec<(String, usize)>>,
    pub chunks: Vec<Chunk>,
    pub word_index: parking_lot::RwLock<Option<WordIndex>>,
    pub word_index_path: Option<PathBuf>,
    pub word_hits_path: Option<PathBuf>,
    pub text_search_index: parking_lot::RwLock<Option<TextSearchIndex>>,
    pub text_search_index_path: Option<PathBuf>,
    text_line_cache: parking_lot::Mutex<TextLineCache>,
    pub deps_forward: parking_lot::RwLock<Option<HashMap<String, Vec<String>>>>,
    pub deps_path: Option<PathBuf>,
    pub deps_reverse: parking_lot::RwLock<Option<HashMap<String, Vec<String>>>>,
    pub graph_stats: LightweightGraphStats,
    pub graph: parking_lot::RwLock<Option<Arc<CodeGraph>>>,
    bm25: parking_lot::RwLock<Option<Bm25Index>>,
    pub changed_files: Vec<ChangedFile>,
    pub storage_dir: Option<String>,
    pub cache_status: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TextLineCacheKey {
    query: String,
    max_results: usize,
    regex: bool,
    path_glob: Option<String>,
    compact: bool,
    include_scope: bool,
}

#[derive(Default)]
struct TextLineCache {
    order: VecDeque<TextLineCacheKey>,
    entries: HashMap<TextLineCacheKey, Arc<Vec<SearchHit>>>,
}

impl TextLineCache {
    fn get(&mut self, key: &TextLineCacheKey) -> Option<Vec<SearchHit>> {
        let hits = self.entries.get(key)?.clone();
        self.order.retain(|current| current != key);
        self.order.push_back(key.clone());
        Some((*hits).clone())
    }

    fn insert(&mut self, key: TextLineCacheKey, hits: Vec<SearchHit>) {
        if hits.len() > TEXT_LINE_CACHE_MAX_HITS_PER_ENTRY {
            return;
        }
        self.order.retain(|current| current != &key);
        self.order.push_back(key.clone());
        self.entries.insert(key, Arc::new(hits));
        while self.order.len() > TEXT_LINE_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }
}

impl TextLineCacheKey {
    fn new(
        query: &str,
        max_results: usize,
        regex: bool,
        path_glob: Option<&str>,
        compact: bool,
        include_scope: bool,
    ) -> Self {
        Self {
            query: query.to_string(),
            max_results,
            regex,
            path_glob: path_glob.map(str::to_string),
            compact,
            include_scope,
        }
    }
}

fn build_symbol_lookup(
    files: &BTreeMap<String, FileEntry>,
) -> HashMap<String, Vec<(String, usize)>> {
    let mut lookup = HashMap::<String, Vec<(String, usize)>>::new();
    for (path, file) in files {
        for (symbol_idx, symbol) in file.symbols.iter().enumerate() {
            lookup
                .entry(symbol.name.clone())
                .or_default()
                .push((path.clone(), symbol_idx));
        }
    }
    lookup
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
        let mut cache_write = project_cache.begin_write()?;
        let storage_dir = project_cache
            .enabled()
            .then(|| project_cache.dir().display().to_string());
        if project_cache.enabled() {
            let stage = Instant::now();
            match project_cache.load(&options) {
                Ok(Some(payload)) => {
                    log_timing(timing, "load_project_cache", stage);
                    return Self::from_cached(
                        root,
                        options,
                        payload,
                        storage_dir,
                        Some(project_cache.word_index_path()),
                        Some(project_cache.word_hits_path()),
                        Some(project_cache.text_search_index_path()),
                        Some(project_cache.bm25_postings_path()),
                        project_cache.current_deps_path()?,
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
            let stage = Instant::now();
            match project_cache.load_incremental_base(&options) {
                Ok(Some(payload)) => {
                    log_timing(timing, "load_incremental_cache_base", stage);
                    return Self::index_incremental(
                        root,
                        options,
                        project_cache,
                        cache_write,
                        storage_dir,
                        payload,
                        total_start,
                    );
                }
                Ok(None) => {
                    log_timing(timing, "load_incremental_cache_base_miss", stage);
                }
                Err(err) => {
                    eprintln!(
                        "codebase-mcp incremental cache ignored at {}: {err:#}",
                        project_cache.dir().display()
                    );
                    log_timing(timing, "load_incremental_cache_base_error", stage);
                }
            }
        }

        let stage = Instant::now();
        let paths = collect_paths_for_incremental(&root, &options)?;
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
        if let Some(transaction) = cache_write.as_ref() {
            let stage = Instant::now();
            match transaction.save_deps_forward(&deps_forward) {
                Ok(()) => {
                    deps_path = Some(transaction.deps_path().to_path_buf());
                    deps_forward.clear();
                    deps_forward.shrink_to_fit();
                }
                Err(err) => eprintln!(
                    "codebase-mcp dependency sidecar save failed at {}: {err:#}",
                    transaction.deps_path().display()
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
        let bm25 = Bm25Index::default();
        strip_chunk_contents(&mut chunks);
        log_timing(timing, "bm25_deferred", stage);

        let mut word_index_path = None;
        let mut word_hits_path = None;
        strip_chunk_contents(&mut chunks);
        strip_chunk_paths(&mut chunks);
        if project_cache.enabled() && deps_path.is_some() && cache_write.is_some() {
            let stage = Instant::now();
            let transaction = cache_write.take().expect("cache transaction checked");
            let save_result =
                project_cache.save(transaction, &options, &files, &chunks, &bm25, graph_stats);
            if let Err(err) = save_result {
                eprintln!(
                    "codebase-mcp cache save failed at {}: {err:#}",
                    project_cache.dir().display()
                );
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
        let symbol_lookup = build_symbol_lookup(&file_map);

        Ok(Self {
            root,
            options,
            seq: now_ms() as u64,
            file_paths,
            files: file_map,
            symbol_lookup,
            chunks,
            word_index: parking_lot::RwLock::new(None),
            word_index_path,
            word_hits_path,
            text_search_index: parking_lot::RwLock::new(None),
            text_search_index_path: project_cache
                .enabled()
                .then(|| project_cache.text_search_index_path()),
            text_line_cache: parking_lot::Mutex::new(TextLineCache::default()),
            deps_forward: parking_lot::RwLock::new(if deps_path.is_some() {
                None
            } else {
                Some(deps_forward)
            }),
            deps_path,
            deps_reverse: parking_lot::RwLock::new(None),
            graph_stats,
            graph: parking_lot::RwLock::new(None),
            bm25: parking_lot::RwLock::new(None),
            changed_files: Vec::new(),
            storage_dir,
            cache_status: if project_cache.enabled() {
                "miss"
            } else {
                "disabled"
            },
        })
    }

    fn from_cached(
        root: PathBuf,
        options: IndexOptions,
        payload: CachedIndexPayload,
        storage_dir: Option<String>,
        word_index_path: Option<PathBuf>,
        word_hits_path: Option<PathBuf>,
        text_search_index_path: Option<PathBuf>,
        bm25_postings_path: Option<PathBuf>,
        deps_path: Option<PathBuf>,
        total_start: Instant,
    ) -> Result<Self> {
        let timing = options.diagnostics.timing;
        let stage = Instant::now();
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
        strip_chunk_paths(&mut chunks);
        log_timing(timing, "restore_cached_files", stage);

        let mut bm25 = payload.bm25;
        if let Some(path) = bm25_postings_path
            && path.is_file()
        {
            bm25.use_postings_file(path);
        }
        let bm25 = (!bm25.is_empty()).then_some(bm25);
        let mut file_map = files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let graph_stats = payload.graph_stats;
        log_timing(timing, "total", total_start);
        strip_file_contents(&mut file_map);
        let symbol_lookup = build_symbol_lookup(&file_map);

        Ok(Self {
            root,
            options,
            seq: now_ms() as u64,
            file_paths,
            files: file_map,
            symbol_lookup,
            chunks,
            word_index: parking_lot::RwLock::new(None),
            word_index_path,
            word_hits_path,
            text_search_index: parking_lot::RwLock::new(None),
            text_search_index_path,
            text_line_cache: parking_lot::Mutex::new(TextLineCache::default()),
            deps_forward: parking_lot::RwLock::new(None),
            deps_path,
            deps_reverse: parking_lot::RwLock::new(None),
            graph_stats,
            graph: parking_lot::RwLock::new(None),
            bm25: parking_lot::RwLock::new(bm25),
            changed_files: Vec::new(),
            storage_dir,
            cache_status: "hit",
        })
    }

    fn index_incremental(
        root: PathBuf,
        options: IndexOptions,
        project_cache: ProjectCache,
        mut cache_write: Option<CacheWriteTransaction>,
        storage_dir: Option<String>,
        payload: CachedIndexPayload,
        total_start: Instant,
    ) -> Result<Self> {
        let timing = options.diagnostics.timing;
        let stage = Instant::now();
        let old_file_entries = payload
            .files
            .into_iter()
            .map(|file| file.into_file_entry())
            .collect::<Vec<_>>();
        let old_file_paths = old_file_entries
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        let old_file_by_path = old_file_entries
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let old_chunk_count = payload.chunks.len();
        let mut old_chunks_by_file = HashMap::<String, Vec<(usize, Chunk)>>::new();
        for (old_idx, mut chunk) in payload.chunks.into_iter().enumerate() {
            let path = chunk_file_path(&chunk, &old_file_paths).to_string();
            chunk.file_path = path.clone();
            old_chunks_by_file
                .entry(path)
                .or_default()
                .push((old_idx, chunk));
        }
        drop(payload.bm25);
        log_timing(timing, "incremental_restore_old_cache", stage);

        let stage = Instant::now();
        let paths = collect_paths_for_incremental(&root, &options)?;
        let fingerprints = fingerprint_paths_incremental(
            &root,
            &paths,
            &old_file_by_path,
            options.max_file_bytes,
        )?;
        log_timing(timing, "incremental_fingerprint", stage);

        let stage = Instant::now();
        let mut changed_paths = Vec::<String>::new();
        let mut unchanged_paths = HashSet::<String>::new();
        for fingerprint in &fingerprints {
            match old_file_by_path.get(&fingerprint.path) {
                Some(old) if old.content_hash == fingerprint.content_hash => {
                    unchanged_paths.insert(fingerprint.path.clone());
                }
                _ => changed_paths.push(fingerprint.path.clone()),
            }
        }
        let changed_path_set = changed_paths.iter().cloned().collect::<HashSet<_>>();
        let mut parsed_changed = changed_paths
            .par_iter()
            .filter_map(|path| {
                read_indexed_file_source(
                    &root,
                    &root.join(path),
                    options.max_file_bytes,
                    options.diagnostics.slow_file_ms,
                )
                .ok()
                .map(|indexed| (path.clone(), indexed))
            })
            .collect::<HashMap<_, _>>();
        log_timing(timing, "incremental_parse_changed_files", stage);

        let stage = Instant::now();
        let mut old_to_new_doc = vec![None; old_chunk_count];
        let mut files = Vec::<FileEntry>::with_capacity(fingerprints.len());
        let mut chunks = Vec::<Chunk>::new();
        for fingerprint in &fingerprints {
            if unchanged_paths.contains(&fingerprint.path) {
                if let Some(file) = old_file_by_path.get(&fingerprint.path) {
                    files.push(file.clone());
                }
                if let Some(old_chunks) = old_chunks_by_file.remove(&fingerprint.path) {
                    for (old_idx, mut chunk) in old_chunks {
                        let new_idx = chunks.len();
                        if let Some(slot) = old_to_new_doc.get_mut(old_idx) {
                            *slot = Some(new_idx);
                        }
                        chunk.id = new_idx;
                        chunk.file_path = fingerprint.path.clone();
                        chunks.push(chunk);
                    }
                }
            } else if let Some(mut indexed) = parsed_changed.remove(&fingerprint.path) {
                for mut chunk in indexed.chunks.drain(..) {
                    chunk.id = chunks.len();
                    chunks.push(chunk);
                }
                files.push(indexed.file);
            }
        }
        let file_paths = files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        assign_chunk_file_ids(&mut chunks, &file_paths);
        let chunk_indices_by_file = build_chunk_indices_by_file(&chunks, &file_paths);
        log_timing(timing, "incremental_merge_files", stage);

        let stage = Instant::now();
        let mut deps_forward = match project_cache.load_incremental_deps(&options) {
            Ok(Some(old_deps_forward)) => merge_incremental_dependencies(
                &root,
                &files,
                &chunks,
                &chunk_indices_by_file,
                &changed_path_set,
                old_deps_forward,
            ),
            Ok(None) => {
                let dependency_symbols = build_dependency_symbols(&files);
                let dependency_identifiers =
                    build_dependency_references_from_sources(&root, &files, &dependency_symbols);
                let (deps_forward, _deps_reverse) = build_dependencies(
                    Some(&root),
                    &files,
                    &chunks,
                    &chunk_indices_by_file,
                    &dependency_symbols,
                    &dependency_identifiers,
                );
                deps_forward
            }
            Err(err) => {
                eprintln!("codebase-mcp incremental dependency sidecar ignored: {err:#}");
                let dependency_symbols = build_dependency_symbols(&files);
                let dependency_identifiers =
                    build_dependency_references_from_sources(&root, &files, &dependency_symbols);
                let (deps_forward, _deps_reverse) = build_dependencies(
                    Some(&root),
                    &files,
                    &chunks,
                    &chunk_indices_by_file,
                    &dependency_symbols,
                    &dependency_identifiers,
                );
                deps_forward
            }
        };
        log_timing(timing, "incremental_dependencies", stage);

        let graph_stats = estimate_graph_stats(&files, &deps_forward);
        let mut deps_path = None;
        if let Some(transaction) = cache_write.as_ref() {
            let stage = Instant::now();
            match transaction.save_deps_forward(&deps_forward) {
                Ok(()) => {
                    deps_path = Some(transaction.deps_path().to_path_buf());
                    deps_forward.clear();
                    deps_forward.shrink_to_fit();
                }
                Err(err) => eprintln!(
                    "codebase-mcp dependency sidecar save failed at {}: {err:#}",
                    transaction.deps_path().display()
                ),
            }
            log_timing(timing, "incremental_save_deps_sidecar", stage);
        }

        let stage = Instant::now();
        let bm25 = Bm25Index::default();
        log_timing(timing, "incremental_bm25_deferred", stage);

        strip_chunk_contents(&mut chunks);
        strip_chunk_paths(&mut chunks);
        if project_cache.enabled() && deps_path.is_some() && cache_write.is_some() {
            let stage = Instant::now();
            let transaction = cache_write.take().expect("cache transaction checked");
            let save_result =
                project_cache.save(transaction, &options, &files, &chunks, &bm25, graph_stats);
            if let Err(err) = save_result {
                eprintln!(
                    "codebase-mcp incremental cache save failed at {}: {err:#}",
                    project_cache.dir().display()
                );
            }
            log_timing(timing, "incremental_save_project_cache", stage);
        }

        let word_index_path = project_cache
            .enabled()
            .then(|| project_cache.word_index_path());
        let word_hits_path = project_cache
            .enabled()
            .then(|| project_cache.word_hits_path());
        let text_search_index_path = project_cache
            .enabled()
            .then(|| project_cache.text_search_index_path());
        let mut file_map = files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        log_timing(timing, "total", total_start);
        strip_file_contents(&mut file_map);
        let symbol_lookup = build_symbol_lookup(&file_map);

        Ok(Self {
            root,
            options,
            seq: now_ms() as u64,
            file_paths,
            files: file_map,
            symbol_lookup,
            chunks,
            word_index: parking_lot::RwLock::new(None),
            word_index_path,
            word_hits_path,
            text_search_index: parking_lot::RwLock::new(None),
            text_search_index_path,
            text_line_cache: parking_lot::Mutex::new(TextLineCache::default()),
            deps_forward: parking_lot::RwLock::new(if deps_path.is_some() {
                None
            } else {
                Some(deps_forward)
            }),
            deps_path,
            deps_reverse: parking_lot::RwLock::new(None),
            graph_stats,
            graph: parking_lot::RwLock::new(None),
            bm25: parking_lot::RwLock::new(None),
            changed_files: Vec::new(),
            storage_dir,
            cache_status: "incremental",
        })
    }

    pub fn update_known_paths(
        &self,
        changed_paths: &[String],
        deleted_paths: &[String],
    ) -> Result<Self> {
        let timing = self.options.diagnostics.timing;
        let total_start = Instant::now();
        let stage = Instant::now();
        let root = self.root.clone();
        let options = self.options.clone();
        let mut requested_deleted = deleted_paths
            .iter()
            .map(|path| normalize_rel_path(path))
            .collect::<HashSet<_>>();
        let requested_changed = changed_paths
            .iter()
            .map(|path| normalize_rel_path(path))
            .filter(|path| !path.is_empty())
            .collect::<BTreeSet<_>>();
        for path in &requested_changed {
            requested_deleted.remove(path);
        }

        let old_files = self
            .file_paths
            .iter()
            .filter_map(|path| self.files.get(path).cloned())
            .collect::<Vec<_>>();
        let old_file_by_path = old_files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let mut old_chunks_by_file = HashMap::<String, Vec<(usize, Chunk)>>::new();
        for (old_idx, mut chunk) in self.chunks.iter().cloned().enumerate() {
            let path = self.chunk_file_path(&chunk).to_string();
            chunk.file_path = path.clone();
            old_chunks_by_file
                .entry(path)
                .or_default()
                .push((old_idx, chunk));
        }
        let old_chunk_count = self.chunks.len();
        log_timing(timing, "live_incremental_restore_old", stage);

        let stage = Instant::now();
        let parsed_changed = requested_changed
            .par_iter()
            .filter_map(|path| {
                let absolute = root.join(path);
                if !absolute.is_file() {
                    return None;
                }
                read_indexed_file_source(
                    &root,
                    &absolute,
                    options.max_file_bytes,
                    options.diagnostics.slow_file_ms,
                )
                .ok()
                .map(|indexed| (path.clone(), indexed))
            })
            .collect::<HashMap<_, _>>();
        let changed_existing = parsed_changed.keys().cloned().collect::<HashSet<_>>();
        log_timing(timing, "live_incremental_parse_changed", stage);

        let stage = Instant::now();
        let mut files_by_path = old_file_by_path;
        for path in requested_deleted {
            files_by_path.remove(&path);
        }
        for path in &requested_changed {
            if !changed_existing.contains(path) {
                files_by_path.remove(path);
            }
        }
        for (path, indexed) in &parsed_changed {
            files_by_path.insert(path.clone(), indexed.file.clone());
        }

        let file_paths = files_by_path.keys().cloned().collect::<Vec<_>>();
        let mut files = files_by_path.into_values().collect::<Vec<_>>();
        let mut _old_to_new_doc = vec![None; old_chunk_count];
        let mut chunks = Vec::<Chunk>::new();
        for file in &files {
            if let Some(indexed) = parsed_changed.get(&file.path) {
                for mut chunk in indexed.chunks.iter().cloned() {
                    chunk.id = chunks.len();
                    chunks.push(chunk);
                }
            } else if let Some(old_chunks) = old_chunks_by_file.remove(&file.path) {
                for (old_idx, mut chunk) in old_chunks {
                    let new_idx = chunks.len();
                    if let Some(slot) = _old_to_new_doc.get_mut(old_idx) {
                        *slot = Some(new_idx);
                    }
                    chunk.id = new_idx;
                    chunk.file_path = file.path.clone();
                    chunks.push(chunk);
                }
            }
        }
        assign_chunk_file_ids(&mut chunks, &file_paths);
        let chunk_indices_by_file = build_chunk_indices_by_file(&chunks, &file_paths);
        log_timing(timing, "live_incremental_merge_files", stage);

        let stage = Instant::now();
        let old_deps_forward = self.deps_forward_snapshot();
        let deps_forward = merge_incremental_dependencies(
            &root,
            &files,
            &chunks,
            &chunk_indices_by_file,
            &changed_existing,
            old_deps_forward,
        );
        let graph_stats = estimate_graph_stats(&files, &deps_forward);
        log_timing(timing, "live_incremental_dependencies", stage);

        let stage = Instant::now();
        log_timing(timing, "live_incremental_bm25_deferred", stage);

        strip_chunk_contents(&mut chunks);
        strip_chunk_paths(&mut chunks);
        let mut file_map = files
            .drain(..)
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        strip_file_contents(&mut file_map);
        let symbol_lookup = build_symbol_lookup(&file_map);
        log_timing(timing, "live_incremental_finish", stage);
        log_timing(timing, "total", total_start);

        Ok(Self {
            root,
            options,
            seq: now_ms() as u64,
            file_paths,
            files: file_map,
            symbol_lookup,
            chunks,
            word_index: parking_lot::RwLock::new(None),
            word_index_path: None,
            word_hits_path: None,
            text_search_index: parking_lot::RwLock::new(None),
            text_search_index_path: None,
            text_line_cache: parking_lot::Mutex::new(TextLineCache::default()),
            deps_forward: parking_lot::RwLock::new(Some(deps_forward)),
            deps_path: None,
            deps_reverse: parking_lot::RwLock::new(None),
            graph_stats,
            graph: parking_lot::RwLock::new(None),
            bm25: parking_lot::RwLock::new(None),
            changed_files: Vec::new(),
            storage_dir: self.storage_dir.clone(),
            cache_status: "live-incremental",
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

    pub fn ranked_bm25_chunks(
        &self,
        query: &str,
        top_k: usize,
        selector: Option<&[usize]>,
    ) -> Result<Vec<(usize, f32)>> {
        {
            let guard = self.bm25.read();
            if let Some(index) = guard.as_ref()
                && !index.is_empty()
            {
                return index.query(query, top_k, selector);
            }
        }

        let mut guard = self.bm25.write();
        let needs_build = match guard.as_ref() {
            Some(index) => index.is_empty(),
            None => true,
        };
        if needs_build {
            let chunk_indices_by_file = build_chunk_indices_by_file(&self.chunks, &self.file_paths);
            let built = build_full_bm25_in_memory(
                &self.root,
                &self.chunks,
                &self.file_paths,
                &chunk_indices_by_file,
            )?;
            self.persist_lazy_bm25(&built);
            *guard = Some(built);
        }

        match guard.as_ref() {
            Some(index) => index.query(query, top_k, selector),
            None => Ok(Vec::new()),
        }
    }

    fn persist_lazy_bm25(&self, bm25: &Bm25Index) {
        let Ok(cache) = ProjectCache::new(&self.root, &self.options.storage) else {
            return;
        };
        if cache.enabled()
            && let Err(err) = cache.save_bm25_index(bm25)
        {
            eprintln!("codebase-mcp BM25 cache save failed: {err:#}");
        }
    }

    pub fn graph(&self) -> Arc<CodeGraph> {
        if let Some(graph) = self.graph.read().as_ref().cloned() {
            return graph;
        }
        let mut guard = self.graph.write();
        if let Some(graph) = guard.as_ref().cloned() {
            return graph;
        }
        if let Ok(cache) = ProjectCache::new(&self.root, &self.options.storage)
            && let Ok(Some(graph)) = cache.load_graph(&self.options)
        {
            let graph = Arc::new(graph);
            *guard = Some(graph.clone());
            return graph;
        }
        let deps_forward = self.deps_forward_snapshot();
        let graph = Arc::new(CodeGraph::build(&self.files, &deps_forward));
        if let Ok(cache) = ProjectCache::new(&self.root, &self.options.storage)
            && let Err(err) = cache.save_graph(&self.options, graph.as_ref())
        {
            eprintln!("codebase-mcp graph cache save failed: {err:#}");
        }
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

    fn loaded_word_hits(&self, word: &str) -> Result<Option<Vec<WordHit>>> {
        let guard = self.word_index.read();
        let Some(index) = guard.as_ref() else {
            return Ok(None);
        };
        index.hits(word).map(Some)
    }

    fn ensure_word_index(&self) {
        if self.word_index.read().is_some() {
            return;
        }
        let mut guard = self.word_index.write();
        if guard.is_some() {
            return;
        }
        let hash = text_search_source_hash(&self.file_paths, &self.files);
        let index = match (&self.word_index_path, &self.word_hits_path) {
            (Some(index_path), Some(hits_path)) if index_path.is_file() && hits_path.is_file() => {
                match read_word_index(index_path, hits_path, &hash, self.file_paths.len()) {
                    Ok(Some(index)) => index,
                    Ok(None) => build_word_index_from_sources(&self.root, &self.file_paths)
                        .unwrap_or_default(),
                    Err(err) => {
                        eprintln!(
                            "codebase-mcp word index cache load failed at {}: {err:#}",
                            index_path.display()
                        );
                        build_word_index_from_sources(&self.root, &self.file_paths)
                            .unwrap_or_default()
                    }
                }
            }
            _ => build_word_index_from_sources(&self.root, &self.file_paths).unwrap_or_default(),
        };
        let mut index = index;
        if let (Some(index_path), Some(hits_path)) = (&self.word_index_path, &self.word_hits_path) {
            if !index.validate_source(&hash, self.file_paths.len())
                || !index_path.is_file()
                || !hits_path.is_file()
            {
                if let Ok(cache) = ProjectCache::new(&self.root, &self.options.storage) {
                    if let Err(err) =
                        cache.save_word_index(&mut index, hash.clone(), self.file_paths.len())
                    {
                        eprintln!("codebase-mcp word index cache save failed: {err:#}");
                    }
                }
            }
        }
        *guard = Some(index);
    }

    fn ensure_text_search_index(&self) {
        if self.text_search_index.read().is_some() {
            return;
        }
        let mut guard = self.text_search_index.write();
        if guard.is_some() {
            return;
        }
        let hash = text_search_source_hash(&self.file_paths, &self.files);
        let cached_index = match &self.text_search_index_path {
            Some(path) if path.is_file() => {
                read_text_search_index(path, &hash, self.file_paths.len()).unwrap_or_else(|err| {
                    eprintln!(
                        "codebase-mcp text search cache load failed at {}: {err:#}",
                        path.display()
                    );
                    None
                })
            }
            _ => None,
        };
        let loaded_from_cache = cached_index.is_some();
        let index = cached_index.unwrap_or_else(|| {
            TextSearchIndex::build(&self.root, &self.files, &self.file_paths, hash.clone())
                .unwrap_or_else(|err| {
                    eprintln!("codebase-mcp text search index build failed: {err:#}");
                    TextSearchIndex::empty(self.file_paths.len())
                })
        });
        if let Ok(cache) = ProjectCache::new(&self.root, &self.options.storage) {
            if cache.enabled() && !loaded_from_cache {
                if let Err(err) = cache.save_text_search_index(&index) {
                    eprintln!("codebase-mcp text search cache save failed: {err:#}");
                }
            }
        }
        *guard = Some(index);
    }

    pub fn text_line_hits(
        &self,
        query: &str,
        max_results: usize,
        regex: bool,
        path_glob: Option<&str>,
        compact: bool,
        include_scope: bool,
    ) -> Result<Vec<SearchHit>> {
        let cache_key =
            TextLineCacheKey::new(query, max_results, regex, path_glob, compact, include_scope);
        if let Some(hits) = self.text_line_cache.lock().get(&cache_key) {
            return Ok(hits);
        }
        let hits = self.text_line_hits_uncached(
            query,
            max_results,
            regex,
            path_glob,
            compact,
            include_scope,
        )?;
        self.text_line_cache.lock().insert(cache_key, hits.clone());
        Ok(hits)
    }

    fn text_line_hits_uncached(
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
        let mut seen = HashSet::<(u32, usize)>::new();

        if !regex && is_single_identifier_query(query) {
            if let Some(word_hits) = self.loaded_word_hits(query)?
                && !word_hits.is_empty()
            {
                let mut lines_by_file = HashMap::<u32, Vec<u32>>::new();
                for hit in word_hits {
                    lines_by_file.entry(hit.file_id).or_default().push(hit.line);
                }
                let mut groups = lines_by_file.into_iter().collect::<Vec<_>>();
                groups.sort_by(|(a_id, a_lines), (b_id, b_lines)| {
                    b_lines.len().cmp(&a_lines.len()).then_with(|| {
                        let a_size = self
                            .file_by_id(*a_id)
                            .map(|file| file.byte_size)
                            .unwrap_or(0);
                        let b_size = self
                            .file_by_id(*b_id)
                            .map(|file| file.byte_size)
                            .unwrap_or(0);
                        a_size.cmp(&b_size)
                    })
                });
                let per_file_cap = max_results;
                for (file_id, mut lines) in groups {
                    if hits.len() >= max_results {
                        return Ok(hits);
                    }
                    let Some(file) = self.file_by_id(file_id) else {
                        continue;
                    };
                    if globset
                        .as_ref()
                        .is_some_and(|glob| !glob.is_match(&file.path))
                    {
                        continue;
                    }
                    lines.sort_unstable();
                    lines.dedup();
                    self.append_text_hits_for_file(
                        file_id,
                        file,
                        query,
                        &lowered,
                        re.as_ref(),
                        Some(&lines),
                        per_file_cap,
                        max_results,
                        compact,
                        include_scope,
                        &mut seen,
                        &mut hits,
                    )?;
                }
                if hits.len() >= max_results {
                    return Ok(hits);
                }
            }
        }

        self.ensure_text_search_index();
        let guard = self.text_search_index.read();
        let Some(text_index) = guard.as_ref() else {
            return self.line_hits(query, max_results, regex, path_glob, compact, include_scope);
        };
        let candidate_ids = if regex {
            text_index.regex_candidate_file_ids(query)
        } else {
            text_index.candidate_file_ids(query)
        };
        let used_text_candidates = candidate_ids.is_some();
        let mut file_ids = match candidate_ids {
            Some(ids) => ids,
            None => (0..self.file_paths.len() as u32).collect(),
        };
        file_ids.sort_by(|a, b| {
            let a_size = self.file_by_id(*a).map(|file| file.byte_size).unwrap_or(0);
            let b_size = self.file_by_id(*b).map(|file| file.byte_size).unwrap_or(0);
            a_size.cmp(&b_size)
        });
        let per_file_cap = max_results;
        for file_id in file_ids {
            if hits.len() >= max_results {
                return Ok(hits);
            }
            let Some(file) = self.file_by_id(file_id) else {
                continue;
            };
            if globset
                .as_ref()
                .is_some_and(|glob| !glob.is_match(&file.path))
            {
                continue;
            }
            self.append_text_hits_for_file(
                file_id,
                file,
                query,
                &lowered,
                re.as_ref(),
                None,
                per_file_cap,
                max_results,
                compact,
                include_scope,
                &mut seen,
                &mut hits,
            )?;
        }

        if hits.len() < max_results && used_text_candidates {
            for file_id in &text_index.skipped_file_ids {
                if hits.len() >= max_results {
                    break;
                }
                let Some(file) = self.file_by_id(*file_id) else {
                    continue;
                };
                if globset
                    .as_ref()
                    .is_some_and(|glob| !glob.is_match(&file.path))
                {
                    continue;
                }
                self.append_text_hits_for_file(
                    *file_id,
                    file,
                    query,
                    &lowered,
                    re.as_ref(),
                    None,
                    max_results,
                    max_results,
                    compact,
                    include_scope,
                    &mut seen,
                    &mut hits,
                )?;
            }
        }

        Ok(hits)
    }

    #[allow(clippy::too_many_arguments)]
    fn append_text_hits_for_file(
        &self,
        file_id: u32,
        file: &FileEntry,
        query: &str,
        lowered_query: &str,
        re: Option<&Regex>,
        target_lines: Option<&[u32]>,
        per_file_cap: usize,
        max_results: usize,
        compact: bool,
        include_scope: bool,
        seen: &mut HashSet<(u32, usize)>,
        hits: &mut Vec<SearchHit>,
    ) -> Result<()> {
        let content = self.file_content(file)?;
        let mut target_idx = 0usize;
        let mut file_hits = 0usize;
        for (idx, line) in content.lines().enumerate() {
            let line_no = idx + 1;
            if let Some(lines) = target_lines {
                while target_idx < lines.len() && lines[target_idx] < line_no as u32 {
                    target_idx += 1;
                }
                if target_idx >= lines.len() {
                    break;
                }
                if lines[target_idx] != line_no as u32 {
                    continue;
                }
            }
            if compact && crate::language::is_comment_or_blank(line) {
                continue;
            }
            let matched = if let Some(re) = re {
                re.is_match(line)
            } else {
                line.to_ascii_lowercase().contains(lowered_query)
            };
            if !matched || !seen.insert((file_id, line_no)) {
                continue;
            }
            let scope = include_scope
                .then(|| crate::language::scope_for_line(&file.symbols, line_no))
                .flatten();
            hits.push(SearchHit {
                path: file.path.clone(),
                line: line_no,
                text: line.trim().to_string(),
                scope,
            });
            file_hits += 1;
            if hits.len() >= max_results || file_hits >= per_file_cap {
                break;
            }
        }
        let _ = query;
        Ok(())
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
        self.symbol_lookup
            .get(name)
            .into_iter()
            .flatten()
            .filter_map(|(path, symbol_idx)| {
                let file = self.files.get(path)?;
                Some((file, file.symbols.get(*symbol_idx)?))
            })
            .collect()
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
    event_log::timing("indexer", stage, start);
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

fn is_single_identifier_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return false;
    }
    let identifiers = raw_identifiers(trimmed);
    identifiers.len() == 1 && identifiers[0] == trimmed
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

pub fn collect_source_paths(root: &Path, options: &IndexOptions) -> Result<Vec<PathBuf>> {
    collect_paths(root, options)
}

pub fn source_watch_roots(root: &Path, options: &IndexOptions) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    if options.root_paths.is_empty() {
        roots.insert(root.to_path_buf());
    } else {
        for scan_root in &options.root_paths {
            let path = root.join(scan_root);
            if path.is_dir() {
                roots.insert(path);
            }
        }
    }
    for include_path in &options.include_paths {
        let path = root.join(include_path);
        if path.is_dir() {
            roots.insert(path);
        }
    }
    roots.into_iter().collect()
}

pub fn is_indexed_source_file(root: &Path, path: &Path, options: &IndexOptions) -> Result<bool> {
    let extensions = options
        .extensions
        .iter()
        .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return Ok(false);
    };
    if !extensions.contains(&ext.to_ascii_lowercase()) {
        return Ok(false);
    }
    if project_relative_path(root, path).is_none() {
        return Ok(false);
    }

    let include_root = options
        .include_paths
        .iter()
        .map(|include_path| root.join(include_path))
        .find(|include_root| path_is_same_or_under(include_root, path));
    if include_root.is_none() && !path_is_under_configured_roots(root, path, options) {
        return Ok(false);
    }

    let exclude_globs = build_optional_globset(&options.exclude_paths)?;
    Ok(!is_skipped_entry(
        root,
        path,
        false,
        include_root.as_deref(),
        options,
        exclude_globs.as_ref(),
    ))
}

fn path_is_under_configured_roots(root: &Path, path: &Path, options: &IndexOptions) -> bool {
    if options.root_paths.is_empty() {
        return path_is_same_or_under(root, path);
    }
    options
        .root_paths
        .iter()
        .map(|scan_root| root.join(scan_root))
        .any(|scan_root| path_is_same_or_under(&scan_root, path))
}

fn fingerprint_paths_incremental(
    root: &Path,
    paths: &[PathBuf],
    old_file_by_path: &BTreeMap<String, FileEntry>,
    max_file_bytes: u64,
) -> Result<Vec<SourceFingerprint>> {
    let mut fingerprints = paths
        .par_iter()
        .filter_map(|path| {
            fingerprint_path_incremental(root, path, old_file_by_path, max_file_bytes).ok()
        })
        .collect::<Vec<_>>();
    fingerprints.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(fingerprints)
}

fn fingerprint_path_incremental(
    root: &Path,
    path: &Path,
    old_file_by_path: &BTreeMap<String, FileEntry>,
    max_file_bytes: u64,
) -> Result<SourceFingerprint> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > max_file_bytes {
        return Err(anyhow!("file too large: {}", path.display()));
    }
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let modified_unix_ms = metadata_modified_unix_ms(&metadata);
    if let Some(old) = old_file_by_path.get(&rel)
        && old.byte_size == metadata.len() as usize
        && old.modified_unix_ms == modified_unix_ms
    {
        return Ok(SourceFingerprint {
            path: rel,
            byte_size: old.byte_size,
            modified_unix_ms: old.modified_unix_ms,
            content_hash: old.content_hash.clone(),
        });
    }
    let bytes = fs::read(path)?;
    if bytes.iter().take(8192).any(|b| *b == 0) {
        return Err(anyhow!("binary file skipped: {}", path.display()));
    }
    Ok(SourceFingerprint {
        path: rel,
        byte_size: bytes.len(),
        modified_unix_ms,
        content_hash: hash_bytes(&bytes),
    })
}

fn collect_paths_for_incremental(root: &Path, options: &IndexOptions) -> Result<Vec<PathBuf>> {
    collect_paths(root, options)
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

    let relative_text = include_root
        .and_then(|include_root| project_relative_path(include_root, path))
        .or_else(|| project_relative_path(root, path));
    let relative_fallback;
    let relative = if let Some(relative) = &relative_text {
        Path::new(relative)
    } else {
        relative_fallback = path.strip_prefix(root).unwrap_or(path);
        relative_fallback
    };
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
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(relative.to_string_lossy().replace('\\', "/"));
    }
    let root_text = normalized_absolute_path_text(root);
    let path_text = normalized_absolute_path_text(path);
    let root_cmp = comparable_path_text(&root_text);
    let path_cmp = comparable_path_text(&path_text);
    let root_cmp = root_cmp.trim_end_matches('/');
    let root_text = root_text.trim_end_matches('/');
    if path_cmp == root_cmp {
        return Some(String::new());
    }
    path_cmp
        .strip_prefix(&format!("{root_cmp}/"))
        .map(|_| path_text[root_text.len() + 1..].to_string())
}

fn path_is_same_or_under(root: &Path, path: &Path) -> bool {
    project_relative_path(root, path).is_some()
}

fn normalized_absolute_path_text(path: &Path) -> String {
    let mut text = path.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_string();
    }
    text
}

fn comparable_path_text(path: &str) -> String {
    if cfg!(windows) {
        path.to_ascii_lowercase()
    } else {
        path.to_string()
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
    let modified_unix_ms = metadata_modified_unix_ms(&metadata);

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

fn metadata_modified_unix_ms(metadata: &fs::Metadata) -> i128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i128)
        .unwrap_or(0)
}

fn build_dependency_references_from_sources(
    root: &Path,
    files: &[FileEntry],
    dependency_symbols: &HashMap<String, Vec<SymbolDefinition>>,
) -> HashMap<String, DependencyReferences> {
    files
        .par_iter()
        .fold(HashMap::new, |mut references_by_file, file| {
            let content;
            let source = if file.content.is_empty() {
                content = read_source_content(root, &file.path).unwrap_or_default();
                content.as_str()
            } else {
                file.content.as_str()
            };
            let active_source = mask_comments(file.language.as_str(), source);
            let mut dependency_references = DependencyReferences::default();
            let aliases = (file.language == "csharp")
                .then(|| parse_using_aliases_from_iter(active_source.lines()));
            if file.language == "csharp" {
                collect_static_using_dependency_references_from_iter(
                    active_source.lines(),
                    dependency_symbols,
                    &mut dependency_references,
                );
            }
            for line in active_source.lines() {
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
                let language = Path::new(rel_path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .and_then(language_for_extension);
                let active_content = language
                    .map(|language| mask_comments(language, &content))
                    .unwrap_or_else(|| content.to_string());
                for (line_idx, line) in active_content.lines().enumerate() {
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
            let active_content = if file.content.is_empty() {
                lines
                    .iter()
                    .map(|(_, line)| *line)
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                mask_comments(file.language.as_str(), &file.content)
            };
            let active_lines = active_content.lines().collect::<Vec<_>>();
            let aliases = (file.language == "csharp").then(|| {
                let numbered = active_lines
                    .iter()
                    .enumerate()
                    .map(|(idx, line)| (idx + 1, *line))
                    .collect::<Vec<_>>();
                parse_using_aliases_from_lines(&numbered)
            });
            if file.language == "csharp" {
                let numbered = active_lines
                    .iter()
                    .enumerate()
                    .map(|(idx, line)| (idx + 1, *line))
                    .collect::<Vec<_>>();
                collect_static_using_dependency_references_from_lines(
                    &numbered,
                    dependency_symbols,
                    &mut dependency_references,
                );
            }
            for (line_no, original_line) in lines {
                let line = active_lines
                    .get(line_no.saturating_sub(1))
                    .copied()
                    .unwrap_or(original_line);
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

fn build_dependency_symbols_for_names(
    files: &[FileEntry],
    names: &HashSet<String>,
) -> HashMap<String, Vec<SymbolDefinition>> {
    if names.is_empty() {
        return HashMap::new();
    }
    let mut symbols_by_name: HashMap<String, Vec<SymbolDefinition>> = HashMap::new();
    for file in files {
        let type_symbols = file
            .symbols
            .iter()
            .filter(|symbol| {
                names.contains(&symbol.name) && is_dependency_symbol_kind(symbol.kind.as_str())
            })
            .collect::<Vec<_>>();
        if type_symbols.is_empty() {
            continue;
        }
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

fn collect_dependency_symbol_names(root: &Path, files: &[FileEntry]) -> HashSet<String> {
    files
        .par_iter()
        .map(|file| {
            let content;
            let source = if file.content.is_empty() {
                content = read_source_content(root, &file.path).unwrap_or_default();
                content.as_str()
            } else {
                file.content.as_str()
            };
            let mut names = HashSet::new();
            for line in source.lines() {
                names.extend(raw_identifiers(line));
            }
            names
        })
        .reduce(HashSet::new, |mut left, right| {
            left.extend(right);
            left
        })
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

fn merge_incremental_dependencies(
    root: &Path,
    files: &[FileEntry],
    chunks: &[Chunk],
    chunk_indices_by_file: &HashMap<String, Vec<usize>>,
    changed_path_set: &HashSet<String>,
    old_deps_forward: HashMap<String, Vec<String>>,
) -> HashMap<String, Vec<String>> {
    let current_paths = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<HashSet<_>>();
    let mut merged = HashMap::<String, Vec<String>>::with_capacity(files.len());
    let removed_paths_present = old_deps_forward
        .keys()
        .any(|path| !current_paths.contains(path));
    for (path, deps) in old_deps_forward {
        if !current_paths.contains(&path) || changed_path_set.contains(&path) {
            continue;
        }
        if removed_paths_present {
            let filtered = deps
                .into_iter()
                .filter(|dep| current_paths.contains(dep))
                .collect::<Vec<_>>();
            merged.insert(path, filtered);
        } else {
            merged.insert(path, deps);
        }
    }

    let changed_files = files
        .iter()
        .filter(|file| changed_path_set.contains(&file.path))
        .cloned()
        .collect::<Vec<_>>();
    if !changed_files.is_empty() {
        let dependency_names = collect_dependency_symbol_names(root, &changed_files);
        let dependency_symbols = build_dependency_symbols_for_names(files, &dependency_names);
        let dependency_identifiers = if dependency_symbols.is_empty() {
            HashMap::new()
        } else {
            build_dependency_references_from_sources(root, &changed_files, &dependency_symbols)
        };
        let (changed_forward, _changed_reverse) = build_dependencies(
            Some(root),
            &changed_files,
            chunks,
            chunk_indices_by_file,
            &dependency_symbols,
            &dependency_identifiers,
        );
        merged.extend(changed_forward);
    }

    for path in current_paths {
        merged.entry(path).or_default();
    }
    merged
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

pub(crate) fn strip_strings_and_line_comment(line: &str) -> String {
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
        let language = Path::new(rel_path)
            .extension()
            .and_then(|ext| ext.to_str())
            .and_then(language_for_extension);
        let active_content = language
            .map(|language| mask_comments(language, &content))
            .unwrap_or(content);
        let lines = active_content.lines().collect::<Vec<_>>();
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

fn build_full_bm25_in_memory(
    root: &Path,
    chunks: &[Chunk],
    file_paths: &[String],
    chunk_indices_by_file: &HashMap<String, Vec<usize>>,
) -> Result<Bm25Index> {
    let mut builder = Bm25Builder::new();
    add_bm25_documents_from_sources(
        root,
        chunks,
        file_paths,
        chunk_indices_by_file,
        |document| {
            builder.add_document(document);
            Ok(())
        },
    )?;
    Ok(builder.finish())
}

fn read_source_content(root: &Path, rel_path: &str) -> Result<String> {
    let path = root.join(rel_path);
    let bytes =
        fs::read(&path).with_context(|| format!("failed to read source {}", path.display()))?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn append_text_tokens(tokens: &mut Vec<String>, text: &str) {
    for raw in raw_identifiers(text) {
        append_identifier_tokens(tokens, &raw, true);
    }
}

#[allow(dead_code)]
fn append_identifier_tokens(tokens: &mut Vec<String>, identifier: &str, filter_stopwords: bool) {
    tokens.extend(
        split_identifier(identifier)
            .into_iter()
            .filter(|token| is_bm25_token(token, filter_stopwords)),
    );
}

#[allow(dead_code)]
fn is_bm25_token(token: &str, filter_stopwords: bool) -> bool {
    token.len() > 1 && (!filter_stopwords || !is_bm25_code_stopword(token))
}

#[allow(dead_code)]
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
        let included_child = root.join("skipped_parent").join("included_child");
        let other_child = root.join("skipped_parent").join("other_child");
        std::fs::create_dir_all(&included_child).unwrap();
        std::fs::create_dir_all(&other_child).unwrap();
        std::fs::write(
            included_child.join("Included.cs"),
            "public class Included {}",
        )
        .unwrap();
        std::fs::write(other_child.join("Skipped.cs"), "public class Skipped {}").unwrap();

        let mut options = IndexOptions::default();
        options.extensions = vec!["cs".to_string()];
        options.include_paths = vec!["skipped_parent/included_child".to_string()];
        options.skip_dirs = vec!["skipped_parent".to_string()];

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
        assert!(rel_paths.contains(&"skipped_parent/included_child/Included.cs".to_string()));
        assert!(!rel_paths.contains(&"skipped_parent/other_child/Skipped.cs".to_string()));

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
    fn root_scope_combines_roots_includes_excludes_and_skips() {
        let root = std::env::temp_dir().join(format!("codebase_mcp_root_scope_{}", now_ms()));
        std::fs::create_dir_all(root.join("src").join("feature").join("excluded")).unwrap();
        std::fs::create_dir_all(root.join("plugins").join("shared")).unwrap();
        std::fs::create_dir_all(root.join("cache").join("included").join("shared_generated"))
            .unwrap();
        std::fs::create_dir_all(root.join("cache").join("other")).unwrap();
        std::fs::write(
            root.join("src").join("feature").join("Runtime.cs"),
            "public class RuntimeEntry {}",
        )
        .unwrap();
        std::fs::write(
            root.join("src")
                .join("feature")
                .join("excluded")
                .join("SkippedByGlob.cs"),
            "public class SkippedByGlob {}",
        )
        .unwrap();
        std::fs::write(
            root.join("plugins").join("shared").join("SharedRuntime.cs"),
            "public class SharedRuntime {}",
        )
        .unwrap();
        std::fs::write(
            root.join("cache")
                .join("included")
                .join("shared_generated")
                .join("IncludedFromSkippedParent.cs"),
            "public class IncludedFromSkippedParent {}",
        )
        .unwrap();
        std::fs::write(
            root.join("cache").join("other").join("SkippedByParent.cs"),
            "public class Skipped {}",
        )
        .unwrap();

        let mut options = IndexOptions::default();
        options.extensions = vec!["cs".to_string()];
        options.root_paths = vec![
            "src".to_string(),
            "plugins".to_string(),
            "cache/included".to_string(),
        ];
        options.include_paths = Vec::new();
        options.exclude_paths = vec!["**/excluded".to_string(), "**/excluded/**".to_string()];
        options.skip_dirs = vec!["cache".to_string(), ".git".to_string()];

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
        assert!(rel_paths.contains(&"src/feature/Runtime.cs".to_string()));
        assert!(rel_paths.contains(&"plugins/shared/SharedRuntime.cs".to_string()));
        assert!(
            rel_paths.contains(
                &"cache/included/shared_generated/IncludedFromSkippedParent.cs".to_string()
            )
        );
        assert!(!rel_paths.contains(&"src/feature/excluded/SkippedByGlob.cs".to_string()));
        assert!(!rel_paths.contains(&"cache/other/SkippedByParent.cs".to_string()));

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
