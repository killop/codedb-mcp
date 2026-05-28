use crate::cache::{CachedCallerEntry, CachedCallerHit, CachedDepsSnapshot, ProjectCache};
use crate::graph::GraphCommunity;
use crate::indexer::{
    ChangedFile, Codebase, IndexOptions, build_globset, fingerprint_project, hash_content,
    normalize_rel_path,
};
use crate::language::{is_comment_or_blank, scope_for_line};
use crate::search::hybrid_search;
use crate::tokens::{has_whole_word, split_identifier};
use crate::types::{FileEntry, SearchHit, Symbol};
use anyhow::{Result, anyhow};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_BATCH_ITEMS: usize = 100;
const MODULE_HUB_INCOMING_LIMIT: usize = 220;
const MODULE_MAX_DEPENDENCY_EDGES_PER_FILE: usize = 72;
const MODULE_MAX_FILES_PER_GROUP: usize = 450;
const MODULE_LABEL_ITERATIONS: usize = 8;

pub struct ProjectManager {
    default_root: PathBuf,
    options: IndexOptions,
    cache: RwLock<HashMap<String, Arc<Codebase>>>,
    build_lock: Mutex<()>,
}

impl ProjectManager {
    pub fn new(default_root: PathBuf, options: IndexOptions) -> Result<Self> {
        let manager = Self::new_lazy(default_root, options);
        let root = manager.default_root.clone();
        manager.reindex(&root)?;
        Ok(manager)
    }

    pub fn new_lazy(default_root: PathBuf, options: IndexOptions) -> Self {
        Self {
            default_root,
            options,
            cache: RwLock::new(HashMap::new()),
            build_lock: Mutex::new(()),
        }
    }

    pub fn get(&self, project: Option<&str>) -> Result<Arc<Codebase>> {
        let root = match project {
            Some(project) if !project.trim().is_empty() => PathBuf::from(project),
            _ => self.default_root.clone(),
        }
        .canonicalize()?;
        let key = root.display().to_string();
        if let Some(index) = self.cache.read().get(&key) {
            return Ok(index.clone());
        }
        let _guard = self.build_lock.lock();
        if let Some(index) = self.cache.read().get(&key) {
            return Ok(index.clone());
        }
        let mut index = Codebase::index(&root, self.options.clone())?;
        index.changed_files = initial_changes(&index);
        let index = Arc::new(index);
        self.cache.write().insert(key, index.clone());
        Ok(index)
    }

    pub fn reindex(&self, path: &Path) -> Result<Arc<Codebase>> {
        let root = path.canonicalize()?;
        let key = root.display().to_string();
        let _guard = self.build_lock.lock();
        let old = self.cache.read().get(&key).cloned();
        let mut index = Codebase::index(&root, self.options.clone())?;
        index.changed_files = match old.as_deref() {
            Some(old) => diff_changes(old, &index),
            None => initial_changes(&index),
        };
        let index = Arc::new(index);
        self.cache.write().insert(key, index.clone());
        Ok(index)
    }

    pub fn reindex_default(&self) -> Result<Arc<Codebase>> {
        self.reindex(&self.default_root)
    }

    pub fn default_root(&self) -> PathBuf {
        self.default_root.clone()
    }

    pub fn extensions(&self) -> Vec<String> {
        self.options.extensions.clone()
    }

    pub fn projects(&self) -> Vec<String> {
        let mut projects = self.cache.read().keys().cloned().collect::<Vec<_>>();
        projects.sort();
        projects
    }

    pub fn default_has_content_changes(&self) -> Result<bool> {
        let root = self.default_root.canonicalize()?;
        let key = root.display().to_string();
        let Some(current) = self.cache.read().get(&key).cloned() else {
            return Ok(true);
        };
        let next = fingerprint_project(&root, &self.options)?;
        if next.len() != current.files.len() {
            return Ok(true);
        }
        for (path, hash) in next {
            let Some(file) = current.files.get(&path) else {
                return Ok(true);
            };
            if file.content_hash != hash {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub fn dispatch_cached_cli_tool(
    default_root: &Path,
    options: &IndexOptions,
    name: &str,
    args: &Value,
) -> Result<Option<String>> {
    if !matches!(
        name,
        "codedb_status" | "codedb_find" | "codedb_deps" | "codedb_callers"
    ) {
        return Ok(None);
    }
    let root = tool_project_root(default_root, args)?;
    let cache = ProjectCache::new(&root, &options.storage)?;
    if !cache.enabled() {
        return Ok(None);
    }
    match name {
        "codedb_status" => Ok(cache
            .load_status(options)?
            .map(|status| format_cached_status(options, &status))),
        "codedb_find" => Ok(cache
            .load_file_list(options)?
            .map(|files| handle_cached_find(&files, args))
            .transpose()?),
        "codedb_deps" => Ok(cache
            .load_deps_snapshot(options)?
            .map(|snapshot| handle_cached_deps(&snapshot, args))
            .transpose()?),
        "codedb_callers" => handle_cached_callers(&cache, options, args),
        _ => Ok(None),
    }
}

fn tool_project_root(default_root: &Path, args: &Value) -> Result<PathBuf> {
    let root = args
        .get("project")
        .and_then(Value::as_str)
        .filter(|project| !project.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| default_root.to_path_buf());
    Ok(root.canonicalize()?)
}

fn format_cached_status(
    options: &IndexOptions,
    status: &crate::cache::CachedStatusSnapshot,
) -> String {
    format!(
        "codedb status:\n  seq: {}\n  files: {}\n  outlines: {}\n  chunks: {}\n  graph: {} nodes, {} edges, {} communities\n  vector_index: lazy flat cosine ({} files vectors)\n  embedding_model: model2vec-rs {} ({} dims)\n  scan: ready\n  extensions: {}\n  cache: hit\n  storage: {}\n",
        status.seq,
        status.files,
        status.files,
        status.chunks,
        status.graph_stats.nodes,
        status.graph_stats.edges,
        status.graph_stats.communities,
        status.vector_count,
        status.embedding_model,
        status.embedding_dims,
        options.extensions.join(","),
        status.storage_dir
    )
}

fn handle_cached_find(files: &[String], args: &Value) -> Result<String> {
    let query = required_str(args, "query")?;
    let max_results = get_usize(args, "max_results").unwrap_or(10).clamp(1, 50);
    let mut matches = files
        .iter()
        .filter_map(|path| fuzzy_score(path, &query).map(|score| (path.clone(), score)))
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    matches.truncate(max_results);
    if matches.is_empty() {
        return Ok("no matches".to_string());
    }
    let mut out = String::new();
    for (idx, (path, score)) in matches.into_iter().enumerate() {
        out.push_str(&format!("{}. {} (score: {:.2})\n", idx + 1, path, score));
    }
    Ok(out)
}

fn handle_cached_deps(snapshot: &CachedDepsSnapshot, args: &Value) -> Result<String> {
    let path = normalize_rel_path(&required_str(args, "path")?);
    let direction = get_str(args, "direction").unwrap_or_else(|| "imported_by".to_string());
    let transitive = get_bool(args, "transitive");
    let max_depth = get_usize(args, "max_depth");
    let forward = direction == "depends_on";
    let results = if transitive {
        cached_transitive_deps(&snapshot.deps_forward, &path, forward, max_depth)
    } else if forward {
        snapshot
            .deps_forward
            .get(&path)
            .cloned()
            .unwrap_or_default()
    } else {
        cached_reverse_deps(&snapshot.deps_forward, &path)
    };

    let mut out = if forward {
        if transitive {
            format!("{path} transitively depends on:\n")
        } else {
            format!("{path} depends on:\n")
        }
    } else if transitive {
        format!("{path} is transitively imported by:\n")
    } else {
        format!("{path} is imported by:\n")
    };
    if results.is_empty() {
        out.push_str("  (none)\n");
        if snapshot.files.binary_search(&path).is_err() {
            out.push_str(&cached_fuzzy_suggestions(&snapshot.files, &path));
        }
    } else {
        for result in &results {
            out.push_str(&format!("  {result}\n"));
        }
        out.push_str(&format!("({} files)\n", results.len()));
    }
    Ok(out)
}

fn cached_reverse_deps(deps_forward: &HashMap<String, Vec<String>>, path: &str) -> Vec<String> {
    let mut results = deps_forward
        .iter()
        .filter_map(|(source, targets)| {
            targets
                .iter()
                .any(|target| target == path)
                .then_some(source.clone())
        })
        .collect::<Vec<_>>();
    results.sort();
    results
}

fn cached_transitive_deps(
    deps_forward: &HashMap<String, Vec<String>>,
    path: &str,
    forward: bool,
    max_depth: Option<usize>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([(path.to_string(), 0usize)]);
    while let Some((current, depth)) = queue.pop_front() {
        if max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        let deps = if forward {
            deps_forward.get(&current).cloned().unwrap_or_default()
        } else {
            cached_reverse_deps(deps_forward, &current)
        };
        for dep in deps {
            if seen.insert(dep.clone()) {
                queue.push_back((dep, depth + 1));
            }
        }
    }
    seen.into_iter().collect()
}

fn cached_fuzzy_suggestions(files: &[String], query: &str) -> String {
    let mut matches = files
        .iter()
        .filter_map(|path| fuzzy_score(path, query).map(|score| (path.clone(), score)))
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| b.1.total_cmp(&a.1));
    if matches.is_empty() {
        return String::new();
    }
    let mut out = String::from("did you mean:\n");
    for (path, score) in matches.into_iter().take(5) {
        out.push_str(&format!("  {path} (score: {score:.2})\n"));
    }
    out
}

fn handle_cached_callers(
    cache: &ProjectCache,
    options: &IndexOptions,
    args: &Value,
) -> Result<Option<String>> {
    if args.get("targets").is_some() {
        return Ok(None);
    }
    let Some(name) = get_str(args, "name") else {
        return Ok(None);
    };
    let Some(path) = get_str(args, "definition_path").or_else(|| get_str(args, "path")) else {
        return Ok(None);
    };
    let Some(line_start) = get_usize(args, "definition_line").or_else(|| get_usize(args, "line"))
    else {
        return Ok(None);
    };
    let path = normalize_rel_path(&path);
    let max_results = get_usize(args, "max_results")
        .unwrap_or(50)
        .clamp(1, 10_000);
    Ok(cache
        .load_caller_entry(options, &name, &path, line_start)?
        .map(|entry| format_cached_caller_entry(&entry, max_results)))
}

fn format_cached_caller_entry(entry: &CachedCallerEntry, max_results: usize) -> String {
    let hits = entry
        .hits
        .iter()
        .take(max_results.min(entry.hits.len()))
        .collect::<Vec<_>>();
    let mut out = format!(
        "{} references for '{}' resolved to {}:{} ({})\n",
        hits.len(),
        entry.name,
        entry.path,
        entry.line_start,
        entry.kind
    );
    for hit in hits {
        if let Some(scope) = &hit.scope {
            out.push_str(&format!(
                "  {}:{}: {}  [in {} ({}, L{}-L{})]\n",
                hit.path, hit.line, hit.text, scope.name, scope.kind, scope.start, scope.end
            ));
        } else {
            out.push_str(&format!("  {}:{}: {}\n", hit.path, hit.line, hit.text));
        }
    }
    out
}

fn initial_changes(index: &Codebase) -> Vec<ChangedFile> {
    index
        .files
        .values()
        .map(|file| ChangedFile {
            path: file.path.clone(),
            op: "upsert",
            size: file.byte_size,
        })
        .collect()
}

fn diff_changes(old: &Codebase, new: &Codebase) -> Vec<ChangedFile> {
    let mut changes = Vec::new();
    for file in new.files.values() {
        match old.files.get(&file.path) {
            Some(previous) if previous.content_hash == file.content_hash => {}
            _ => changes.push(ChangedFile {
                path: file.path.clone(),
                op: "upsert",
                size: file.byte_size,
            }),
        }
    }
    for path in old.files.keys() {
        if !new.files.contains_key(path) {
            changes.push(ChangedFile {
                path: path.clone(),
                op: "delete",
                size: 0,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

pub fn dispatch_tool(manager: &ProjectManager, name: &str, args: &Value) -> String {
    let result = match name {
        "codedb_index" => handle_index(manager, args),
        "codedb_projects" => Ok(handle_projects(manager)),
        "codedb_bundle" => handle_bundle(manager, args),
        "codedb_remote" => {
            Ok("error: codedb_remote is not implemented in local Rust codebase-mcp".to_string())
        }
        "codedb_edit" => Ok("error: codedb_edit is disabled; this server is read-only".to_string()),
        _ => {
            let project = get_str(args, "project");
            match manager.get(project.as_deref()) {
                Ok(index) => dispatch_index_tool(index.as_ref(), name, args),
                Err(err) => Ok(format!("error: failed to load project: {err}")),
            }
        }
    };
    result.unwrap_or_else(|err| format!("error: {err}"))
}

fn dispatch_index_tool(index: &Codebase, name: &str, args: &Value) -> Result<String> {
    match name {
        "codedb_tree" => Ok(handle_tree(index)),
        "codedb_outline" => handle_outline(index, args),
        "codedb_symbol" => handle_symbol(index, args),
        "codedb_search" => handle_search(index, args),
        "codedb_word" => handle_word(index, args),
        "codedb_callers" => handle_callers(index, args),
        "codedb_hot" => Ok(handle_hot(index, args)),
        "codedb_deps" => handle_deps(index, args),
        "codedb_read" => handle_read(index, args),
        "codedb_changes" => Ok(handle_changes(index, args)),
        "codedb_status" => Ok(handle_status(index)),
        "codedb_snapshot" => Ok(handle_snapshot(index)),
        "codedb_find" => handle_find(index, args),
        "codedb_glob" => handle_glob(index, args),
        "codedb_ls" => handle_ls(index, args),
        "codedb_query" => handle_query(index, args),
        "codedb_graph" => handle_graph(index, args),
        "codedb_explain" => handle_explain(index, args),
        "codedb_path" => handle_path(index, args),
        "codedb_communities" => handle_communities(index, args),
        "codedb_module_map" => handle_module_map(index, args),
        "codedb_module_atlas" => handle_module_atlas(index, args),
        "codedb_analyze" => handle_analyze(index, args),
        "codedb_export" => handle_export(index, args),
        _ => Ok(format!("error: unknown tool: {name}")),
    }
}

fn handle_tree(index: &Codebase) -> String {
    let mut out = String::new();
    out.push_str(&format!("{}\n", index.root.display()));
    for file in index.files.values() {
        out.push_str(&format!(
            "  {} ({}, {}L, {} sym)\n",
            file.path,
            file.language,
            file.line_count,
            file.symbols.len()
        ));
    }
    out
}

fn handle_outline(index: &Codebase, args: &Value) -> Result<String> {
    let path = required_str(args, "path")?;
    let compact = get_bool(args, "compact");
    let Some(file) = index.file(&path) else {
        return Ok(format!(
            "error: file not indexed: {path}\n{}",
            fuzzy_suggestions(index, &path)
        ));
    };
    let mut out = String::new();
    out.push_str(&format!(
        "{} ({}, {} lines, {} bytes)\n",
        file.path, file.language, file.line_count, file.byte_size
    ));
    for symbol in &file.symbols {
        if compact {
            out.push_str(&format!(
                "  L{}: {} {}\n",
                symbol.line_start, symbol.kind, symbol.name
            ));
        } else {
            out.push_str(&format!(
                "  L{}: {} {}  // {}\n",
                symbol.line_start, symbol.kind, symbol.name, symbol.detail
            ));
        }
    }
    Ok(out)
}

fn handle_symbol(index: &Codebase, args: &Value) -> Result<String> {
    let name = required_str(args, "name")?;
    let include_body = get_bool(args, "body");
    let results = index.symbols_named(&name);
    if results.is_empty() {
        return Ok(format!("no results for: {name}"));
    }
    let mut out = format!("{} results for '{}':\n", results.len(), name);
    for (file, symbol) in results {
        out.push_str(&format!(
            "  {}:{} ({})  // {}\n",
            file.path, symbol.line_start, symbol.kind, symbol.detail
        ));
        if include_body {
            let content = index.file_content(file)?;
            out.push_str(&extract_lines(
                &content,
                symbol.line_start,
                symbol.line_end,
                false,
            ));
        }
    }
    Ok(out)
}

fn handle_search(index: &Codebase, args: &Value) -> Result<String> {
    if args.get("queries").is_some() {
        let Some(items) = args.get("queries").and_then(Value::as_array) else {
            return Ok("error: 'queries' must be an array".to_string());
        };
        return handle_search_batch(index, args, items);
    }
    handle_search_one(index, args)
}

fn handle_search_batch(index: &Codebase, base_args: &Value, items: &[Value]) -> Result<String> {
    if items.is_empty() {
        return Ok("error: 'queries' must not be empty".to_string());
    }
    let mut out = format!(
        "{} codedb_search batch items:\n",
        items.len().min(MAX_BATCH_ITEMS)
    );
    for (idx, item) in items.iter().take(MAX_BATCH_ITEMS).enumerate() {
        let args = batch_item_args(base_args, "queries", item, "query")?;
        let query = get_str(&args, "query").unwrap_or_default();
        out.push_str(&format!("--- [{idx}] codedb_search: {query} ---\n"));
        out.push_str(&handle_search_one(index, &args)?);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if items.len() > MAX_BATCH_ITEMS {
        out.push_str(&format!(
            "(truncated: {} more batch items not executed)\n",
            items.len() - MAX_BATCH_ITEMS
        ));
    }
    Ok(out)
}

fn handle_search_one(index: &Codebase, args: &Value) -> Result<String> {
    let query = required_str(args, "query")?;
    if query.trim().is_empty() {
        return Ok("error: empty query - pass a non-empty 'query' string".to_string());
    }
    let max_results = get_usize(args, "max_results")
        .unwrap_or(50)
        .clamp(1, 10_000);
    let scope = get_bool(args, "scope");
    let compact = get_bool(args, "compact");
    let regex = get_bool(args, "regex");
    let path_glob = get_str(args, "path_glob");

    if regex {
        let hits = index.line_hits(
            &query,
            max_results,
            regex,
            path_glob.as_deref(),
            compact,
            scope,
        )?;
        return Ok(format_line_hits(&query, hits));
    }

    let selector = path_glob
        .as_deref()
        .map(|glob| index.path_selector(Some(glob)));
    let selector_ref = selector.as_deref();
    let hits = hybrid_search(index, &query, max_results, selector_ref)?;
    if hits.is_empty() {
        let fallback = index.line_hits(
            &query,
            max_results,
            regex,
            path_glob.as_deref(),
            compact,
            scope,
        )?;
        return Ok(format_line_hits(&query, fallback));
    }
    format_chunk_hits(index, &query, hits)
}

fn handle_word(index: &Codebase, args: &Value) -> Result<String> {
    let word = required_str(args, "word")?;
    let hits = index.word_hits(&word)?;
    let mut out = format!("{} hits for '{}':\n", hits.len(), word);
    for hit in &hits {
        if let Some(file) = index.file_by_id(hit.file_id) {
            out.push_str(&format!("  {}:{}\n", file.path, hit.line));
        }
    }
    Ok(out)
}

fn handle_callers(index: &Codebase, args: &Value) -> Result<String> {
    if args.get("targets").is_some() {
        let Some(items) = args.get("targets").and_then(Value::as_array) else {
            return Ok("error: 'targets' must be an array".to_string());
        };
        return handle_callers_batch(index, args, items);
    }
    handle_callers_one(index, args)
}

fn handle_callers_batch(index: &Codebase, base_args: &Value, items: &[Value]) -> Result<String> {
    if items.is_empty() {
        return Ok("error: 'targets' must not be empty".to_string());
    }
    let mut out = format!(
        "{} codedb_callers batch items:\n",
        items.len().min(MAX_BATCH_ITEMS)
    );
    for (idx, item) in items.iter().take(MAX_BATCH_ITEMS).enumerate() {
        let args = batch_item_args(base_args, "targets", item, "name")?;
        let name = get_str(&args, "name").unwrap_or_default();
        out.push_str(&format!("--- [{idx}] codedb_callers: {name} ---\n"));
        out.push_str(&handle_callers_one(index, &args)?);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    if items.len() > MAX_BATCH_ITEMS {
        out.push_str(&format!(
            "(truncated: {} more batch items not executed)\n",
            items.len() - MAX_BATCH_ITEMS
        ));
    }
    Ok(out)
}

fn handle_callers_one(index: &Codebase, args: &Value) -> Result<String> {
    let name = required_str(args, "name")?;
    let max_results = get_usize(args, "max_results")
        .unwrap_or(50)
        .clamp(1, 10_000);
    let target = match resolve_callers_target(index, args, &name)? {
        TargetResolution::Resolved(target) => target,
        TargetResolution::NotFound => return Ok(format!("no definition found for: {name}")),
        TargetResolution::Ambiguous(candidates) => {
            return Ok(format_ambiguous_callers_target(&name, candidates));
        }
    };
    let all_definition_sites = index
        .symbols_named(&name)
        .into_iter()
        .map(|(file, symbol)| (file.path.clone(), symbol.line_start))
        .collect::<BTreeSet<_>>();
    let mut hits = reference_candidates(index, &name)?;
    hits.retain(|hit| is_lsp_like_reference(index, &target, hit, &all_definition_sites));
    save_cached_caller_entry(index, &target, &hits);
    hits.truncate(max_results);
    let mut out = format!(
        "{} references for '{}' resolved to {}:{} ({})\n",
        hits.len(),
        name,
        target.path,
        target.line_start,
        target.kind
    );
    for hit in hits {
        if let Some(scope) = hit.scope {
            out.push_str(&format!(
                "  {}:{}: {}  [in {} ({}, L{}-L{})]\n",
                hit.path, hit.line, hit.text, scope.name, scope.kind, scope.start, scope.end
            ));
        } else {
            out.push_str(&format!("  {}:{}: {}\n", hit.path, hit.line, hit.text));
        }
    }
    Ok(out)
}

fn save_cached_caller_entry(index: &Codebase, target: &SymbolTarget, hits: &[SearchHit]) {
    let Ok(cache) = ProjectCache::new(&index.root, &index.options.storage) else {
        return;
    };
    if !cache.enabled() {
        return;
    }
    let entry = CachedCallerEntry {
        name: target.name.clone(),
        path: target.path.clone(),
        line_start: target.line_start,
        kind: target.kind.clone(),
        hits: hits
            .iter()
            .map(|hit| CachedCallerHit {
                path: hit.path.clone(),
                line: hit.line,
                text: hit.text.clone(),
                scope: hit.scope.clone(),
            })
            .collect(),
    };
    if let Err(err) = cache.save_caller_entry(entry) {
        eprintln!("codebase-mcp caller sidecar save failed: {err:#}");
    }
}

fn batch_item_args(
    base_args: &Value,
    batch_key: &str,
    item: &Value,
    scalar_key: &str,
) -> Result<Value> {
    let mut merged = base_args.as_object().cloned().unwrap_or_else(Map::new);
    merged.remove(batch_key);
    match item {
        Value::String(value) => {
            merged.insert(scalar_key.to_string(), Value::String(value.clone()));
        }
        Value::Object(overrides) => {
            for (key, value) in overrides {
                if key != batch_key {
                    merged.insert(key.clone(), value.clone());
                }
            }
        }
        _ => return Err(anyhow!("batch items must be strings or objects")),
    }
    Ok(Value::Object(merged))
}

fn reference_candidates(index: &Codebase, name: &str) -> Result<Vec<SearchHit>> {
    let word_hits = index.word_hits(name)?;
    if word_hits.is_empty() {
        return Ok(Vec::new());
    };
    let mut content_by_file = HashMap::<u32, String>::new();
    let mut results = Vec::new();
    for hit in &word_hits {
        let Some(file) = index.file_by_id(hit.file_id) else {
            continue;
        };
        if !content_by_file.contains_key(&hit.file_id) {
            content_by_file.insert(hit.file_id, index.file_content(file)?);
        }
        if let Some(content) = content_by_file.get(&hit.file_id) {
            let line = hit.line as usize;
            let Some(text) = content.lines().nth(line.saturating_sub(1)) else {
                continue;
            };
            let scope = scope_for_line(&file.symbols, line);
            results.push(SearchHit {
                path: file.path.clone(),
                line,
                text: text.trim().to_string(),
                scope,
            });
        }
    }
    Ok(results)
}

#[derive(Debug, Clone)]
struct SymbolTarget {
    name: String,
    kind: String,
    path: String,
    line_start: usize,
    namespace: Option<String>,
    detail: String,
}

enum TargetResolution {
    Resolved(SymbolTarget),
    Ambiguous(Vec<SymbolTarget>),
    NotFound,
}

fn resolve_callers_target(index: &Codebase, args: &Value, name: &str) -> Result<TargetResolution> {
    let definition_path = get_str(args, "definition_path").or_else(|| get_str(args, "path"));
    let definition_line = get_usize(args, "definition_line").or_else(|| get_usize(args, "line"));
    let candidates = if let Some(path) = definition_path {
        let normalized = normalize_rel_path(&path);
        let Some(file) = index.file(&normalized) else {
            return Ok(TargetResolution::NotFound);
        };
        file.symbols
            .iter()
            .filter(|symbol| symbol.name == name)
            .filter(|symbol| {
                definition_line.is_none_or(|line| {
                    symbol.line_start == line
                        || (symbol.line_start <= line && line <= symbol.line_end)
                })
            })
            .map(|symbol| target_from_symbol(file, symbol))
            .collect::<Vec<_>>()
    } else {
        index
            .symbols_named(name)
            .into_iter()
            .map(|(file, symbol)| target_from_symbol(file, symbol))
            .collect::<Vec<_>>()
    };

    match candidates.len() {
        0 => Ok(TargetResolution::NotFound),
        1 => Ok(TargetResolution::Resolved(
            candidates.into_iter().next().unwrap(),
        )),
        _ => Ok(TargetResolution::Ambiguous(candidates)),
    }
}

fn target_from_symbol(file: &FileEntry, symbol: &Symbol) -> SymbolTarget {
    SymbolTarget {
        name: symbol.name.clone(),
        kind: symbol.kind.to_string(),
        path: file.path.clone(),
        line_start: symbol.line_start,
        namespace: file.namespace.clone(),
        detail: symbol.detail.clone(),
    }
}

fn format_ambiguous_callers_target(name: &str, mut candidates: Vec<SymbolTarget>) -> String {
    candidates.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line_start.cmp(&b.line_start))
    });
    let mut out = format!(
        "ambiguous symbol '{name}': {} definitions. Pass definition_path and definition_line.\n",
        candidates.len()
    );
    for candidate in candidates.into_iter().take(20) {
        out.push_str(&format!(
            "  {}:{} ({})  // {}\n",
            candidate.path, candidate.line_start, candidate.kind, candidate.detail
        ));
    }
    out
}

fn is_lsp_like_reference(
    index: &Codebase,
    target: &SymbolTarget,
    hit: &SearchHit,
    all_definition_sites: &BTreeSet<(String, usize)>,
) -> bool {
    if all_definition_sites.contains(&(hit.path.clone(), hit.line)) {
        return false;
    }
    let Some(file) = index.file(&hit.path) else {
        return false;
    };
    let code = strip_strings_and_line_comment(&hit.text);
    if !can_file_reference_target(file, target) && !code_references_qualified_target(&code, target)
    {
        return false;
    }
    if !has_whole_word(&code, &target.name) {
        return false;
    }
    if !has_reference_occurrence(&code, target) {
        return false;
    }
    if is_type_symbol_kind(&target.kind) {
        has_type_reference_shape(&code, &target.name)
    } else {
        true
    }
}

fn can_file_reference_target(file: &FileEntry, target: &SymbolTarget) -> bool {
    if file.path == target.path {
        return true;
    }
    if target.namespace.is_none() {
        if file.path.starts_with("Library/PackageCache/")
            && !target.path.starts_with("Library/PackageCache/")
        {
            return false;
        }
        if is_sibling_third_party_package(&target.path, &file.path) {
            return false;
        }
        if file
            .namespace
            .as_deref()
            .is_some_and(|namespace| namespace == "Rewired" || namespace.starts_with("Rewired."))
            && !target.path.contains("/Rewired/")
        {
            return false;
        }
        return true;
    }

    let target_namespace = target.namespace.as_deref().unwrap();
    file.namespace.as_deref() == Some(target_namespace)
        || imports_symbol_namespace(&file.imports, target_namespace, &target.name)
}

fn imports_symbol_namespace(imports: &[String], namespace: &str, name: &str) -> bool {
    let fully_qualified = format!("{namespace}.{name}");
    let wildcard = format!("{namespace}.*");
    imports
        .iter()
        .any(|import| import == namespace || import == &fully_qualified || import == &wildcard)
}

fn code_references_qualified_target(code: &str, target: &SymbolTarget) -> bool {
    let Some(namespace) = target.namespace.as_deref() else {
        return false;
    };
    let normalized = code
        .replace("::", ".")
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    normalized.contains(&format!("{namespace}.{}", target.name))
}

fn is_sibling_third_party_package(target_path: &str, hit_path: &str) -> bool {
    let Some(target_package) = third_party_package(target_path) else {
        return false;
    };
    let Some(hit_package) = third_party_package(hit_path) else {
        return false;
    };
    target_package != hit_package
}

fn third_party_package(path: &str) -> Option<&str> {
    let prefix = "Assets/Plugins/3rdPlugins/";
    let rest = path.strip_prefix(prefix)?;
    rest.split('/').next()
}

fn is_type_symbol_kind(kind: &str) -> bool {
    matches!(kind, "class" | "interface" | "struct" | "enum" | "record")
}

fn has_reference_occurrence(code: &str, target: &SymbolTarget) -> bool {
    for pos in whole_word_positions(code, &target.name) {
        let before = code[..pos].chars().rev().find(|ch| !ch.is_whitespace());
        if before == Some('.') {
            if let Some(namespace) = &target.namespace {
                let prefix = format!("{namespace}.");
                if code[..pos].trim_end().ends_with(&prefix) {
                    return true;
                }
            }
            continue;
        }
        return true;
    }
    false
}

fn has_type_reference_shape(code: &str, name: &str) -> bool {
    for pos in whole_word_positions(code, name) {
        if occurrence_has_type_context(code, name, pos) {
            return true;
        }
    }
    false
}

fn occurrence_has_type_context(code: &str, name: &str, pos: usize) -> bool {
    let before = code[..pos].trim_end();
    let after = code[pos + name.len()..].trim_start();
    let previous_word = previous_identifier(before);
    if matches!(
        previous_word.as_deref(),
        Some("new" | "typeof" | "nameof" | "is" | "as")
    ) {
        return true;
    }
    if before.ends_with("typeof(") || before.ends_with("nameof(") {
        return true;
    }
    if before
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, ':' | '<' | ',' | '('))
    {
        return true;
    }
    if after.starts_with('.') {
        return true;
    }
    let after = after
        .strip_prefix("[]")
        .unwrap_or(after)
        .strip_prefix('?')
        .unwrap_or(after)
        .trim_start();
    after
        .chars()
        .next()
        .is_some_and(|ch| ch == '@' || ch == '_' || ch.is_ascii_alphabetic())
}

fn whole_word_positions(haystack: &str, needle: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let pos = from + rel;
        let before = haystack[..pos].chars().next_back();
        let after = haystack[pos + needle.len()..].chars().next();
        let before_ok = before.is_none_or(|ch| !crate::tokens::is_identifier_char(ch));
        let after_ok = after.is_none_or(|ch| !crate::tokens::is_identifier_char(ch));
        if before_ok && after_ok {
            positions.push(pos);
        }
        from = pos + needle.len();
    }
    positions
}

fn previous_identifier(text: &str) -> Option<String> {
    let mut end = text.len();
    while end > 0 {
        let ch = text[..end].chars().next_back()?;
        if crate::tokens::is_identifier_char(ch) {
            break;
        }
        end -= ch.len_utf8();
    }
    let mut start = end;
    while start > 0 {
        let ch = text[..start].chars().next_back()?;
        if !crate::tokens::is_identifier_char(ch) {
            break;
        }
        start -= ch.len_utf8();
    }
    (start < end).then(|| text[start..end].to_string())
}

fn strip_strings_and_line_comment(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    let mut string_quote = '\0';
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            if escaped {
                escaped = false;
                out.push(' ');
                continue;
            }
            if ch == '\\' {
                escaped = true;
                out.push(' ');
                continue;
            }
            if ch == string_quote {
                in_string = false;
            }
            out.push(' ');
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if ch == '"' || ch == '\'' {
            in_string = true;
            string_quote = ch;
            out.push(' ');
            continue;
        }
        out.push(ch);
    }
    out
}

fn handle_hot(index: &Codebase, args: &Value) -> String {
    let limit = get_usize(args, "limit").unwrap_or(10).clamp(1, 1000);
    let mut files = index.files.values().collect::<Vec<_>>();
    files.sort_by(|a, b| {
        b.modified_unix_ms
            .cmp(&a.modified_unix_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut out = String::new();
    for (idx, file) in files.into_iter().take(limit).enumerate() {
        out.push_str(&format!("{}. {}\n", idx + 1, file.path));
    }
    out
}

fn handle_deps(index: &Codebase, args: &Value) -> Result<String> {
    let path = normalize_rel_path(&required_str(args, "path")?);
    let direction = get_str(args, "direction").unwrap_or_else(|| "imported_by".to_string());
    let transitive = get_bool(args, "transitive");
    let max_depth = get_usize(args, "max_depth");
    let forward = direction == "depends_on";
    let results = if transitive {
        transitive_deps(index, &path, forward, max_depth)
    } else if forward {
        index.deps_for(&path)
    } else {
        index.reverse_deps_for(&path)
    };

    let mut out = if forward {
        if transitive {
            format!("{path} transitively depends on:\n")
        } else {
            format!("{path} depends on:\n")
        }
    } else if transitive {
        format!("{path} is transitively imported by:\n")
    } else {
        format!("{path} is imported by:\n")
    };
    if results.is_empty() {
        out.push_str("  (none)\n");
        if !index.files.contains_key(&path) {
            out.push_str(&fuzzy_suggestions(index, &path));
        }
    } else {
        for result in &results {
            out.push_str(&format!("  {result}\n"));
        }
        out.push_str(&format!("({} files)\n", results.len()));
    }
    Ok(out)
}

fn handle_read(index: &Codebase, args: &Value) -> Result<String> {
    let path = normalize_rel_path(&required_str(args, "path")?);
    if path.contains("..") || Path::new(&path).is_absolute() {
        return Ok("error: path traversal not allowed".to_string());
    }
    let Some(file) = index.file(&path) else {
        return Ok(format!(
            "error: file not indexed: {path}\n{}",
            fuzzy_suggestions(index, &path)
        ));
    };
    let content = index.file_content(file)?;
    let hash = hash_content(&content);
    if get_str(args, "if_hash").as_deref() == Some(hash.as_str()) {
        return Ok(format!("unchanged:{hash}"));
    }
    let compact = get_bool(args, "compact");
    let start = get_usize(args, "line_start").unwrap_or(1);
    let end = get_usize(args, "line_end").unwrap_or(file.line_count.max(1));
    if start == 0 || end == 0 {
        return Ok("error: line_start and line_end must be >= 1".to_string());
    }
    if start > end {
        return Ok(format!("error: line_start ({start}) > line_end ({end})"));
    }
    let mut out = format!("hash:{hash}\n");
    if start != 1 || end != file.line_count || compact {
        out.push_str(&extract_lines(&content, start, end, compact));
    } else {
        out.push_str(&content);
    }
    Ok(out)
}

fn handle_changes(index: &Codebase, args: &Value) -> String {
    let since = get_u64(args, "since").unwrap_or(0);
    let mut out = if since < index.seq {
        format!(
            "seq: {}, {} files changed since {}:\n",
            index.seq,
            index.changed_files.len(),
            since
        )
    } else {
        format!("seq: {}, 0 files changed since {}:\n", index.seq, since)
    };
    if since < index.seq {
        for file in &index.changed_files {
            out.push_str(&format!(
                "  {} (seq={}, op={}, size={})\n",
                file.path, index.seq, file.op, file.size
            ));
        }
    }
    out
}

fn handle_status(index: &Codebase) -> String {
    let stats = index.stats();
    format!(
        "codedb status:\n  seq: {}\n  files: {}\n  outlines: {}\n  chunks: {}\n  graph: {} nodes, {} edges, {} communities\n  vector_index: lazy flat cosine ({} {} vectors)\n  embedding_model: model2vec-rs {} ({} dims)\n  scan: {}\n  extensions: {}\n  cache: {}\n  storage: {}\n",
        stats.seq,
        stats.files,
        stats.files,
        stats.chunks,
        stats.graph_nodes,
        stats.graph_edges,
        stats.graph_communities,
        stats.vector_count,
        stats.vector_units,
        stats.embedding_model,
        stats.embedding_dims,
        stats.scan,
        stats.extensions.join(","),
        stats.cache,
        stats.storage_dir.as_deref().unwrap_or("disabled")
    )
}

fn handle_snapshot(index: &Codebase) -> String {
    let graph = index.graph_summary();
    let snapshot = json!({
        "root": index.root.display().to_string(),
        "seq": index.seq,
        "stats": index.stats(),
        "files": index.files.values().collect::<Vec<_>>(),
        "deps": {
            "forward": index.deps_forward_snapshot(),
            "reverse": index.deps_reverse_snapshot(),
        },
        "graph": {
            "nodes": graph.nodes,
            "edges": graph.edges,
            "communities": graph.communities,
        },
    });
    serde_json::to_string_pretty(&snapshot)
        .unwrap_or_else(|err| format!("error: snapshot serialization failed: {err}"))
}

fn handle_find(index: &Codebase, args: &Value) -> Result<String> {
    let query = required_str(args, "query")?;
    let max_results = get_usize(args, "max_results").unwrap_or(10).clamp(1, 50);
    let mut matches = index
        .files
        .keys()
        .filter_map(|path| fuzzy_score(path, &query).map(|score| (path.clone(), score)))
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    matches.truncate(max_results);
    if matches.is_empty() {
        return Ok("no matches".to_string());
    }
    let mut out = String::new();
    for (idx, (path, score)) in matches.into_iter().enumerate() {
        out.push_str(&format!("{}. {} (score: {:.2})\n", idx + 1, path, score));
    }
    Ok(out)
}

fn handle_glob(index: &Codebase, args: &Value) -> Result<String> {
    let pattern = required_str(args, "pattern")?;
    let max_results = get_usize(args, "max_results").unwrap_or(200).clamp(1, 5000);
    let glob = build_globset(&pattern)?;
    let mut matches = index
        .files
        .keys()
        .filter(|path| glob.is_match(path.as_str()))
        .take(max_results)
        .cloned()
        .collect::<Vec<_>>();
    matches.sort();
    if matches.is_empty() {
        return Ok("no matches".to_string());
    }
    Ok(matches.join("\n") + "\n")
}

fn handle_ls(index: &Codebase, args: &Value) -> Result<String> {
    let prefix = get_str(args, "path").unwrap_or_default();
    let prefix = normalize_dir_prefix(&prefix);
    let mut dirs = BTreeSet::new();
    let mut files = Vec::new();
    for file in index.files.values() {
        if !file.path.starts_with(&prefix) {
            continue;
        }
        let rest = &file.path[prefix.len()..];
        if rest.is_empty() {
            continue;
        }
        if let Some((dir, _)) = rest.split_once('/') {
            dirs.insert(dir.to_string());
        } else {
            files.push(file);
        }
    }
    let mut out = String::new();
    for dir in dirs {
        out.push_str(&format!("{dir}/\n"));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for file in files {
        let name = file.path.rsplit('/').next().unwrap_or(&file.path);
        out.push_str(&format!(
            "{}  ({}, {}L, {} sym)\n",
            name,
            file.language,
            file.line_count,
            file.symbols.len()
        ));
    }
    if out.is_empty() {
        Ok("no entries".to_string())
    } else {
        Ok(out)
    }
}

fn handle_query(index: &Codebase, args: &Value) -> Result<String> {
    let Some(pipeline) = args.get("pipeline").and_then(Value::as_array) else {
        return Ok("error: missing 'pipeline'".to_string());
    };
    let mut paths: Option<BTreeSet<String>> = None;
    let mut out = String::new();
    for step in pipeline {
        let op = get_str(step, "op").unwrap_or_default();
        match op.as_str() {
            "find" => {
                let query = required_str(step, "query")?;
                let max = get_usize(step, "max_results").unwrap_or(50);
                let mut found = index
                    .files
                    .keys()
                    .filter_map(|path| fuzzy_score(path, &query).map(|score| (path.clone(), score)))
                    .collect::<Vec<_>>();
                found.sort_by(|a, b| b.1.total_cmp(&a.1));
                paths = Some(found.into_iter().take(max).map(|(path, _)| path).collect());
            }
            "search" => {
                let query = required_str(step, "query")?;
                let max = get_usize(step, "max_results").unwrap_or(50);
                let selector = paths
                    .as_ref()
                    .map(|paths| chunk_selector_for_paths(index, paths));
                let hits = hybrid_search(index, &query, max, selector.as_deref())?;
                paths = Some(
                    hits.iter()
                        .map(|hit| index.chunk_file_path(&hit.chunk).to_string())
                        .collect(),
                );
                out = format_chunk_hits(index, &query, hits)?;
            }
            "filter" => {
                let pattern = required_str(step, "path_glob")?;
                let glob = build_globset(&pattern)?;
                let current = paths
                    .take()
                    .unwrap_or_else(|| index.files.keys().cloned().collect());
                paths = Some(
                    current
                        .into_iter()
                        .filter(|path| glob.is_match(path))
                        .collect(),
                );
            }
            "limit" => {
                let limit = get_usize(step, "n")
                    .or_else(|| get_usize(step, "limit"))
                    .unwrap_or(10);
                if let Some(current) = paths.take() {
                    paths = Some(current.into_iter().take(limit).collect());
                }
            }
            "outline" => {
                let mut text = String::new();
                for path in paths.clone().unwrap_or_default().iter().take(20) {
                    text.push_str(&handle_outline(
                        index,
                        &json!({ "path": path, "compact": true }),
                    )?);
                }
                out = text;
            }
            _ => return Ok(format!("error: unsupported pipeline op: {op}")),
        }
    }
    if !out.is_empty() {
        return Ok(out);
    }
    if let Some(paths) = paths {
        return Ok(paths.into_iter().collect::<Vec<_>>().join("\n") + "\n");
    }
    Ok("pipeline completed with no output".to_string())
}

fn handle_graph(index: &Codebase, args: &Value) -> Result<String> {
    let format = get_str(args, "format").unwrap_or_else(|| "summary".to_string());
    let max_nodes = get_usize(args, "max_nodes").unwrap_or(1000);
    let max_edges = get_usize(args, "max_edges").unwrap_or(3000);
    let graph = index.graph();
    match format.as_str() {
        "summary" => {
            let stats = graph.stats();
            let analysis = graph.analysis(10);
            let mut out = format!(
                "code graph:\n  nodes: {}\n  edges: {}\n  communities: {}\n  isolated_nodes: {}\n  average_degree: {:.2}\n",
                stats.nodes,
                stats.edges,
                stats.communities,
                stats.isolated_nodes,
                stats.average_degree
            );
            out.push_str("top nodes:\n");
            for node in analysis.top_nodes.iter().take(10) {
                out.push_str(&format!(
                    "  {} ({}, degree {}, community {:?}) {}\n",
                    node.label,
                    node.node_type,
                    node.degree,
                    node.community,
                    node.file_path.as_deref().unwrap_or("")
                ));
            }
            out.push_str("top relations:\n");
            for item in analysis.relation_counts.iter().take(8) {
                out.push_str(&format!("  {}: {}\n", item.name, item.count));
            }
            Ok(out)
        }
        "json" => serde_json::to_string_pretty(&graph.limited_json(max_nodes, max_edges))
            .map_err(Into::into),
        "graphml" => Ok(graph.to_graphml(max_nodes, max_edges)),
        "cypher" | "neo4j" => Ok(graph.to_cypher(max_nodes, max_edges)),
        other => Ok(format!(
            "error: unsupported graph format: {other}; use summary, json, graphml, or cypher"
        )),
    }
}

fn handle_explain(index: &Codebase, args: &Value) -> Result<String> {
    let term = get_str(args, "node")
        .or_else(|| get_str(args, "query"))
        .ok_or_else(|| anyhow!("missing 'node' argument"))?;
    let limit = get_usize(args, "limit").unwrap_or(20).clamp(1, 200);
    let graph = index.graph();
    match graph.explain(&term, limit) {
        Some(result) => serde_json::to_string_pretty(&result).map_err(Into::into),
        None => Ok(format!("error: graph node not found: {term}")),
    }
}

fn handle_path(index: &Codebase, args: &Value) -> Result<String> {
    let source = get_str(args, "source")
        .or_else(|| get_str(args, "from"))
        .ok_or_else(|| anyhow!("missing 'source' argument"))?;
    let target = get_str(args, "target")
        .or_else(|| get_str(args, "to"))
        .ok_or_else(|| anyhow!("missing 'target' argument"))?;
    let max_depth = get_usize(args, "max_depth").unwrap_or(8).clamp(1, 32);
    let graph = index.graph();
    let result = graph.shortest_path(&source, &target, max_depth);
    serde_json::to_string_pretty(&result).map_err(Into::into)
}

fn handle_communities(index: &Codebase, args: &Value) -> Result<String> {
    let community_id = get_usize(args, "community_id").or_else(|| get_usize(args, "id"));
    let limit = get_usize(args, "limit").unwrap_or(20).clamp(1, 500);
    let community_limit = get_usize(args, "community_limit")
        .unwrap_or(100)
        .clamp(1, 5000);
    let include_members = get_bool(args, "include_members") || get_bool(args, "members");
    let include_children = get_bool(args, "children") || get_bool(args, "subcommunities");
    let child_id = get_usize(args, "child_id").or_else(|| get_usize(args, "subcommunity_id"));
    ensure_louvain_communities(index);
    let graph = index.graph();
    let communities = index.louvain_communities.read();
    let communities = communities
        .as_ref()
        .ok_or_else(|| anyhow!("failed to initialize Louvain communities"))?;

    if include_children {
        let Some(id) = community_id else {
            return Ok("error: children=true requires community_id or id".to_string());
        };
        let Some(parent) = communities.iter().find(|community| community.id == id) else {
            return Ok(format!("error: community not found: {id}"));
        };
        let subcommunities = ensure_louvain_subcommunities(index, parent);
        return serde_json::to_string_pretty(&graph.subcommunities_summary_for(
            parent,
            &subcommunities,
            child_id,
            limit,
            community_limit,
            include_members,
        ))
        .map_err(Into::into);
    }

    serde_json::to_string_pretty(&graph.communities_summary_for(
        communities,
        "lazy-louvain",
        community_id,
        limit,
        community_limit,
        include_members,
    ))
    .map_err(Into::into)
}

#[derive(Debug)]
struct ModuleRaw {
    community_id: usize,
    fallback_label: String,
    files: Vec<String>,
    token_counts: BTreeMap<String, usize>,
    symbol_count: usize,
    internal_deps: usize,
    outgoing_deps: usize,
    incoming_deps: usize,
}

#[derive(Debug)]
struct ModuleCandidate {
    score: f32,
    file_count: usize,
    value: Value,
}

fn handle_module_map(index: &Codebase, args: &Value) -> Result<String> {
    let limit = get_usize(args, "limit").unwrap_or(40).clamp(1, 200);
    let min_files = get_usize(args, "min_files").unwrap_or(2).clamp(1, 1000);
    let max_files_per_module = get_usize(args, "max_files_per_module")
        .unwrap_or(40)
        .clamp(1, 1000);
    let include_files = get_bool(args, "include_files");
    let semantic_neighbors = get_usize(args, "semantic_neighbors")
        .unwrap_or(5)
        .clamp(0, 20);
    let path_prefix = get_str(args, "path_prefix")
        .and_then(|prefix| (!prefix.trim().is_empty()).then(|| normalize_dir_prefix(&prefix)));

    let raws = build_file_module_raws(index, min_files, path_prefix.as_deref());
    let term_document_frequency = module_term_document_frequency(&raws);
    let total_modules = raws.len();
    let mut candidates = Vec::new();

    for raw in raws {
        let terms = ranked_module_terms(&raw.token_counts, &term_document_frequency, total_modules);
        let label = module_label(&terms, &raw.fallback_label);
        let file_set = raw.files.iter().cloned().collect::<BTreeSet<_>>();
        let central_files = central_files_for_module(index, &file_set, 8);
        let key_symbols = key_symbols_for_module(index, &file_set, 12);
        let entry_points = entry_points_for_module(index, &file_set, 12);
        let path_roots = module_path_roots(&raw.files, 8);
        let semantic_query = semantic_query_for_module(&label, &terms, &key_symbols);
        let (semantic_items, semantic_density) =
            semantic_neighbors_for_module(index, &semantic_query, &file_set, semantic_neighbors)?;
        let boundary_deps = raw.outgoing_deps + raw.incoming_deps;
        let cohesion = dependency_cohesion(raw.internal_deps, boundary_deps);
        let cross_folder = path_roots.len() > 1;
        let score = module_confidence_score(
            raw.files.len(),
            cohesion,
            semantic_density,
            entry_points.len(),
            cross_folder,
        );
        let files_value = if include_files {
            json!(
                raw.files
                    .iter()
                    .take(max_files_per_module)
                    .collect::<Vec<_>>()
            )
        } else {
            Value::Null
        };
        let evidence = module_evidence(
            raw.files.len(),
            raw.symbol_count,
            raw.internal_deps,
            boundary_deps,
            cohesion,
            cross_folder,
            semantic_density,
        );

        let value = json!({
            "id": raw.community_id,
            "label": label,
            "source": "rust dependency-connected file graph + label propagation + dependency evidence",
            "confidence": score,
            "file_count": raw.files.len(),
            "symbol_count": raw.symbol_count,
            "cohesion": cohesion,
            "dependency_edges": {
                "internal": raw.internal_deps,
                "outgoing": raw.outgoing_deps,
                "incoming": raw.incoming_deps,
                "boundary": boundary_deps,
            },
            "cross_folder": cross_folder,
            "path_roots": path_roots,
            "terms": terms
                .iter()
                .take(8)
                .map(|(term, score, count)| json!({"term": term, "score": score, "count": count}))
                .collect::<Vec<_>>(),
            "entry_points": entry_points,
            "key_symbols": key_symbols,
            "central_files": central_files,
            "semantic_query": semantic_query,
            "semantic_density": semantic_density,
            "semantic_neighbors": semantic_items,
            "files": files_value,
            "evidence": evidence,
        });
        candidates.push(ModuleCandidate {
            score,
            file_count: raw.files.len(),
            value,
        });
    }

    candidates.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| b.file_count.cmp(&a.file_count))
            .then_with(|| {
                a.value
                    .get("label")
                    .and_then(Value::as_str)
                    .cmp(&b.value.get("label").and_then(Value::as_str))
            })
    });
    let returned = candidates.len().min(limit);
    let modules = candidates
        .into_iter()
        .take(limit)
        .map(|candidate| candidate.value)
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&json!({
        "algorithm": "module-atlas-v2-rust-dependency-components",
        "purpose": "DeepWiki module planning evidence; final page boundaries should be decided by the agent.",
        "inspired_by": [
            "Understand-Anything: separate business-domain planning from structural graph facts",
            "Understand-Anything: semantic batching and neighbor maps to preserve cross-boundary context",
            "Embedding Atlas: semantic-neighbor probes, density-style confidence, and c-TF-IDF-like labels"
        ],
        "scope": {
            "path_prefix": path_prefix.as_deref().unwrap_or(""),
            "min_files": min_files,
            "include_files": include_files,
            "semantic_neighbors": semantic_neighbors,
        },
        "total_modules": total_modules,
        "returned_modules": returned,
        "modules": modules,
    }))
    .map_err(Into::into)
}

fn handle_module_atlas(index: &Codebase, args: &Value) -> Result<String> {
    let started = Instant::now();
    let limit = get_usize(args, "limit").unwrap_or(2000).clamp(1, 5000);
    let min_files = get_usize(args, "min_files").unwrap_or(2).clamp(1, 1000);
    let include_files = get_bool(args, "include_files");
    let split_files = get_bool(args, "split_files");
    let path_prefix = get_str(args, "path_prefix")
        .and_then(|prefix| (!prefix.trim().is_empty()).then(|| normalize_dir_prefix(&prefix)));
    let raws = build_file_module_raws(index, min_files, path_prefix.as_deref());
    let term_document_frequency = module_term_document_frequency(&raws);
    let total_modules = raws.len();
    let mut modules = raws
        .into_iter()
        .map(|raw| {
            let terms =
                ranked_module_terms(&raw.token_counts, &term_document_frequency, total_modules);
            let label = module_label(&terms, &raw.fallback_label);
            let file_set = raw.files.iter().cloned().collect::<BTreeSet<_>>();
            let central_files = central_files_for_module_camel(index, &file_set, 5);
            let key_symbols = key_symbols_for_module_camel(index, &file_set, 4);
            let entry_points = entry_points_for_module_camel(index, &file_set, 5);
            let path_roots = module_path_roots_camel(&raw.files, 10);
            let boundary_deps = raw.outgoing_deps + raw.incoming_deps;
            let cohesion = dependency_cohesion(raw.internal_deps, boundary_deps);
            let semantic_density = module_semantic_density(&terms, raw.symbol_count);
            let confidence = module_confidence_score(
                raw.files.len(),
                cohesion,
                semantic_density,
                entry_points.len(),
                path_roots.len() > 1,
            );
            ModuleAtlasModule {
                community_id: raw.community_id,
                label,
                file_count: raw.files.len(),
                symbol_count: raw.symbol_count,
                confidence,
                cohesion,
                semantic_density,
                cross_folder: path_roots.len() > 1,
                language_counts: module_language_counts(index, &raw.files),
                terms: terms
                    .iter()
                    .take(6)
                    .map(|(term, score, count)| {
                        json!({"term": term, "score": score, "count": count})
                    })
                    .collect(),
                path_roots,
                entry_points,
                key_symbols,
                central_files,
                files: raw.files,
            }
        })
        .collect::<Vec<_>>();
    modules.sort_by(|a, b| {
        b.file_count
            .cmp(&a.file_count)
            .then_with(|| b.confidence.total_cmp(&a.confidence))
            .then_with(|| a.label.cmp(&b.label))
    });
    modules.truncate(limit);
    for (id, module) in modules.iter_mut().enumerate() {
        module.community_id = id;
    }

    let layouts = module_layouts(index, &modules);
    let mut path_to_point_id = HashMap::<String, usize>::new();
    for module in &modules {
        for path in &module.files {
            if index.files.contains_key(path) && !path_to_point_id.contains_key(path) {
                let id = path_to_point_id.len();
                path_to_point_id.insert(path.clone(), id);
            }
        }
    }
    let mut points = Vec::new();
    for module in &modules {
        let layout = layouts
            .get(&module.community_id)
            .copied()
            .unwrap_or(ModuleLayout {
                x: 0.0,
                y: 0.0,
                radius: module_layout_radius(module.file_count),
            });
        let local_offsets = module_file_offsets(index, module, layout.radius);
        for path in &module.files {
            let Some(file) = index.files.get(path) else {
                continue;
            };
            let point_id = path_to_point_id.get(path).copied().unwrap_or(points.len());
            let local = local_offsets.get(path).copied().unwrap_or((0.0, 0.0));
            let dep_in = index.reverse_deps_for(path);
            let dep_out = index.deps_for(path);
            points.push(json!({
                "id": point_id,
                "path": path,
                "language": file.language,
                "languageLabel": language_label(file.language.as_str()),
                "moduleId": module.community_id,
                "moduleLabel": module.label,
                "x": layout.x + local.0,
                "y": layout.y + local.1,
                "category": module.community_id % 12,
                "symbols": file.symbols.iter().take(12).map(|symbol| symbol.name.clone()).collect::<Vec<_>>(),
                "lineCount": file.line_count,
                "depIn": dep_in.len(),
                "depOut": dep_out.len(),
                "depInIds": atlas_dependency_ids(Some(&dep_in), &path_to_point_id, 80),
                "depOutIds": atlas_dependency_ids(Some(&dep_out), &path_to_point_id, 80),
            }));
        }
    }

    let modules_json = modules
        .iter()
        .map(|module| {
            let files = if include_files {
                json!(module.files)
            } else {
                Value::Null
            };
            let layout = layouts
                .get(&module.community_id)
                .copied()
                .unwrap_or(ModuleLayout {
                    x: 0.0,
                    y: 0.0,
                    radius: module_layout_radius(module.file_count),
                });
            json!({
                "id": module.community_id,
                "label": module.label,
                "fileCount": module.file_count,
                "symbolCount": module.symbol_count,
                "confidence": module.confidence,
                "cohesion": module.cohesion,
                "semanticDensity": module.semantic_density,
                "crossFolder": module.cross_folder,
                "languageCounts": module.language_counts,
                "terms": module.terms,
                "pathRoots": module.path_roots,
                "entryPoints": module.entry_points,
                "keySymbols": module.key_symbols,
                "centralFiles": module.central_files,
                "layout": {
                    "x": round2_local(layout.x),
                    "y": round2_local(layout.y),
                    "radius": round2_local(layout.radius),
                },
                "files": files,
            })
        })
        .collect::<Vec<_>>();
    let graph = index.graph_summary();
    let mut metadata = json!({
        "project": index.root.file_name().and_then(|name| name.to_str()).unwrap_or("project"),
        "root": index.root.display().to_string().replace('\\', "/"),
        "generatedAt": chrono_like_timestamp(),
        "extensions": index.options.extensions.clone(),
        "languages": language_counts(index).keys().cloned().collect::<Vec<_>>(),
        "languageCounts": language_counts(index),
        "totalFiles": index.files.len(),
        "totalModules": modules_json.len(),
        "graph": {
            "nodes": graph.nodes,
            "edges": graph.edges,
        },
        "algorithm": "rust dependency-connected file graph + label propagation",
        "generationMs": started.elapsed().as_millis() as u64,
        "projection": "dependency-aware organic module layout + dependency-aware intra-module file layout; rendered by embedding-atlas EmbeddingView",
    });
    if split_files {
        metadata["pointsPath"] = Value::String("module-atlas-points.json".to_string());
    }

    let data = json!({
        "metadata": {
            "project": metadata["project"].clone(),
            "root": metadata["root"].clone(),
            "generatedAt": metadata["generatedAt"].clone(),
            "extensions": metadata["extensions"].clone(),
            "languages": metadata["languages"].clone(),
            "languageCounts": metadata["languageCounts"].clone(),
            "totalFiles": metadata["totalFiles"].clone(),
            "totalModules": metadata["totalModules"].clone(),
            "graph": metadata["graph"].clone(),
            "algorithm": metadata["algorithm"].clone(),
            "generationMs": metadata["generationMs"].clone(),
            "projection": metadata["projection"].clone(),
            "pointsPath": metadata.get("pointsPath").cloned().unwrap_or(Value::Null),
        },
        "modules": modules_json,
        "points": if split_files { Value::Null } else { Value::Array(points.clone()) },
    });
    let content = serde_json::to_string(&data)?;
    if let Some(output_path) = get_str(args, "output_path") {
        let output_path = resolve_output_path(&index.root, &output_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&output_path, content)?;
        if split_files {
            let points_path = output_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("module-atlas-points.json");
            fs::write(&points_path, serde_json::to_string(&points)?)?;
        }
        Ok(format!(
            "exported module atlas to {}",
            output_path.display()
        ))
    } else {
        Ok(content)
    }
}

#[derive(Debug)]
struct ModuleAtlasModule {
    community_id: usize,
    label: String,
    file_count: usize,
    symbol_count: usize,
    confidence: f32,
    cohesion: f32,
    semantic_density: f32,
    cross_folder: bool,
    language_counts: Vec<Value>,
    terms: Vec<Value>,
    path_roots: Vec<Value>,
    entry_points: Vec<Value>,
    key_symbols: Vec<Value>,
    central_files: Vec<Value>,
    files: Vec<String>,
}

fn build_file_module_raws(
    index: &Codebase,
    min_files: usize,
    path_prefix: Option<&str>,
) -> Vec<ModuleRaw> {
    let allowed = index
        .files
        .keys()
        .filter(|path| path_prefix.is_none_or(|prefix| path_matches_prefix(path, prefix)))
        .cloned()
        .collect::<BTreeSet<_>>();
    if allowed.is_empty() {
        return Vec::new();
    }
    let communities = detect_file_dependency_modules(index, &allowed);
    communities
        .into_iter()
        .enumerate()
        .filter_map(|(community_id, files)| {
            if files.len() < min_files {
                return None;
            }
            let file_set = files.iter().cloned().collect::<BTreeSet<_>>();
            let token_counts = module_token_counts(index, &files);
            let symbol_count = files
                .iter()
                .filter_map(|path| index.files.get(path))
                .map(|file| file.symbols.len())
                .sum();
            let (internal_deps, outgoing_deps, incoming_deps) =
                module_dependency_counts(index, &file_set);
            Some(ModuleRaw {
                community_id,
                fallback_label: module_label_from_files(index, &files),
                files,
                token_counts,
                symbol_count,
                internal_deps,
                outgoing_deps,
                incoming_deps,
            })
        })
        .collect()
}

fn detect_file_dependency_modules(
    index: &Codebase,
    allowed: &BTreeSet<String>,
) -> Vec<Vec<String>> {
    let mut label_ids = BTreeMap::<String, usize>::new();
    let mut next_label_id = 0usize;
    let mut labels = HashMap::<String, usize>::new();
    let mut own_labels = HashMap::<String, usize>::new();
    for path in allowed {
        let label = index
            .files
            .get(path)
            .map(dominant_feature_for_file)
            .unwrap_or_else(|| module_path_root(path));
        let id = *label_ids.entry(label).or_insert_with(|| {
            let id = next_label_id;
            next_label_id += 1;
            id
        });
        labels.insert(path.clone(), id);
        own_labels.insert(path.clone(), id);
    }

    let reverse_deps = index.deps_reverse_snapshot();
    let hub_targets = reverse_deps
        .iter()
        .filter(|(path, sources)| {
            allowed.contains(*path) && sources.len() > MODULE_HUB_INCOMING_LIMIT
        })
        .map(|(path, _)| path.clone())
        .collect::<BTreeSet<_>>();
    let mut adjacency = HashMap::<String, Vec<(String, f32)>>::new();
    for path in allowed {
        let mut emitted = 0usize;
        for dep in index.deps_for(path) {
            if emitted >= MODULE_MAX_DEPENDENCY_EDGES_PER_FILE {
                break;
            }
            if !allowed.contains(&dep) || hub_targets.contains(&dep) || dep == *path {
                continue;
            }
            adjacency
                .entry(path.clone())
                .or_default()
                .push((dep.clone(), 6.0));
            adjacency
                .entry(dep.clone())
                .or_default()
                .push((path.clone(), 4.0));
            emitted += 1;
        }
    }

    let mut modules = Vec::new();
    for component in dependency_components(allowed, &adjacency) {
        let component_set = component.iter().cloned().collect::<BTreeSet<_>>();
        let mut order = component.clone();
        order.sort_by(|a, b| {
            adjacency
                .get(b)
                .map(Vec::len)
                .unwrap_or(0)
                .cmp(&adjacency.get(a).map(Vec::len).unwrap_or(0))
                .then_with(|| a.cmp(b))
        });
        for _ in 0..MODULE_LABEL_ITERATIONS {
            let mut changed = false;
            for path in &order {
                let mut votes = BTreeMap::<usize, f32>::new();
                if let Some(own) = own_labels.get(path).copied() {
                    *votes.entry(own).or_default() += 2.5;
                }
                for (neighbor, weight) in adjacency.get(path).into_iter().flatten() {
                    if !component_set.contains(neighbor) {
                        continue;
                    }
                    if let Some(label) = labels.get(neighbor).copied() {
                        *votes.entry(label).or_default() += *weight;
                    }
                }
                let Some((best, _)) = votes
                    .into_iter()
                    .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
                else {
                    continue;
                };
                if labels.get(path).copied() != Some(best) {
                    labels.insert(path.clone(), best);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let mut grouped = BTreeMap::<usize, Vec<String>>::new();
        for path in component {
            let label = labels.get(&path).copied().unwrap_or(0);
            grouped.entry(label).or_default().push(path);
        }
        for (_, files) in grouped {
            split_dependency_module_group(index, files, &mut modules);
        }
    }
    modules.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    modules
}

fn dependency_components(
    allowed: &BTreeSet<String>,
    adjacency: &HashMap<String, Vec<(String, f32)>>,
) -> Vec<Vec<String>> {
    let mut seen = BTreeSet::<String>::new();
    let mut components = Vec::new();
    for path in allowed {
        if seen.contains(path) {
            continue;
        }
        let mut queue = VecDeque::from([path.clone()]);
        let mut component = Vec::new();
        seen.insert(path.clone());
        while let Some(current) = queue.pop_front() {
            component.push(current.clone());
            for (neighbor, _) in adjacency.get(&current).into_iter().flatten() {
                if allowed.contains(neighbor) && seen.insert(neighbor.clone()) {
                    queue.push_back(neighbor.clone());
                }
            }
        }
        component.sort();
        components.push(component);
    }
    components.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    components
}

fn split_dependency_module_group(index: &Codebase, files: Vec<String>, out: &mut Vec<Vec<String>>) {
    if files.len() <= MODULE_MAX_FILES_PER_GROUP {
        out.push(files);
        return;
    }
    let mut by_feature = BTreeMap::<String, Vec<String>>::new();
    for path in files {
        let feature = index
            .files
            .get(&path)
            .map(dominant_feature_for_file)
            .unwrap_or_else(|| module_path_root(&path));
        by_feature.entry(feature).or_default().push(path);
    }
    for (_, mut group) in by_feature {
        group.sort();
        if group.len() <= MODULE_MAX_FILES_PER_GROUP {
            out.push(group);
        } else {
            for chunk in group.chunks(MODULE_MAX_FILES_PER_GROUP) {
                out.push(chunk.to_vec());
            }
        }
    }
}

fn dominant_feature_for_file(file: &FileEntry) -> String {
    top_terms_from_counts(&file_module_token_counts(file), 1)
        .into_iter()
        .next()
        .map(|(term, _)| term)
        .unwrap_or_else(|| module_path_root(&file.path))
}

fn module_label_from_files(index: &Codebase, files: &[String]) -> String {
    let counts = module_token_counts(index, files);
    let terms = top_terms_from_counts(&counts, 2)
        .into_iter()
        .map(|(term, _)| term)
        .collect::<Vec<_>>();
    if terms.is_empty() {
        files
            .first()
            .map(|path| module_path_root(path))
            .unwrap_or_else(|| "module".to_string())
    } else {
        terms.join("/")
    }
}

fn module_term_document_frequency(raws: &[ModuleRaw]) -> BTreeMap<String, usize> {
    let mut document_frequency = BTreeMap::new();
    for raw in raws {
        for term in raw.token_counts.keys() {
            *document_frequency.entry(term.clone()).or_default() += 1;
        }
    }
    document_frequency
}

fn ranked_module_terms(
    counts: &BTreeMap<String, usize>,
    document_frequency: &BTreeMap<String, usize>,
    total_modules: usize,
) -> Vec<(String, f32, usize)> {
    let total_modules = total_modules.max(1) as f32;
    let mut terms = counts
        .iter()
        .filter_map(|(term, count)| {
            let df = *document_frequency.get(term).unwrap_or(&1) as f32;
            let idf = (1.0 + total_modules / df).ln();
            let score = (*count as f32) * idf;
            (score > 0.0).then(|| (term.clone(), round2_local(score), *count))
        })
        .collect::<Vec<_>>();
    terms.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.2.cmp(&a.2)));
    terms
}

fn module_label(terms: &[(String, f32, usize)], fallback: &str) -> String {
    let selected = terms
        .iter()
        .map(|(term, _, _)| term.as_str())
        .take(2)
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if selected.is_empty() {
        fallback.to_string()
    } else {
        selected.join("/")
    }
}

fn module_token_counts(index: &Codebase, files: &[String]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for path in files {
        for part in path.split(['/', '.', '-', ' ', '+']) {
            add_module_token(part, 1, &mut counts);
        }
        let Some(file) = index.files.get(path) else {
            continue;
        };
        if let Some(namespace) = &file.namespace {
            for part in namespace.split('.') {
                add_module_token(part, 2, &mut counts);
            }
        }
        for symbol in &file.symbols {
            let weight = if matches!(
                symbol.kind.as_str(),
                "class" | "interface" | "struct" | "enum" | "record"
            ) {
                4
            } else {
                2
            };
            for part in split_identifier(&symbol.name) {
                add_module_token(&part, weight, &mut counts);
            }
        }
    }
    counts
}

fn add_module_token(token: &str, weight: usize, counts: &mut BTreeMap<String, usize>) {
    for part in split_identifier(token) {
        if is_module_token(&part) {
            *counts.entry(part).or_default() += weight;
        }
    }
}

fn is_module_token(token: &str) -> bool {
    token.len() >= 3
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
        && !token.chars().all(|ch| ch.is_ascii_digit())
        && !MODULE_TERM_STOPWORDS.contains(&token)
}

fn module_dependency_counts(
    index: &Codebase,
    file_paths: &BTreeSet<String>,
) -> (usize, usize, usize) {
    let mut internal = 0usize;
    let mut outgoing = 0usize;
    let mut incoming = 0usize;
    for path in file_paths {
        for dep in index.deps_for(path) {
            if file_paths.contains(&dep) {
                internal += 1;
            } else {
                outgoing += 1;
            }
        }
        for source in index.reverse_deps_for(path) {
            if !file_paths.contains(&source) {
                incoming += 1;
            }
        }
    }
    (internal, outgoing, incoming)
}

fn dependency_cohesion(internal: usize, boundary: usize) -> f32 {
    let total = internal + boundary;
    if total == 0 {
        0.0
    } else {
        round2_local(internal as f32 / total as f32)
    }
}

fn central_files_for_module(
    index: &Codebase,
    file_paths: &BTreeSet<String>,
    limit: usize,
) -> Vec<Value> {
    let mut items = file_paths
        .iter()
        .filter_map(|path| {
            let file = index.files.get(path)?;
            let outgoing = index.deps_for(path);
            let internal_out = outgoing
                .iter()
                .filter(|dep| file_paths.contains(*dep))
                .count();
            let incoming = index.reverse_deps_for(path);
            let internal_in = incoming
                .iter()
                .filter(|source| file_paths.contains(*source))
                .count();
            let external_out = outgoing
                .iter()
                .filter(|dep| !file_paths.contains(*dep))
                .count();
            let external_in = incoming
                .iter()
                .filter(|source| !file_paths.contains(*source))
                .count();
            let score = internal_in * 3
                + internal_out * 3
                + external_in
                + external_out
                + file.symbols.len();
            Some((
                score,
                json!({
                    "path": path,
                    "language": file.language,
                    "line_count": file.line_count,
                    "symbols": file.symbols.len(),
                    "internal_edges": internal_in + internal_out,
                    "external_edges": external_in + external_out,
                }),
            ))
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items
        .into_iter()
        .take(limit)
        .map(|(_, value)| value)
        .collect()
}

fn central_files_for_module_camel(
    index: &Codebase,
    file_paths: &BTreeSet<String>,
    limit: usize,
) -> Vec<Value> {
    central_files_for_module(index, file_paths, limit)
        .into_iter()
        .map(|item| {
            json!({
                "path": item.get("path").cloned().unwrap_or(Value::Null),
                "language": item.get("language").cloned().unwrap_or(Value::Null),
                "lineCount": item.get("line_count").cloned().unwrap_or(Value::Null),
                "symbols": item.get("symbols").cloned().unwrap_or(Value::Null),
                "internalEdges": item.get("internal_edges").cloned().unwrap_or(Value::Null),
                "externalEdges": item.get("external_edges").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn key_symbols_for_module(
    index: &Codebase,
    file_paths: &BTreeSet<String>,
    limit: usize,
) -> Vec<Value> {
    let mut items = Vec::new();
    for path in file_paths {
        let Some(file) = index.files.get(path) else {
            continue;
        };
        for symbol in &file.symbols {
            let score = symbol_importance(symbol.kind.as_str(), &symbol.name);
            if score == 0 {
                continue;
            }
            items.push((
                score,
                json!({
                    "path": path,
                    "line": symbol.line_start,
                    "kind": symbol.kind,
                    "name": symbol.name,
                }),
            ));
        }
    }
    items.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            a.1.get("name")
                .and_then(Value::as_str)
                .cmp(&b.1.get("name").and_then(Value::as_str))
        })
    });
    items
        .into_iter()
        .take(limit)
        .map(|(_, value)| value)
        .collect()
}

fn key_symbols_for_module_camel(
    index: &Codebase,
    file_paths: &BTreeSet<String>,
    limit: usize,
) -> Vec<Value> {
    key_symbols_for_module(index, file_paths, limit)
        .into_iter()
        .map(|item| {
            let path = item.get("path").and_then(Value::as_str).unwrap_or("");
            let language = index
                .files
                .get(path)
                .map(|file| language_label(file.language.as_str()))
                .unwrap_or("");
            json!({
                "path": item.get("path").cloned().unwrap_or(Value::Null),
                "line": item.get("line").cloned().unwrap_or(Value::Null),
                "language": language,
                "kind": item.get("kind").cloned().unwrap_or(Value::Null),
                "name": item.get("name").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn entry_points_for_module(
    index: &Codebase,
    file_paths: &BTreeSet<String>,
    limit: usize,
) -> Vec<Value> {
    let mut items = Vec::new();
    for path in file_paths {
        let Some(file) = index.files.get(path) else {
            continue;
        };
        for symbol in &file.symbols {
            let score = entry_point_score(symbol.kind.as_str(), &symbol.name, path);
            if score == 0 {
                continue;
            }
            items.push((
                score,
                json!({
                    "path": path,
                    "line": symbol.line_start,
                    "kind": symbol.kind,
                    "name": symbol.name,
                }),
            ));
        }
    }
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items
        .into_iter()
        .take(limit)
        .map(|(_, value)| value)
        .collect()
}

fn entry_points_for_module_camel(
    index: &Codebase,
    file_paths: &BTreeSet<String>,
    limit: usize,
) -> Vec<Value> {
    entry_points_for_module(index, file_paths, limit)
        .into_iter()
        .map(|item| {
            let path = item.get("path").and_then(Value::as_str).unwrap_or("");
            let language = index
                .files
                .get(path)
                .map(|file| language_label(file.language.as_str()))
                .unwrap_or("");
            json!({
                "path": item.get("path").cloned().unwrap_or(Value::Null),
                "line": item.get("line").cloned().unwrap_or(Value::Null),
                "language": language,
                "kind": item.get("kind").cloned().unwrap_or(Value::Null),
                "name": item.get("name").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

fn symbol_importance(kind: &str, name: &str) -> usize {
    let kind_score = match kind {
        "class" | "interface" | "struct" | "enum" | "record" => 8,
        "method" | "constructor" | "function" => 4,
        "property" | "field" => 2,
        _ => 1,
    };
    kind_score + business_name_bonus(name)
}

fn entry_point_score(kind: &str, name: &str, path: &str) -> usize {
    let lower = name.to_ascii_lowercase();
    let path_lower = path.to_ascii_lowercase();
    let mut score = 0usize;
    if matches!(
        kind,
        "class" | "interface" | "struct" | "record" | "method" | "function" | "constructor"
    ) {
        score += business_name_bonus(name);
    }
    if matches!(
        lower.as_str(),
        "awake" | "start" | "init" | "initialize" | "run" | "execute" | "process" | "update"
    ) {
        score += 4;
    }
    if lower.starts_with("on") || lower.starts_with("handle") {
        score += 3;
    }
    if path_lower.contains("/controller/")
        || path_lower.contains("/controllers/")
        || path_lower.contains("/manager/")
        || path_lower.contains("/managers/")
        || path_lower.contains("/service/")
        || path_lower.contains("/services/")
        || path_lower.contains("/view/")
        || path_lower.contains("/views/")
    {
        score += 2;
    }
    score
}

fn business_name_bonus(name: &str) -> usize {
    let lower = name.to_ascii_lowercase();
    let mut score = 0usize;
    for suffix in [
        "manager",
        "controller",
        "service",
        "listener",
        "handler",
        "router",
        "view",
        "panel",
        "system",
        "facade",
        "module",
        "model",
        "repository",
        "processor",
        "factory",
    ] {
        if lower.contains(suffix) {
            score += 3;
        }
    }
    score
}

fn module_path_roots(files: &[String], limit: usize) -> Vec<Value> {
    let mut counts = BTreeMap::<String, usize>::new();
    for path in files {
        *counts.entry(module_path_root(path)).or_default() += 1;
    }
    let mut items = counts.into_iter().collect::<Vec<_>>();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    items
        .into_iter()
        .take(limit)
        .map(|(path, files)| json!({"path": path, "files": files}))
        .collect()
}

fn module_path_roots_camel(files: &[String], limit: usize) -> Vec<Value> {
    module_path_roots(files, limit)
}

fn module_path_root(path: &str) -> String {
    let parts = path.split('/').collect::<Vec<_>>();
    let dir_count = parts.len().saturating_sub(1);
    let depth = dir_count.min(5);
    if depth == 0 {
        String::new()
    } else {
        parts[..depth].join("/")
    }
}

fn semantic_query_for_module(
    label: &str,
    terms: &[(String, f32, usize)],
    key_symbols: &[Value],
) -> String {
    let mut pieces = Vec::new();
    pieces.push(label.to_string());
    pieces.extend(terms.iter().take(6).map(|(term, _, _)| term.clone()));
    pieces.extend(
        key_symbols
            .iter()
            .take(4)
            .filter_map(|item| item.get("name").and_then(Value::as_str).map(str::to_string)),
    );
    pieces.join(" ")
}

fn semantic_neighbors_for_module(
    index: &Codebase,
    query: &str,
    file_paths: &BTreeSet<String>,
    limit: usize,
) -> Result<(Vec<Value>, f32)> {
    if limit == 0 || query.trim().is_empty() {
        return Ok((Vec::new(), 0.0));
    }
    let model = index.embedding_model()?;
    let query_vec = model.encode_one(query);
    let hits = index.vector_store()?.query(&query_vec, limit, None)?;
    let mut in_module = 0usize;
    let mut values = Vec::new();
    for (unit_idx, score) in hits {
        let Some(unit) = index.semantic_units.get(unit_idx) else {
            continue;
        };
        let is_in_module = file_paths.contains(&unit.file_path);
        if is_in_module {
            in_module += 1;
        }
        values.push(json!({
            "path": unit.file_path,
            "score": round2_local(score),
            "in_module": is_in_module,
        }));
    }
    let density = if values.is_empty() {
        0.0
    } else {
        round2_local(in_module as f32 / values.len() as f32)
    };
    Ok((values, density))
}

fn module_semantic_density(terms: &[(String, f32, usize)], symbol_count: usize) -> f32 {
    let top_count = terms
        .iter()
        .take(5)
        .map(|(_, _, count)| *count)
        .sum::<usize>();
    if symbol_count == 0 {
        0.0
    } else {
        round2_local((top_count as f32 / symbol_count as f32).min(1.0))
    }
}

fn module_language_counts(index: &Codebase, files: &[String]) -> Vec<Value> {
    let mut counts = BTreeMap::<String, usize>::new();
    for path in files {
        if let Some(file) = index.files.get(path) {
            *counts
                .entry(language_label(file.language.as_str()).to_string())
                .or_default() += 1;
        }
    }
    let mut items = counts.into_iter().collect::<Vec<_>>();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    items
        .into_iter()
        .map(|(language, files)| json!({"language": language, "files": files}))
        .collect()
}

fn language_counts(index: &Codebase) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::<String, usize>::new();
    for file in index.files.values() {
        *counts
            .entry(language_label(file.language.as_str()).to_string())
            .or_default() += 1;
    }
    counts
}

fn language_label(language: &str) -> &str {
    match language {
        "csharp" => "C#",
        "java" => "Java",
        "rust" => "Rust",
        "python" => "Python",
        "javascript" | "jsx" => "JavaScript",
        "typescript" | "tsx" => "TypeScript",
        "c" => "C",
        "cpp" => "C++",
        other => other,
    }
}

fn file_module_token_counts(file: &FileEntry) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for part in file.path.split(['/', '.', '-', ' ', '+', '_']) {
        add_module_token(part, 1, &mut counts);
    }
    if let Some(namespace) = &file.namespace {
        for part in namespace.split('.') {
            add_module_token(part, 2, &mut counts);
        }
    }
    for import in &file.imports {
        for part in import.split(['/', '.', ':', '-', ' ', '+', '_']) {
            add_module_token(part, 1, &mut counts);
        }
    }
    for symbol in &file.symbols {
        let weight = if matches!(
            symbol.kind.as_str(),
            "class" | "interface" | "struct" | "enum" | "record"
        ) {
            4
        } else {
            2
        };
        for part in split_identifier(&symbol.name) {
            add_module_token(&part, weight, &mut counts);
        }
    }
    counts
}

fn top_terms_from_counts(counts: &BTreeMap<String, usize>, limit: usize) -> Vec<(String, usize)> {
    let mut items = counts
        .iter()
        .map(|(term, count)| (term.clone(), *count))
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    items.truncate(limit);
    items
}

fn atlas_dependency_ids(
    paths: Option<&Vec<String>>,
    path_to_point_id: &HashMap<String, usize>,
    limit: usize,
) -> Vec<usize> {
    let Some(paths) = paths else {
        return Vec::new();
    };
    let mut items = paths
        .iter()
        .filter_map(|path| path_to_point_id.get(path).copied())
        .collect::<Vec<_>>();
    items.sort();
    items.dedup();
    items.truncate(limit);
    items
}

fn module_confidence_score(
    file_count: usize,
    cohesion: f32,
    semantic_density: f32,
    entry_points: usize,
    cross_folder: bool,
) -> f32 {
    let size_component = ((file_count as f32).ln_1p() / 5.0).min(0.35);
    let entry_component = (entry_points as f32 * 0.03).min(0.15);
    let cross_component = if cross_folder { 0.05 } else { 0.0 };
    round2_local(
        (cohesion * 0.35
            + semantic_density * 0.25
            + size_component
            + entry_component
            + cross_component)
            .clamp(0.0, 1.0),
    )
}

fn module_evidence(
    file_count: usize,
    symbol_count: usize,
    internal_deps: usize,
    boundary_deps: usize,
    cohesion: f32,
    cross_folder: bool,
    semantic_density: f32,
) -> Vec<String> {
    let mut evidence = vec![
        format!("{file_count} files and {symbol_count} indexed symbols"),
        format!(
            "dependency cohesion {cohesion:.2} from {internal_deps} internal and {boundary_deps} boundary dependency edges"
        ),
        format!("semantic neighbor density {semantic_density:.2}"),
    ];
    if cross_folder {
        evidence.push(
            "files span multiple path roots; treat folder labels as weak evidence".to_string(),
        );
    }
    evidence
}

#[derive(Debug, Clone, Copy)]
struct ModuleLayout {
    x: f32,
    y: f32,
    radius: f32,
}

#[derive(Debug, Clone)]
struct ModuleLayoutItem {
    id: usize,
    radius: f32,
    desired_x: f32,
    desired_y: f32,
    x: f32,
    y: f32,
}

fn module_layouts(index: &Codebase, modules: &[ModuleAtlasModule]) -> HashMap<usize, ModuleLayout> {
    if modules.is_empty() {
        return HashMap::new();
    }

    let mut items = modules
        .iter()
        .map(|module| {
            let counts = module_term_counts(module);
            let anchor = module_anchor(&module.label, &counts, modules.len());
            ModuleLayoutItem {
                id: module.community_id,
                radius: module_layout_radius(module.file_count),
                desired_x: anchor.0,
                desired_y: anchor.1,
                x: anchor.0,
                y: anchor.1,
            }
        })
        .collect::<Vec<_>>();
    let module_edges = module_layout_edges(index, modules);
    relax_module_layout(&mut items, &module_edges);

    let mut layouts = HashMap::new();
    for item in items {
        layouts.insert(
            item.id,
            ModuleLayout {
                x: round2_local(item.x),
                y: round2_local(item.y),
                radius: item.radius,
            },
        );
    }

    layouts
}

fn module_layout_radius(file_count: usize) -> f32 {
    let radius = 2.1 + (file_count as f32).sqrt() * 0.34;
    radius.clamp(2.8, 11.5)
}

fn module_anchor(label: &str, counts: &BTreeMap<String, usize>, module_count: usize) -> (f32, f32) {
    let semantic = project_terms(counts, 1.0);
    let hash = fnv1a(label);
    let angle = random01(hash) * std::f32::consts::TAU;
    let ring = 0.65 + random01(hash ^ 0x4f1b_cdc1) * 0.55;
    let spread = (module_count as f32).sqrt().max(8.0) * 32.0;
    (
        (semantic.0 * 0.62 + angle.cos() * ring * 0.38) * spread,
        (semantic.1 * 0.62 + angle.sin() * ring * 0.38) * spread,
    )
}

fn module_layout_edges(
    index: &Codebase,
    modules: &[ModuleAtlasModule],
) -> Vec<(usize, usize, f32)> {
    let mut path_to_module = HashMap::<&str, usize>::new();
    for module in modules {
        for path in &module.files {
            path_to_module.insert(path.as_str(), module.community_id);
        }
    }

    let mut weights = BTreeMap::<(usize, usize), f32>::new();
    for module in modules {
        for path in &module.files {
            let Some(from) = path_to_module.get(path.as_str()).copied() else {
                continue;
            };
            for dep in index.deps_for(path) {
                let Some(to) = path_to_module.get(dep.as_str()).copied() else {
                    continue;
                };
                if from == to {
                    continue;
                }
                let key = if from < to { (from, to) } else { (to, from) };
                *weights.entry(key).or_default() += 1.0;
            }
        }
    }

    weights
        .into_iter()
        .filter_map(|((a, b), weight)| (weight >= 2.0).then_some((a, b, weight)))
        .collect()
}

fn relax_module_layout(items: &mut [ModuleLayoutItem], edges: &[(usize, usize, f32)]) {
    let mut id_to_index = HashMap::<usize, usize>::new();
    for (idx, item) in items.iter().enumerate() {
        id_to_index.insert(item.id, idx);
    }

    for _ in 0..180 {
        let mut delta = vec![(0.0f32, 0.0f32); items.len()];
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let dx = items[j].x - items[i].x;
                let dy = items[j].y - items[i].y;
                let distance_sq = (dx * dx + dy * dy).max(0.04);
                let distance = distance_sq.sqrt();
                let min_distance = items[i].radius + items[j].radius + 8.0;
                let force = if distance < min_distance {
                    (min_distance - distance) * 0.18
                } else {
                    ((items[i].radius * items[j].radius) / distance_sq).min(0.04)
                };
                let nx = dx / distance;
                let ny = dy / distance;
                delta[i].0 -= nx * force;
                delta[i].1 -= ny * force;
                delta[j].0 += nx * force;
                delta[j].1 += ny * force;
            }
        }

        for (a, b, weight) in edges {
            let (Some(&ai), Some(&bi)) = (id_to_index.get(a), id_to_index.get(b)) else {
                continue;
            };
            let dx = items[bi].x - items[ai].x;
            let dy = items[bi].y - items[ai].y;
            let distance = (dx * dx + dy * dy).sqrt().max(0.001);
            let target = items[ai].radius + items[bi].radius + 30.0 + 50.0 / weight.sqrt();
            let weight_effect = weight.sqrt().min(3.0);
            let force = ((distance - target) * 0.0015 * weight_effect).clamp(-0.05, 0.05);
            let nx = dx / distance;
            let ny = dy / distance;
            delta[ai].0 += nx * force;
            delta[ai].1 += ny * force;
            delta[bi].0 -= nx * force;
            delta[bi].1 -= ny * force;
        }

        for (idx, item) in items.iter().enumerate() {
            delta[idx].0 += (item.desired_x - item.x) * 0.018;
            delta[idx].1 += (item.desired_y - item.y) * 0.018;
        }

        for (item, (dx, dy)) in items.iter_mut().zip(delta) {
            item.x += dx.clamp(-2.4, 2.4);
            item.y += dy.clamp(-2.4, 2.4);
        }
    }

    resolve_module_collisions(items);
    center_layout(items);
}

fn resolve_module_collisions(items: &mut [ModuleLayoutItem]) {
    for _ in 0..80 {
        let mut moved = false;
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let dx = items[j].x - items[i].x;
                let dy = items[j].y - items[i].y;
                let distance = (dx * dx + dy * dy).sqrt().max(0.001);
                let min_distance = items[i].radius + items[j].radius + 7.0;
                if distance >= min_distance {
                    continue;
                }
                let push = (min_distance - distance) * 0.52;
                let nx = dx / distance;
                let ny = dy / distance;
                items[i].x -= nx * push;
                items[i].y -= ny * push;
                items[j].x += nx * push;
                items[j].y += ny * push;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
}

fn center_layout(items: &mut [ModuleLayoutItem]) {
    if items.is_empty() {
        return;
    }
    let cx = items.iter().map(|item| item.x).sum::<f32>() / items.len() as f32;
    let cy = items.iter().map(|item| item.y).sum::<f32>() / items.len() as f32;
    for item in items {
        item.x -= cx;
        item.y -= cy;
    }
}

fn module_file_offsets(
    index: &Codebase,
    module: &ModuleAtlasModule,
    radius: f32,
) -> HashMap<String, (f32, f32)> {
    let internal_degrees = module_internal_degrees(index, module);
    let max_degree = internal_degrees.values().copied().max().unwrap_or(1).max(1) as f32;
    let mut items = module
        .files
        .iter()
        .filter_map(|path| {
            let file = index.files.get(path)?;
            let terms = file_module_token_counts(file);
            let degree = *internal_degrees.get(path).unwrap_or(&0) as f32;
            let target = file_layout_target(path, &terms, degree, max_degree, radius);
            Some(FileLayoutItem {
                path: path.clone(),
                radius: file_node_radius(file),
                x: target.0,
                y: target.1,
                target_x: target.0,
                target_y: target.1,
            })
        })
        .collect::<Vec<_>>();

    if items.is_empty() {
        return HashMap::new();
    }
    if items.len() == 1 {
        return HashMap::from([(items[0].path.clone(), (0.0, 0.0))]);
    }

    let edges = file_layout_edges(index, module);
    relax_file_layout(&mut items, &edges, radius);
    items
        .into_iter()
        .map(|item| (item.path, (round2_local(item.x), round2_local(item.y))))
        .collect()
}

#[derive(Debug, Clone)]
struct FileLayoutItem {
    path: String,
    radius: f32,
    x: f32,
    y: f32,
    target_x: f32,
    target_y: f32,
}

fn file_node_radius(file: &FileEntry) -> f32 {
    (0.09 + (file.symbols.len() as f32).sqrt() * 0.012).clamp(0.08, 0.22)
}

fn module_internal_degrees(index: &Codebase, module: &ModuleAtlasModule) -> HashMap<String, usize> {
    let file_set = module
        .files
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut degrees = HashMap::<String, usize>::new();
    for path in &module.files {
        let outgoing = index
            .deps_for(path)
            .into_iter()
            .filter(|dep| file_set.contains(dep.as_str()))
            .count();
        let incoming = index
            .reverse_deps_for(path)
            .into_iter()
            .filter(|source| file_set.contains(source.as_str()))
            .count();
        degrees.insert(path.clone(), incoming + outgoing);
    }
    degrees
}

fn file_layout_target(
    path: &str,
    terms: &BTreeMap<String, usize>,
    degree: f32,
    max_degree: f32,
    radius: f32,
) -> (f32, f32) {
    let semantic = project_terms(terms, radius * 0.46);
    let folder = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or(path);
    let folder_hash = fnv1a(folder);
    let folder_angle = random01(folder_hash) * std::f32::consts::TAU;
    let folder_radius = random01(folder_hash ^ 0x72ab_91d3).sqrt() * radius * 0.34;
    let path_hash = fnv1a(path);
    let jitter_angle = random01(path_hash ^ 0x7f4a_7c15) * std::f32::consts::TAU;
    let jitter_radius = random01(path_hash ^ 0xb529_7a4d).sqrt() * radius * 0.22;
    let centrality = (degree / max_degree).sqrt().clamp(0.0, 1.0);
    let inward = 1.0 - centrality * 0.55;
    clamp_vector(
        (
            (semantic.0 + folder_angle.cos() * folder_radius) * inward
                + jitter_angle.cos() * jitter_radius,
            (semantic.1 + folder_angle.sin() * folder_radius) * inward
                + jitter_angle.sin() * jitter_radius,
        ),
        radius * 0.78,
    )
}

fn file_layout_edges(index: &Codebase, module: &ModuleAtlasModule) -> Vec<(usize, usize, f32)> {
    let mut path_to_index = HashMap::<&str, usize>::new();
    for (idx, path) in module.files.iter().enumerate() {
        path_to_index.insert(path.as_str(), idx);
    }
    let mut weights = BTreeMap::<(usize, usize), f32>::new();
    for path in &module.files {
        let Some(from) = path_to_index.get(path.as_str()).copied() else {
            continue;
        };
        for dep in index.deps_for(path) {
            let Some(to) = path_to_index.get(dep.as_str()).copied() else {
                continue;
            };
            if from == to {
                continue;
            }
            let key = if from < to { (from, to) } else { (to, from) };
            *weights.entry(key).or_default() += 1.0;
        }
    }
    weights.into_iter().map(|((a, b), w)| (a, b, w)).collect()
}

fn relax_file_layout(items: &mut [FileLayoutItem], edges: &[(usize, usize, f32)], radius: f32) {
    for _ in 0..70 {
        let mut delta = vec![(0.0f32, 0.0f32); items.len()];
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                let dx = items[j].x - items[i].x;
                let dy = items[j].y - items[i].y;
                let distance_sq = (dx * dx + dy * dy).max(0.0004);
                let distance = distance_sq.sqrt();
                let min_distance = items[i].radius + items[j].radius + radius * 0.018;
                if distance >= min_distance {
                    continue;
                }
                let force = (min_distance - distance) * 0.24;
                let nx = dx / distance;
                let ny = dy / distance;
                delta[i].0 -= nx * force;
                delta[i].1 -= ny * force;
                delta[j].0 += nx * force;
                delta[j].1 += ny * force;
            }
        }

        for (a, b, weight) in edges {
            let dx = items[*b].x - items[*a].x;
            let dy = items[*b].y - items[*a].y;
            let distance = (dx * dx + dy * dy).sqrt().max(0.001);
            let target = radius * (0.18 + 0.08 / weight.sqrt());
            let force = ((distance - target) * 0.006 * weight.sqrt()).clamp(-0.035, 0.035);
            let nx = dx / distance;
            let ny = dy / distance;
            delta[*a].0 += nx * force;
            delta[*a].1 += ny * force;
            delta[*b].0 -= nx * force;
            delta[*b].1 -= ny * force;
        }

        for (idx, item) in items.iter().enumerate() {
            delta[idx].0 += (item.target_x - item.x) * 0.05;
            delta[idx].1 += (item.target_y - item.y) * 0.05;
            delta[idx].0 -= item.x * 0.002;
            delta[idx].1 -= item.y * 0.002;
            let distance = (item.x * item.x + item.y * item.y).sqrt();
            if distance > radius * 0.82 {
                let pull = (distance - radius * 0.82) * 0.18;
                delta[idx].0 -= item.x / distance * pull;
                delta[idx].1 -= item.y / distance * pull;
            }
        }

        for (item, (dx, dy)) in items.iter_mut().zip(delta) {
            item.x += dx.clamp(-radius * 0.035, radius * 0.035);
            item.y += dy.clamp(-radius * 0.035, radius * 0.035);
            let clamped = clamp_vector((item.x, item.y), radius * 0.84);
            item.x = clamped.0;
            item.y = clamped.1;
        }
    }
}

fn module_term_counts(module: &ModuleAtlasModule) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for term in &module.terms {
        let Some(name) = term.get("term").and_then(Value::as_str) else {
            continue;
        };
        let count = term
            .get("count")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                (term
                    .get("score")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0)
                    .max(0.1)
                    * 10.0)
                    .round() as u64
            })
            .max(1) as usize;
        counts.insert(name.to_string(), count);
    }
    counts
}

fn clamp_vector(point: (f32, f32), max_radius: f32) -> (f32, f32) {
    let distance = (point.0 * point.0 + point.1 * point.1).sqrt();
    if distance <= max_radius || distance <= f32::EPSILON {
        point
    } else {
        let scale = max_radius / distance;
        (point.0 * scale, point.1 * scale)
    }
}

fn project_terms(counts: &BTreeMap<String, usize>, scale: f32) -> (f32, f32) {
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut weight_sum = 0.0f32;
    for (term, weight) in counts {
        let hash = fnv1a(term);
        let angle = (hash as f32 / u32::MAX as f32) * std::f32::consts::TAU;
        let radius = 0.7 + random01(hash ^ 0x9e3779b9) * 0.6;
        let weight = *weight as f32;
        x += angle.cos() * radius * weight;
        y += angle.sin() * radius * weight;
        weight_sum += weight;
    }
    if weight_sum <= f32::EPSILON {
        (0.0, 0.0)
    } else {
        ((x / weight_sum) * scale, (y / weight_sum) * scale)
    }
}

fn fnv1a(value: &str) -> u32 {
    let mut hash = 2166136261u32;
    for byte in value.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

fn random01(seed: u32) -> f32 {
    let mut value = seed;
    value ^= value << 13;
    value ^= value >> 17;
    value ^= value << 5;
    (value % 100000) as f32 / 100000.0
}

fn chrono_like_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn path_matches_prefix(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn round2_local(value: f32) -> f32 {
    (value * 100.0).round() / 100.0
}

const MODULE_TERM_STOPWORDS: &[&str] = &[
    "asset",
    "assets",
    "base",
    "build",
    "cache",
    "client",
    "code",
    "common",
    "component",
    "components",
    "config",
    "core",
    "data",
    "default",
    "editor",
    "factory",
    "file",
    "framework",
    "game",
    "generated",
    "global",
    "helper",
    "helpers",
    "hot",
    "hotfix",
    "impl",
    "index",
    "item",
    "items",
    "lib",
    "library",
    "list",
    "logic",
    "main",
    "manager",
    "model",
    "module",
    "modules",
    "object",
    "package",
    "packages",
    "plugin",
    "runtime",
    "script",
    "scripts",
    "service",
    "simple",
    "system",
    "test",
    "tests",
    "type",
    "types",
    "util",
    "utils",
    "view",
    "views",
];

const LOUVAIN_CACHE_VERSION: u32 = 2;
const LOUVAIN_SUBCOMMUNITIES_CACHE_VERSION: u32 = 12;
const LOUVAIN_CACHE_FILE: &str = "louvain-communities.bin";
const LOUVAIN_SUBCOMMUNITIES_CACHE_FILE: &str = "louvain-subcommunities.bin";

#[derive(Debug, Serialize, Deserialize)]
struct LouvainCachePayload {
    version: u32,
    graph_hash: String,
    communities: Vec<GraphCommunity>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LouvainSubcommunitiesCachePayload {
    version: u32,
    graph_hash: String,
    entries: HashMap<usize, Vec<GraphCommunity>>,
}

fn ensure_louvain_communities(index: &Codebase) {
    if index.louvain_communities.read().is_some() {
        return;
    }

    let graph_hash = louvain_graph_hash(index);
    if let Some(communities) = load_louvain_cache(index, &graph_hash) {
        *index.louvain_communities.write() = Some(communities);
        return;
    }

    let graph = index.graph();
    let communities = graph.louvain_communities();
    save_louvain_cache(index, &graph_hash, &communities);
    *index.louvain_communities.write() = Some(communities);
}

fn louvain_graph_hash(index: &Codebase) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"codebase-mcp-louvain-v2");
    let graph = index.graph_summary();
    hasher.update(graph.nodes.to_string().as_bytes());
    hasher.update(graph.edges.to_string().as_bytes());
    for file in index.files.values() {
        hasher.update(file.path.as_bytes());
        hasher.update(file.content_hash.as_bytes());
    }
    hasher.finalize().to_hex()[..16].to_string()
}

fn louvain_cache_path(index: &Codebase) -> Option<PathBuf> {
    index
        .storage_dir
        .as_ref()
        .map(|dir| PathBuf::from(dir).join(LOUVAIN_CACHE_FILE))
}

fn load_louvain_cache(index: &Codebase, graph_hash: &str) -> Option<Vec<GraphCommunity>> {
    let path = louvain_cache_path(index)?;
    let bytes = fs::read(&path).ok()?;
    let payload: LouvainCachePayload = bincode::deserialize(&bytes).ok()?;
    if payload.version == LOUVAIN_CACHE_VERSION && payload.graph_hash == graph_hash {
        Some(payload.communities)
    } else {
        None
    }
}

fn save_louvain_cache(index: &Codebase, graph_hash: &str, communities: &[GraphCommunity]) {
    let Some(path) = louvain_cache_path(index) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let payload = LouvainCachePayload {
        version: LOUVAIN_CACHE_VERSION,
        graph_hash: graph_hash.to_string(),
        communities: communities.to_vec(),
    };
    let Ok(bytes) = bincode::serialize(&payload) else {
        return;
    };
    let tmp = path.with_extension("bin.tmp");
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::remove_file(&path);
        let _ = fs::rename(tmp, path);
    }
}

fn ensure_louvain_subcommunities(index: &Codebase, parent: &GraphCommunity) -> Vec<GraphCommunity> {
    if let Some(communities) = index.louvain_subcommunities.read().get(&parent.id).cloned() {
        return communities;
    }

    let graph_hash = louvain_graph_hash(index);
    if let Some(entries) = load_louvain_subcommunities_cache(index, &graph_hash) {
        let cached = entries.get(&parent.id).cloned();
        {
            let mut guard = index.louvain_subcommunities.write();
            for (id, communities) in entries {
                guard.entry(id).or_insert(communities);
            }
        }
        if let Some(communities) = cached {
            return communities;
        }
    }

    let graph = index.graph();
    let communities =
        graph.louvain_subcommunities(&parent.nodes, &index.files, Some(&parent.label));
    {
        let mut guard = index.louvain_subcommunities.write();
        guard.insert(parent.id, communities.clone());
        save_louvain_subcommunities_cache(index, &graph_hash, &guard);
    }
    communities
}

fn louvain_subcommunities_cache_path(index: &Codebase) -> Option<PathBuf> {
    index
        .storage_dir
        .as_ref()
        .map(|dir| PathBuf::from(dir).join(LOUVAIN_SUBCOMMUNITIES_CACHE_FILE))
}

fn load_louvain_subcommunities_cache(
    index: &Codebase,
    graph_hash: &str,
) -> Option<HashMap<usize, Vec<GraphCommunity>>> {
    let path = louvain_subcommunities_cache_path(index)?;
    let bytes = fs::read(&path).ok()?;
    let payload: LouvainSubcommunitiesCachePayload = bincode::deserialize(&bytes).ok()?;
    if payload.version == LOUVAIN_SUBCOMMUNITIES_CACHE_VERSION && payload.graph_hash == graph_hash {
        Some(payload.entries)
    } else {
        None
    }
}

fn save_louvain_subcommunities_cache(
    index: &Codebase,
    graph_hash: &str,
    entries: &HashMap<usize, Vec<GraphCommunity>>,
) {
    let Some(path) = louvain_subcommunities_cache_path(index) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let payload = LouvainSubcommunitiesCachePayload {
        version: LOUVAIN_SUBCOMMUNITIES_CACHE_VERSION,
        graph_hash: graph_hash.to_string(),
        entries: entries.clone(),
    };
    let Ok(bytes) = bincode::serialize(&payload) else {
        return;
    };
    let tmp = path.with_extension("bin.tmp");
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::remove_file(&path);
        let _ = fs::rename(tmp, path);
    }
}

fn handle_analyze(index: &Codebase, args: &Value) -> Result<String> {
    let top_n = get_usize(args, "top_n").unwrap_or(10).clamp(1, 100);
    let graph = index.graph();
    serde_json::to_string_pretty(&graph.analysis(top_n)).map_err(Into::into)
}

fn handle_export(index: &Codebase, args: &Value) -> Result<String> {
    let format = get_str(args, "format").unwrap_or_else(|| "json".to_string());
    let returning = get_str(args, "output_path").is_none();
    let max_nodes =
        get_usize(args, "max_nodes").unwrap_or(if returning { 1000 } else { usize::MAX });
    let max_edges =
        get_usize(args, "max_edges").unwrap_or(if returning { 3000 } else { usize::MAX });
    let graph = index.graph();
    let content = match format.as_str() {
        "json" => serde_json::to_string_pretty(&graph.limited_json(max_nodes, max_edges))?,
        "graphml" => graph.to_graphml(max_nodes, max_edges),
        "cypher" | "neo4j" => graph.to_cypher(max_nodes, max_edges),
        other => {
            return Ok(format!(
                "error: unsupported export format: {other}; use json, graphml, or cypher"
            ));
        }
    };

    if let Some(output_path) = get_str(args, "output_path") {
        let output_path = resolve_output_path(&index.root, &output_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&output_path, content)?;
        Ok(format!(
            "exported {} graph to {}",
            format,
            output_path.display()
        ))
    } else {
        Ok(content)
    }
}

fn handle_index(manager: &ProjectManager, args: &Value) -> Result<String> {
    let path = required_str(args, "path")?;
    let index = manager.reindex(Path::new(&path))?;
    Ok(format!(
        "indexed {}: {} files, {} chunks, {} symbols",
        index.root.display(),
        index.files.len(),
        index.chunks.len(),
        index
            .files
            .values()
            .map(|file| file.symbols.len())
            .sum::<usize>()
    ))
}

fn handle_projects(manager: &ProjectManager) -> String {
    let projects = manager.projects();
    if projects.is_empty() {
        "no projects indexed".to_string()
    } else {
        projects
            .into_iter()
            .enumerate()
            .map(|(idx, project)| format!("{}. {}", idx + 1, project))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }
}

fn handle_bundle(manager: &ProjectManager, args: &Value) -> Result<String> {
    let Some(ops) = args.get("ops").and_then(Value::as_array) else {
        return Ok("error: missing 'ops'".to_string());
    };
    let timing = get_bool(args, "timing");
    let discard_output = get_bool(args, "discard_output");
    let mut out = String::new();
    for (idx, op) in ops.iter().take(MAX_BATCH_ITEMS).enumerate() {
        let tool = get_str(op, "tool").unwrap_or_default();
        if tool.is_empty() {
            out.push_str(&format!(
                "--- [{idx}] <missing> ---\nerror: missing 'tool' field\n"
            ));
            continue;
        }
        if tool == "codedb_bundle" {
            out.push_str(&format!(
                "--- [{idx}] {tool} ---\nerror: codedb_bundle not allowed in bundle\n"
            ));
            continue;
        }
        let arguments = op.get("arguments").unwrap_or(op);
        let start = Instant::now();
        let result = dispatch_tool(manager, &tool, arguments);
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        out.push_str(&format!("--- [{}] {} ---\n", idx, tool));
        if timing {
            out.push_str(&format!("time_ms: {:.3}\n", elapsed_ms));
        }
        if discard_output {
            let first_line = result.lines().next().unwrap_or_default();
            out.push_str(&format!("summary: {first_line}\n"));
        } else {
            out.push_str(&result);
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    if ops.len() > MAX_BATCH_ITEMS {
        out.push_str(&format!(
            "(truncated: {} more bundle ops not executed)\n",
            ops.len() - MAX_BATCH_ITEMS
        ));
    }
    Ok(out)
}

fn format_line_hits(query: &str, hits: Vec<crate::types::SearchHit>) -> String {
    let mut out = format!("{} results for '{}':\n", hits.len(), query);
    let mut per_file: HashMap<String, usize> = HashMap::new();
    let mut shown = 0usize;
    for hit in hits {
        let count = per_file.entry(hit.path.clone()).or_default();
        *count += 1;
        if *count > 5 {
            if *count == 6 {
                out.push_str(&format!("  {}: ... (more matches truncated)\n", hit.path));
            }
            continue;
        }
        if let Some(scope) = hit.scope {
            out.push_str(&format!(
                "  {}:{}: {}  [in {} ({}, L{}-L{})]\n",
                hit.path, hit.line, hit.text, scope.name, scope.kind, scope.start, scope.end
            ));
        } else {
            out.push_str(&format!("  {}:{}: {}\n", hit.path, hit.line, hit.text));
        }
        shown += 1;
    }
    let total: usize = per_file.values().sum();
    if shown < total {
        out.push_str(&format!(
            "({shown} shown, {} truncated by per-file cap)\n",
            total - shown
        ));
    }
    out
}

fn format_chunk_hits(
    index: &Codebase,
    query: &str,
    hits: Vec<crate::types::ChunkSearchHit>,
) -> Result<String> {
    let mut out = format!("{} results for '{}':\n", hits.len(), query);
    let mut content_by_file = HashMap::new();
    for hit in hits {
        let content = index.chunk_content_cached(&hit.chunk, &mut content_by_file)?;
        let preview = content
            .lines()
            .filter(|line| !is_comment_or_blank(line))
            .take(5)
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n    ");
        out.push_str(&format!(
            "  {}:{}-{}  [score={:.3}, {}]\n    {}\n",
            index.chunk_file_path(&hit.chunk),
            hit.chunk.start_line,
            hit.chunk.end_line,
            hit.score,
            hit.source,
            preview
        ));
    }
    Ok(out)
}

fn extract_lines(content: &str, start: usize, end: usize, compact: bool) -> String {
    let mut out = String::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if line_no < start || line_no > end {
            continue;
        }
        if compact && is_comment_or_blank(line) {
            continue;
        }
        out.push_str(&format!("{line_no}: {line}\n"));
    }
    out
}

fn transitive_deps(
    index: &Codebase,
    path: &str,
    forward: bool,
    max_depth: Option<usize>,
) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([(path.to_string(), 0usize)]);
    while let Some((current, depth)) = queue.pop_front() {
        if max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        let deps = if forward {
            index.deps_for(&current)
        } else {
            index.reverse_deps_for(&current)
        };
        for dep in deps {
            if seen.insert(dep.clone()) {
                queue.push_back((dep, depth + 1));
            }
        }
    }
    seen.into_iter().collect()
}

fn fuzzy_suggestions(index: &Codebase, query: &str) -> String {
    let mut matches = index
        .files
        .keys()
        .filter_map(|path| fuzzy_score(path, query).map(|score| (path.clone(), score)))
        .collect::<Vec<_>>();
    matches.sort_by(|a, b| b.1.total_cmp(&a.1));
    if matches.is_empty() {
        return String::new();
    }
    let mut out = String::from("did you mean:\n");
    for (path, score) in matches.into_iter().take(5) {
        out.push_str(&format!("  {path} (score: {score:.2})\n"));
    }
    out
}

fn fuzzy_score(path: &str, query: &str) -> Option<f32> {
    let path_lower = path.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if query_lower.is_empty() {
        return None;
    }
    if path_lower.contains(&query_lower) {
        return Some(100.0 + query_lower.len() as f32 / path_lower.len().max(1) as f32);
    }
    let compact_path = compact_fuzzy_text(&path_lower);
    let compact_query = compact_fuzzy_text(&query_lower);
    if !compact_query.is_empty() && compact_path.contains(&compact_query) {
        return Some(80.0 + compact_query.len() as f32 / compact_path.len().max(1) as f32);
    }
    fuzzy_subsequence_score(&path_lower, &query_lower).or_else(|| {
        (!compact_query.is_empty())
            .then(|| {
                fuzzy_subsequence_score(&compact_path, &compact_query).map(|score| score * 0.8)
            })
            .flatten()
    })
}

fn fuzzy_subsequence_score(path_lower: &str, query_lower: &str) -> Option<f32> {
    let mut score = 0.0f32;
    let mut pos = 0usize;
    let mut streak = 0.0f32;
    for ch in query_lower.chars() {
        let rest = &path_lower[pos..];
        let found = rest.find(ch)?;
        pos += found + ch.len_utf8();
        streak = if found == 0 { streak + 1.0 } else { 1.0 };
        score += 1.0 + streak * 0.5 - (found as f32 * 0.01);
    }
    Some(score)
}

fn compact_fuzzy_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

fn chunk_selector_for_paths(index: &Codebase, paths: &BTreeSet<String>) -> Vec<usize> {
    let mut selector = Vec::new();
    for path in paths {
        if let Some(indices) = index.chunk_indices_by_file.get(path) {
            selector.extend(indices.iter().copied());
        }
    }
    selector.sort_unstable();
    selector.dedup();
    selector
}

fn normalize_dir_prefix(path: &str) -> String {
    let normalized = normalize_rel_path(path);
    if normalized.is_empty() {
        String::new()
    } else if normalized.ends_with('/') {
        normalized
    } else {
        format!("{normalized}/")
    }
}

fn resolve_output_path(root: &Path, output_path: &str) -> PathBuf {
    let path = PathBuf::from(output_path);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn required_str(args: &Value, key: &str) -> Result<String> {
    get_str(args, key).ok_or_else(|| anyhow!("missing '{key}' argument"))
}

fn get_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)?.as_str().map(str::to_string)
}

fn get_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn get_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key)?.as_u64().map(|n| n as usize)
}

fn get_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key)?.as_u64()
}
