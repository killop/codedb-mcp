use crate::bm25::Bm25Index;
use crate::cache::{CachedIndexPayload, ProjectCache, SourceFingerprint};
use crate::embedding::MinishEmbeddingModel;
use crate::graph::CodeGraph;
use crate::language::{analyze_source, chunk_source, language_for_extension};
use crate::tokens::{raw_identifiers, tokenize};
use crate::types::{Chunk, FileEntry, SearchHit, SemanticUnit, Symbol, WordHit};
use crate::vector_store::MinishVectorStore;
use anyhow::{Context, Result, anyhow};
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use rayon::prelude::*;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(test)]
use crate::language::{analyze_symbols, parse_imports, parse_namespace};

const DEFAULT_MAX_FILE_BYTES: u64 = 50_000_000;

#[derive(Debug, Clone)]
pub struct IndexOptions {
    pub extensions: Vec<String>,
    pub max_file_bytes: u64,
    pub embedding_model: String,
    pub respect_gitignore: bool,
    pub include_paths: Vec<String>,
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
        "cs", "java", "py", "pyw", "js", "jsx", "mjs", "cjs", "ts", "tsx", "c", "h", "cc", "cpp",
        "cxx", "hpp", "hh", "hxx",
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
            embedding_model: "minishlab/potion-code-16M".to_string(),
            respect_gitignore: true,
            include_paths: vec!["Library/PackageCache".to_string()],
            skip_dirs: vec![
                ".git".to_string(),
                ".hg".to_string(),
                ".svn".to_string(),
                ".vs".to_string(),
                ".idea".to_string(),
                ".gradle".to_string(),
                "node_modules".to_string(),
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

pub struct Codebase {
    pub root: PathBuf,
    pub options: IndexOptions,
    pub seq: u64,
    pub files: BTreeMap<String, FileEntry>,
    pub chunks: Vec<Chunk>,
    pub semantic_units: Vec<SemanticUnit>,
    pub chunk_indices_by_file: HashMap<String, Vec<usize>>,
    pub symbol_definition_chunks: HashMap<String, Vec<usize>>,
    pub word_index: HashMap<String, Vec<WordHit>>,
    pub deps_forward: HashMap<String, Vec<String>>,
    pub deps_reverse: HashMap<String, Vec<String>>,
    pub graph: CodeGraph,
    pub bm25: Bm25Index,
    pub vectors: MinishVectorStore,
    pub model: MinishEmbeddingModel,
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

        let stage = Instant::now();
        let paths = collect_paths(&root, &options)?;
        log_timing(timing, "collect_paths", stage);
        let project_cache = ProjectCache::new(&root, &options.storage)?;
        let storage_dir = project_cache
            .enabled()
            .then(|| project_cache.dir().display().to_string());
        let cache_fingerprints = if project_cache.enabled() {
            let stage = Instant::now();
            let fingerprints = fingerprint_paths(&root, &paths, options.max_file_bytes)?;
            log_timing(timing, "fingerprint_sources", stage);
            fingerprints
        } else {
            Vec::new()
        };
        if project_cache.enabled() {
            let stage = Instant::now();
            match project_cache.load(&options, &cache_fingerprints) {
                Ok(Some(payload)) => {
                    log_timing(timing, "load_project_cache", stage);
                    return Self::from_cached(root, options, payload, storage_dir, total_start);
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
        let mut files: Vec<FileEntry> = paths
            .par_iter()
            .filter_map(|path| {
                read_file_entry(
                    &root,
                    path,
                    options.max_file_bytes,
                    options.diagnostics.slow_file_ms,
                )
                .ok()
            })
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        log_timing(timing, "read_parse_files", stage);

        let stage = Instant::now();
        let mut chunks = Vec::new();
        for file in &files {
            chunks.extend(chunk_source(
                &file.language,
                &file.content,
                &file.path,
                &file.symbols,
            ));
        }
        for (id, chunk) in chunks.iter_mut().enumerate() {
            chunk.id = id;
        }
        let chunk_indices_by_file = build_chunk_indices_by_file(&chunks);
        let symbol_definition_chunks =
            build_symbol_definition_chunks(&files, &chunks, &chunk_indices_by_file);
        log_timing(timing, "chunk_files", stage);

        let stage = Instant::now();
        let dependency_symbols = build_dependency_symbols(&files);
        let (word_index, dependency_identifiers) = build_word_index(&files, &dependency_symbols);
        log_timing(timing, "word_index", stage);

        let stage = Instant::now();
        let (deps_forward, deps_reverse) =
            build_dependencies(&files, &dependency_symbols, &dependency_identifiers);
        log_timing(timing, "dependencies", stage);

        let stage = Instant::now();
        let file_map = files
            .iter()
            .cloned()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let graph = CodeGraph::build(&file_map, &deps_forward);
        log_timing(timing, "graph", stage);

        let stage = Instant::now();
        let bm25 = Bm25Index::new(
            chunks
                .par_iter()
                .map(|chunk| tokenize(&enrich_for_bm25(chunk)))
                .collect(),
        );
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

        let stage = Instant::now();
        let model = load_embedding_model(&options.embedding_model)?;
        log_timing(timing, "load_embedding_model", stage);

        let stage = Instant::now();
        let texts = semantic_units
            .iter()
            .map(|unit| unit.text.clone())
            .collect::<Vec<_>>();
        let embeddings = model.encode(&texts);
        log_timing(timing, "encode_embeddings", stage);

        let stage = Instant::now();
        let vectors = MinishVectorStore::build(&embeddings)?;
        log_timing(timing, "hnsw", stage);
        if project_cache.enabled() {
            let stage = Instant::now();
            if let Err(err) =
                project_cache.save(&options, &files, &chunks, &semantic_units, &embeddings)
            {
                eprintln!(
                    "codebase-mcp cache save failed at {}: {err:#}",
                    project_cache.dir().display()
                );
            }
            log_timing(timing, "save_project_cache", stage);
        }
        log_timing(timing, "total", total_start);

        Ok(Self {
            root,
            options,
            seq: now_ms() as u64,
            files: file_map,
            chunks,
            semantic_units,
            chunk_indices_by_file,
            symbol_definition_chunks,
            word_index,
            deps_forward,
            deps_reverse,
            graph,
            bm25,
            vectors,
            model,
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
        total_start: Instant,
    ) -> Result<Self> {
        let timing = options.diagnostics.timing;
        let stage = Instant::now();
        let model = load_embedding_model(&options.embedding_model)?;
        log_timing(timing, "load_embedding_model", stage);

        let stage = Instant::now();
        let vectors = MinishVectorStore::build(&payload.embeddings)?;
        log_timing(timing, "hnsw", stage);

        let stage = Instant::now();
        let files = payload
            .files
            .into_iter()
            .map(|file| file.into_file_entry())
            .collect::<Vec<_>>();
        let chunks = payload.chunks;
        let chunk_indices_by_file = build_chunk_indices_by_file(&chunks);
        let symbol_definition_chunks =
            build_symbol_definition_chunks(&files, &chunks, &chunk_indices_by_file);
        log_timing(timing, "restore_cached_files", stage);

        let stage = Instant::now();
        let dependency_symbols = build_dependency_symbols(&files);
        let (word_index, dependency_identifiers) = build_word_index(&files, &dependency_symbols);
        log_timing(timing, "word_index", stage);

        let stage = Instant::now();
        let (deps_forward, deps_reverse) =
            build_dependencies(&files, &dependency_symbols, &dependency_identifiers);
        log_timing(timing, "dependencies", stage);

        let stage = Instant::now();
        let file_map = files
            .into_iter()
            .map(|file| (file.path.clone(), file))
            .collect::<BTreeMap<_, _>>();
        let graph = CodeGraph::build(&file_map, &deps_forward);
        log_timing(timing, "graph", stage);

        let stage = Instant::now();
        let bm25 = Bm25Index::new(
            chunks
                .par_iter()
                .map(|chunk| tokenize(&enrich_for_bm25(chunk)))
                .collect(),
        );
        log_timing(timing, "bm25", stage);
        log_timing(timing, "total", total_start);

        Ok(Self {
            root,
            options,
            seq: now_ms() as u64,
            files: file_map,
            chunks,
            semantic_units: payload.semantic_units,
            chunk_indices_by_file,
            symbol_definition_chunks,
            word_index,
            deps_forward,
            deps_reverse,
            graph,
            bm25,
            vectors,
            model,
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
            graph_nodes: self.graph.nodes.len(),
            graph_edges: self.graph.edges.len(),
            graph_communities: self.graph.communities.len(),
            embedding_model: self.model.model_id().to_string(),
            embedding_dims: self.model.dim(),
            vector_count: self.vectors.len(),
            vector_units: "files",
            storage_dir: self.storage_dir.clone(),
            cache: self.cache_status,
        }
    }

    pub fn file(&self, path: &str) -> Option<&FileEntry> {
        let normalized = normalize_rel_path(path);
        self.files.get(&normalized)
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
            .filter_map(|(idx, chunk)| globset.is_match(&chunk.file_path).then_some(idx))
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
            for (idx, line) in file.content.lines().enumerate() {
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

fn build_chunk_indices_by_file(chunks: &[Chunk]) -> HashMap<String, Vec<usize>> {
    let mut by_file: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, chunk) in chunks.iter().enumerate() {
        by_file
            .entry(chunk.file_path.clone())
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

    let mut paths = BTreeSet::new();
    collect_paths_from(
        root,
        root,
        &extensions,
        options.respect_gitignore,
        options,
        None,
        &mut paths,
    )?;

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
    paths: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    let filter_root = root.to_path_buf();
    let filter_include_root = include_root.map(Path::to_path_buf);
    let filter_options = options.clone();
    let mut builder = WalkBuilder::new(start);
    builder
        .hidden(false)
        .parents(true)
        .git_ignore(respect_gitignore)
        .git_exclude(respect_gitignore)
        .filter_entry(move |entry| {
            !is_skipped_entry(
                &filter_root,
                entry.path(),
                filter_include_root.as_deref(),
                &filter_options,
            )
        });
    for entry in builder.build() {
        let entry = entry?;
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if extensions.contains(&ext.to_ascii_lowercase()) {
            paths.insert(path.to_path_buf());
        }
    }
    Ok(())
}

fn is_skipped_entry(
    root: &Path,
    path: &Path,
    include_root: Option<&Path>,
    options: &IndexOptions,
) -> bool {
    if path == root || include_root.is_some_and(|include_root| path == include_root) {
        return false;
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
            ".git"
                | ".hg"
                | ".svn"
                | ".vs"
                | ".idea"
                | ".gradle"
                | ".codedb-mcp"
                | "node_modules"
        ) {
            return true;
        }

        if options.skip_dirs.iter().any(|skip| skip == &part) {
            return true;
        }
    }

    false
}

fn load_embedding_model(model_id: &str) -> Result<MinishEmbeddingModel> {
    let path = Path::new(model_id);
    if path.exists() {
        MinishEmbeddingModel::load_local(path)
    } else {
        MinishEmbeddingModel::load(model_id)
    }
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
        language: language.to_string(),
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

fn build_word_index(
    files: &[FileEntry],
    dependency_symbols: &HashMap<String, Vec<SymbolDefinition>>,
) -> (
    HashMap<String, Vec<WordHit>>,
    HashMap<String, DependencyReferences>,
) {
    let partials = files
        .par_iter()
        .map(|file| {
            let mut local: HashMap<String, Vec<WordHit>> = HashMap::new();
            let mut dependency_references = DependencyReferences::default();
            let aliases = (file.language == "csharp").then(|| parse_using_aliases(&file.content));
            if file.language == "csharp" {
                collect_static_using_dependency_references(
                    &file.content,
                    dependency_symbols,
                    &mut dependency_references,
                );
            }
            for (line_idx, line) in file.content.lines().enumerate() {
                let mut seen = HashSet::new();
                let identifiers = raw_identifiers(line);
                let code = strip_strings_and_line_comment(line);
                let dependency_line = is_dependency_reference_line(&file.language, &code);
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
                        local.entry(raw).or_default().push(WordHit {
                            path: file.path.clone(),
                            line: line_idx + 1,
                        });
                    }
                }
            }
            (local, file.path.clone(), dependency_references)
        })
        .collect::<Vec<_>>();

    let mut index: HashMap<String, Vec<WordHit>> = HashMap::new();
    let mut references_by_file = HashMap::new();
    for (partial, path, dependency_references) in partials {
        for (word, mut hits) in partial {
            index.entry(word).or_default().append(&mut hits);
        }
        references_by_file.insert(path, dependency_references);
    }
    for hits in index.values_mut() {
        hits.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
    }
    (index, references_by_file)
}

#[derive(Clone)]
struct SymbolDefinition {
    name: String,
    path: String,
    namespace: Option<String>,
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
            .filter(|symbol| is_dependency_symbol_kind(&symbol.kind))
            .collect::<Vec<_>>();
        let has_primary_type = type_symbols.iter().any(|symbol| {
            is_dependency_symbol_kind(&symbol.kind)
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
            symbols_by_name
                .entry(symbol.name.clone())
                .or_default()
                .push(SymbolDefinition {
                    name: symbol.name.clone(),
                    path: file.path.clone(),
                    namespace: file.namespace.clone(),
                });
        }
    }
    symbols_by_name
}

fn build_dependencies(
    files: &[FileEntry],
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references_by_file: &HashMap<String, DependencyReferences>,
) -> (HashMap<String, Vec<String>>, HashMap<String, Vec<String>>) {
    let mut forward: HashMap<String, BTreeSet<String>> = HashMap::new();
    for file in files {
        let deps = forward.entry(file.path.clone()).or_default();
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
    matches!(kind, "class" | "interface" | "struct" | "enum" | "record")
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
        _ => trimmed.starts_with("using ") || trimmed.starts_with("namespace "),
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

fn parse_using_aliases(content: &str) -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for line in content.lines() {
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

fn collect_static_using_dependency_references(
    content: &str,
    symbols_by_name: &HashMap<String, Vec<SymbolDefinition>>,
    references: &mut DependencyReferences,
) {
    for line in content.lines() {
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
    let Some(candidate_namespace) = candidate.namespace.as_deref() else {
        return file.namespace.is_none();
    };
    file.namespace.as_deref() == Some(candidate_namespace)
        || imports_symbol_namespace(&file.imports, candidate_namespace, &candidate.name)
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

fn enrich_for_bm25(chunk: &Chunk) -> String {
    let path = Path::new(&chunk.file_path);
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let parent = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    format!("{} {stem} {stem} {parent}", chunk.content)
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
        text.push_str(&symbol.kind);
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

    fn file_with_language(path: &str, language: &str, content: &str) -> FileEntry {
        FileEntry {
            path: path.to_string(),
            language: language.to_string(),
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
        let symbols = build_dependency_symbols(&files);
        let (_, references) = build_word_index(&files, &symbols);
        let (forward, _) = build_dependencies(&files, &symbols, &references);
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
}
