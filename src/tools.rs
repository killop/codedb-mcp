use crate::cache::{
    CachedCallerEntry, CachedCallerHit, CachedDepsSnapshot, CachedFileEntry, ProjectCache,
};
use crate::event_log::{self, ToolLogContext};
use crate::graph_query::{
    self, Direction as GraphDirection, NodePattern as GraphNodePattern, QueryEdge as PropertyEdge,
    QueryNode as PropertyNode, QueryProvider, RelationshipPattern as GraphRelationshipPattern,
    Scalar as GraphScalar,
};
use crate::indexer::{
    ChangedFile, Codebase, IndexOptions, build_globset, hash_content, normalize_rel_path,
    strip_strings_and_line_comment,
};
use crate::language::{is_comment_or_blank, mask_comments, scope_for_line};
use crate::search::{hybrid_ranked_chunks, is_symbol_query, lexical_ranked_chunks};
use crate::tokens::{
    has_whole_word, is_identifier_char, raw_identifiers, split_identifier, tokenize,
};
use crate::types::{FileEntry, SearchHit, Symbol, SymbolKind};
use anyhow::{Result, anyhow};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde::ser::{SerializeSeq, Serializer as _};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_BATCH_ITEMS: usize = 20;
const READ_COMPACT_MAX_LINES: usize = 80;
const READ_FULL_RANGE_MAX_LINES: usize = 80;
const READ_SYMBOL_LEAD_MAX_RANGE_LINES: usize = 90;
const READ_SYMBOL_LEAD_MAX_SYMBOLS: usize = 4;
const READ_SYMBOL_LEAD_EMIT_SYMBOLS: usize = 1;
const READ_WIDE_SYMBOL_LEAD_EMIT_SYMBOLS: usize = 1;
const READ_SYMBOL_LEAD_HANDOFF_LIMIT: usize = 2;
const OUTLINE_BODY_FOLLOWUP_LIMIT: usize = 4;
const OUTLINE_BODY_FOLLOWUP_SCAN_SYMBOL_LIMIT: usize = 180;
const OUTLINE_LITERAL_BRIDGE_MAX_FILE_LINES: usize = 120;
const OUTLINE_LITERAL_BRIDGE_LEAD_LIMIT: usize = 2;
const SEARCH_JSON_MAX_RESULTS: usize = 40;
const WORD_SCOPE_HIT_LIMIT: usize = 5_000;
const WORD_DEFAULT_MAX_RESULTS: usize = 50;
const FIND_SYMBOL_SUMMARY_RESULT_LIMIT: usize = 12;
const FIND_SYMBOL_SUMMARY_PER_FILE: usize = 4;
const GLOB_CENTER_SUMMARY_RESULT_LIMIT: usize = 6;
const GLOB_SYMBOL_SUMMARY_PER_FILE: usize = 4;
const GLOB_ACTIONABLE_MAX_MATCHES: usize = 24;
const GLOB_ACTIONABLE_FILE_SCAN_LIMIT: usize = 32;
const GLOB_ACTIONABLE_SYMBOL_SCAN_LIMIT: usize = 32;
const GLOB_ACTIONABLE_LEAD_LIMIT: usize = 4;
const GLOB_ACTIONABLE_LEADS_PER_SYMBOL: usize = 2;
const GLOB_ACTIONABLE_MAX_SYMBOL_LINES: usize = 240;
const CALLERS_AMBIGUOUS_AUTO_LIMIT: usize = 4;
const SYMBOL_EXPLORATORY_PER_FILE_LIMIT: usize = 3;
const SYMBOL_BODY_LARGE_CONTAINER_MAX_LINES: usize = 140;
const SYMBOL_BODY_NESTED_SYMBOL_LIMIT: usize = 18;
const SYMBOL_BODY_NESTED_FOLLOWUP_LIMIT: usize = 4;
const SYMBOL_BODY_LEAD_LIMIT: usize = 3;
const SYMBOL_BODY_ORDERED_LEAD_LIMIT: usize = 4;
const SYMBOL_BODY_ORDERED_TAIL_PREVIEW_LIMIT: usize = 1;
const SYMBOL_BODY_ORDERED_TAIL_PREVIEW_MAX_LINES: usize = 14;
const SYMBOL_BODY_ORDERED_TAIL_PREVIEW_MAX_CHARS: usize = 420;
const SYMBOL_BODY_ORDERED_TAIL_NESTED_PREVIEW_LIMIT: usize = 1;
const SYMBOL_BODY_ORDERED_TAIL_NESTED_PREVIEW_MAX_LINES: usize = 14;
const SYMBOL_BODY_ORDERED_TAIL_NESTED_PREVIEW_MAX_CHARS: usize = 360;
const SYMBOL_BODY_ORDERED_TAIL_GRANDCHILD_PREVIEW_LIMIT: usize = 0;
const SYMBOL_BODY_ORDERED_TAIL_GRANDCHILD_PREVIEW_MAX_LINES: usize = 12;
const SYMBOL_BODY_ORDERED_TAIL_GRANDCHILD_PREVIEW_MAX_CHARS: usize = 320;
const SYMBOL_BODY_SHORT_WRAPPER_MAX_LINES: usize = 8;
const SYMBOL_BODY_FLOW_HANDOFF_LEAD_LIMIT: usize = 4;
const SYMBOL_BODY_QUALIFIED_TAIL_CALL_LEAD_LIMIT: usize = 8;
const SYMBOL_BODY_FLOW_HANDOFF_PREVIEW_LIMIT: usize = 2;
const SYMBOL_BODY_FLOW_HANDOFF_PREVIEW_MAX_LINES: usize = 18;
const SYMBOL_BODY_FLOW_HANDOFF_PREVIEW_MAX_CHARS: usize = 360;
#[allow(dead_code)]
const SYMBOL_BODY_DISPATCH_PREVIEW_LIMIT: usize = 6;
#[allow(dead_code)]
const SYMBOL_BODY_DISPATCH_PREVIEW_MAX_LINES: usize = 30;
#[allow(dead_code)]
const SYMBOL_BODY_DISPATCH_PREVIEW_MAX_CHARS: usize = 700;
const SYMBOL_BODY_DATA_TYPE_LEAD_LIMIT: usize = 5;
const SYMBOL_BODY_DATA_TYPE_MIN_SCORE: usize = 40;
const SYMBOL_BODY_DATA_TYPE_REF_HITS_PER_LEAD: usize = 1;
const SYMBOL_BODY_DATA_TYPE_REF_TOTAL_HITS: usize = 2;
const SYMBOL_BODY_DATA_TYPE_REF_MAX_WORD_HITS: usize = 320;
const SYMBOL_BODY_DATA_TYPE_REF_SCOPE_LEAD_LIMIT: usize = 2;
const SYMBOL_BODY_DATA_TYPE_REF_SCOPE_DATA_LEADS: usize = 4;
const SYMBOL_BODY_DATA_TYPE_REF_SCOPE_MAX_LINES: usize = 260;
const SYMBOL_BODY_LEAD_PREVIEW_LIMIT: usize = 1;
const SYMBOL_BODY_LEAD_PREVIEW_MAX_LINES: usize = 14;
const SYMBOL_BODY_LEAD_PREVIEW_MAX_CHARS: usize = 360;
const SYMBOL_BODY_LEAD_NESTED_PREVIEW_LIMIT: usize = 0;
const SYMBOL_BODY_LEAD_NESTED_PREVIEW_DEPTH: usize = 1;
const SYMBOL_BODY_LEAD_NESTED_PREVIEW_MAX_LINES: usize = 14;
const SYMBOL_BODY_LEAD_NESTED_PREVIEW_MAX_CHARS: usize = 500;
const SYMBOL_BODY_EXACT_REF_TERM_LIMIT: usize = 3;
const SYMBOL_BODY_EXACT_REF_HITS_PER_TERM: usize = 2;
const SYMBOL_BODY_EXACT_REF_TOTAL_HITS: usize = 0;
const SYMBOL_BODY_EXACT_REF_SCOPE_LEAD_LIMIT: usize = 1;
const SYMBOL_BODY_EXACT_REF_SCOPE_LEADS_PER_HIT: usize = 2;
const SYMBOL_BODY_EXACT_REF_SCOPE_MAX_LINES: usize = 240;
const SYMBOL_BODY_ASSIGNMENT_TARGET_LEAD_LIMIT: usize = 3;
const SYMBOL_BODY_CALLEE_EXACT_REF_SYMBOL_LIMIT: usize = 2;
const SYMBOL_BODY_CALLEE_EXACT_REF_TERM_LIMIT: usize = 1;
const SYMBOL_BODY_CALLEE_EXACT_REF_HITS_PER_TERM: usize = 2;
const SYMBOL_BODY_CALLEE_EXACT_REF_TOTAL_HITS: usize = 0;
const SYMBOL_BODY_CALLEE_EXACT_REF_MAX_LINES: usize = 240;
const SYMBOL_BODY_INCOMING_REF_LIMIT: usize = 0;
const SYMBOL_BODY_INCOMING_REF_MAX_WORD_HITS: usize = 240;
const SYMBOL_BODY_INCOMING_SCOPE_TERM_LIMIT: usize = 3;
const SYMBOL_BODY_INCOMING_SCOPE_MAX_LINES: usize = 240;
const SYMBOL_BODY_LITERAL_BRIDGE_LITERAL_LIMIT: usize = 4;
const SYMBOL_BODY_LITERAL_BRIDGE_LEAD_LIMIT: usize = 3;
const SYMBOL_BODY_LITERAL_BRIDGE_SYMBOL_SCAN_LIMIT: usize = 24;
const SEARCH_DIVERSE_FETCH_MULTIPLIER: usize = 8;
const SEARCH_DIVERSE_GROUP_LIMIT: usize = 4;
const MODULE_HUB_INCOMING_LIMIT: usize = 220;
const MODULE_MAX_DEPENDENCY_EDGES_PER_FILE: usize = 72;
const MODULE_MAX_FILES_PER_GROUP: usize = 450;
const MODULE_LABEL_ITERATIONS: usize = 8;
const CONTEXT_DEFAULT_MAX_FILES: usize = 5;
const CONTEXT_GRAPH_TRAIL_LIMIT: usize = 8;
const CONTEXT_GRAPH_TRAIL_SEEDS: usize = 8;
const CONTEXT_GRAPH_TRAIL_FANOUT: usize = 8;
const CONTEXT_GRAPH_TRAIL_PER_VIA_LIMIT: usize = 3;
const CONTEXT_MODULE_INVENTORY_BROAD_LIMIT: usize = 6;
const CONTEXT_MODULE_INVENTORY_FOCUSED_LIMIT: usize = 12;
const CONTEXT_MODULE_INVENTORY_LEAF_GROUP_LIMIT: usize = 16;
const CONTEXT_MODULE_INVENTORY_LEAF_GROUP_CHILD_LIMIT: usize = 32;
const CONTEXT_MODULE_INVENTORY_LEAF_GROUP_TOTAL_CHILD_LIMIT: usize = 160;
const CONTEXT_MODULE_INVENTORY_MIN_FILES: usize = 3;
const CONTEXT_MODULE_INVENTORY_MAX_FILES: usize = 220;
const CONTEXT_MODULE_INVENTORY_MAX_DEPTH: usize = 7;
const CONTEXT_MODULE_INVENTORY_LINK_LIMIT: usize = 18;
const CONTEXT_MODULE_INVENTORY_LINK_PER_PREFIX: usize = 2;
const CONTEXT_MODULE_INVENTORY_LINK_COUNT_PER_PREFIX: usize = 1;
const CONTEXT_MODULE_INVENTORY_BROAD_MAX_DEPTH: usize = 4;
const CONTEXT_MODULE_INVENTORY_FOCUSED_MIN_DEPTH: usize = 3;
const CONTEXT_MODULE_INVENTORY_FOCUSED_PER_PARENT: usize = 8;
const CONTEXT_SYMBOL_HANDOFF_GLOBAL_LIMIT: usize = 8;
const CONTEXT_SYMBOL_HANDOFF_PER_FILE_LIMIT: usize = 3;
const CONTEXT_SYMBOL_HANDOFF_PER_SYMBOL_LIMIT: usize = 2;
const CONTEXT_SYMBOL_HANDOFF_MAX_SOURCE_LINES: usize = 360;
const CONTEXT_FLOW_CANDIDATE_LIMIT: usize = 8;
const CONTEXT_FLOW_SYMBOLS_PER_FILE: usize = 2;
const CONTEXT_FLOW_STRUCTURAL_FOLLOWUP_FILE_LIMIT: usize = 3;
const CONTEXT_FLOW_STRUCTURAL_FOLLOWUP_PER_FILE_LIMIT: usize = 2;
const CONTEXT_FLOW_SYMBOL_EDGE_LIMIT: usize = 8;
const CONTEXT_FLOW_TRACE_LIMIT: usize = 4;
const CONTEXT_FLOW_TRACE_SOURCE_LIMIT: usize = 6;
const CONTEXT_FLOW_TRACE_FANOUT: usize = 2;
const CONTEXT_FLOW_TRACE_PREVIEW_LIMIT: usize = 1;
const CONTEXT_FLOW_TRACE_PREVIEW_MAX_LINES: usize = 16;
const CONTEXT_FLOW_TRACE_PREVIEW_MAX_CHARS: usize = 360;
const CONTEXT_FLOW_SPINE_SOURCE_LIMIT: usize = 5;
const CONTEXT_FLOW_SPINE_SOURCE_MAX_LINES: usize = 100;
const CONTEXT_FLOW_SPINE_SOURCE_MAX_CHARS: usize = 2_800;
const CONTEXT_FLOW_SPINE_SOURCE_TOTAL_CHARS: usize = 5_200;
const CONTEXT_FLOW_DATA_TYPE_SOURCE_SYMBOLS: usize = 3;
const CONTEXT_FLOW_DATA_TYPE_PER_SYMBOL: usize = 4;
const CONTEXT_FLOW_DATA_TYPE_TOTAL_LIMIT: usize = 4;
const CONTEXT_FLOW_FILE_EDGE_LIMIT: usize = 4;
const CONTEXT_FLOW_FOLLOWUP_LIMIT: usize = 3;
const CONTEXT_FLOW_BODY_SCAN_MAX_LINES: usize = 360;
const CONTEXT_DEFAULT_MAX_CHARS: usize = 9_000;
const CONTEXT_DEFAULT_SNIPPET_RADIUS: usize = 2;
const CONTEXT_MAX_SNIPPET_RADIUS: usize = 4;
const CONTEXT_DEFAULT_SNIPPETS_PER_FILE: usize = 2;
const CONTEXT_MAX_SNIPPETS_PER_FILE: usize = 3;
const CONTEXT_FALLBACK_STOPWORDS: &[&str] = &[
    "about",
    "active",
    "after",
    "also",
    "analysis",
    "analyze",
    "architecture",
    "are",
    "around",
    "asynchronous",
    "before",
    "being",
    "between",
    "big",
    "boundaries",
    "change",
    "client",
    "code",
    "condition",
    "concrete",
    "could",
    "current",
    "data",
    "disabled",
    "distinguish",
    "does",
    "each",
    "editor",
    "ensure",
    "entries",
    "entry",
    "event",
    "evidence",
    "exact",
    "execution",
    "failure",
    "fallback",
    "feature",
    "flow",
    "framework",
    "from",
    "game",
    "gameplay",
    "handle",
    "handling",
    "have",
    "identify",
    "implement",
    "important",
    "improve",
    "investigate",
    "interface",
    "identifier",
    "identifiers",
    "into",
    "just",
    "key",
    "legacy",
    "like",
    "logic",
    "main",
    "make",
    "makes",
    "more",
    "most",
    "module",
    "need",
    "needs",
    "only",
    "open",
    "opened",
    "opens",
    "opening",
    "order",
    "other",
    "over",
    "paths",
    "referenced",
    "relevant",
    "readiness",
    "reconnect",
    "retry",
    "runtime",
    "source",
    "state",
    "states",
    "where",
    "whether",
    "point",
    "points",
    "populate",
    "populated",
    "populates",
    "populating",
    "search",
    "should",
    "some",
    "source",
    "setup",
    "such",
    "than",
    "that",
    "term",
    "terms",
    "test",
    "the",
    "their",
    "them",
    "then",
    "there",
    "they",
    "this",
    "transition",
    "transitions",
    "under",
    "update",
    "updates",
    "used",
    "using",
    "unity",
    "very",
    "what",
    "when",
    "where",
    "which",
    "while",
    "will",
    "with",
    "would",
];

pub struct ProjectManager {
    default_root: PathBuf,
    options: RwLock<IndexOptions>,
    options_revision: AtomicU64,
    cache: RwLock<HashMap<String, Arc<Codebase>>>,
    build_lock: Mutex<()>,
}

pub enum ReindexCheck {
    Unchanged,
    Reindexed(Arc<Codebase>),
}

pub enum ReloadCheck {
    Unchanged,
    Reindexed(Arc<Codebase>),
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
            options: RwLock::new(options),
            options_revision: AtomicU64::new(0),
            cache: RwLock::new(HashMap::new()),
            build_lock: Mutex::new(()),
        }
    }

    pub fn get(&self, project: Option<&str>) -> Result<Arc<Codebase>> {
        let started = Instant::now();
        let root = requested_project_path(&self.default_root, project).canonicalize()?;
        let key = root.display().to_string();
        if let Some(index) = self.cache.read().get(&key) {
            event_log::emit(|| {
                format!(
                    "event=project_get root={key} cache=memory elapsed_ms={:.3}",
                    started.elapsed().as_secs_f64() * 1000.0
                )
            });
            return Ok(index.clone());
        }
        let _guard = self.build_lock.lock();
        if let Some(index) = self.cache.read().get(&key) {
            event_log::emit(|| {
                format!(
                    "event=project_get root={key} cache=memory_after_lock elapsed_ms={:.3}",
                    started.elapsed().as_secs_f64() * 1000.0
                )
            });
            return Ok(index.clone());
        }
        event_log::emit(|| format!("event=project_index_start root={key} reason=get"));
        let options = self.options();
        let mut index = Codebase::index(&root, options)?;
        index.changed_files = initial_changes(&index);
        let index = Arc::new(index);
        self.cache.write().insert(key, index.clone());
        let stats = index.stats();
        event_log::emit(|| {
            format!(
                "event=project_get root={} cache={} elapsed_ms={:.3} files={} chunks={} symbols={}",
                stats.root,
                stats.cache,
                started.elapsed().as_secs_f64() * 1000.0,
                stats.files,
                stats.chunks,
                stats.symbols
            )
        });
        Ok(index)
    }

    pub fn reindex(&self, path: &Path) -> Result<Arc<Codebase>> {
        let started = Instant::now();
        let root = path.canonicalize()?;
        let key = root.display().to_string();
        event_log::emit(|| format!("event=project_index_start root={key} reason=reindex"));
        let _guard = self.build_lock.lock();
        let old = self.cache.read().get(&key).cloned();
        let options = self.options();
        let mut index = Codebase::index(&root, options)?;
        index.changed_files = match old.as_deref() {
            Some(old) => diff_changes(old, &index),
            None => initial_changes(&index),
        };
        let index = Arc::new(index);
        self.cache.write().insert(key, index.clone());
        let stats = index.stats();
        event_log::emit(|| {
            format!(
                "event=project_index_finish root={} cache={} elapsed_ms={:.3} files={} chunks={} symbols={} changed_files={}",
                stats.root,
                stats.cache,
                started.elapsed().as_secs_f64() * 1000.0,
                stats.files,
                stats.chunks,
                stats.symbols,
                index.changed_files.len()
            )
        });
        Ok(index)
    }

    pub fn apply_default_changes(
        &self,
        changed_paths: Vec<String>,
        deleted_paths: Vec<String>,
    ) -> Result<ReindexCheck> {
        let started = Instant::now();
        let changed_count = changed_paths.len();
        let deleted_count = deleted_paths.len();
        if changed_paths.is_empty() && deleted_paths.is_empty() {
            event_log::emit(|| "event=live_update_skip reason=no_changes".to_string());
            return Ok(ReindexCheck::Unchanged);
        }
        let root = self.default_root.canonicalize()?;
        let key = root.display().to_string();
        event_log::emit(|| {
            format!(
                "event=live_update_start root={key} changed={changed_count} deleted={deleted_count}"
            )
        });
        let _guard = self.build_lock.lock();
        let Some(old) = self.cache.read().get(&key).cloned() else {
            event_log::emit(|| format!("event=live_update_no_cache root={key} action=full_index"));
            let options = self.options();
            let mut index = Codebase::index(&root, options)?;
            index.changed_files = initial_changes(&index);
            let index = Arc::new(index);
            self.cache.write().insert(key, index.clone());
            let stats = index.stats();
            event_log::emit(|| {
                format!(
                    "event=live_update_finish root={} mode=full_index elapsed_ms={:.3} changed={} deleted={} files={} chunks={} symbols={} cache={}",
                    stats.root,
                    started.elapsed().as_secs_f64() * 1000.0,
                    changed_count,
                    deleted_count,
                    stats.files,
                    stats.chunks,
                    stats.symbols,
                    stats.cache
                )
            });
            return Ok(ReindexCheck::Reindexed(index));
        };
        let mut index = old.update_known_paths(&changed_paths, &deleted_paths)?;
        index.changed_files = changed_files_from_paths(&root, &changed_paths, &deleted_paths);
        let index = Arc::new(index);
        self.cache.write().insert(key, index.clone());
        let stats = index.stats();
        event_log::emit(|| {
            format!(
                "event=live_update_finish root={} mode=incremental elapsed_ms={:.3} changed={} deleted={} files={} chunks={} symbols={} cache={}",
                stats.root,
                started.elapsed().as_secs_f64() * 1000.0,
                changed_count,
                deleted_count,
                stats.files,
                stats.chunks,
                stats.symbols,
                stats.cache
            )
        });
        Ok(ReindexCheck::Reindexed(index))
    }

    pub fn reload_options(&self, new_options: IndexOptions) -> Result<ReloadCheck> {
        let root = self.default_root.canonicalize()?;
        let key = root.display().to_string();
        if self.options().cache_identity_eq(&new_options) {
            *self.options.write() = new_options;
            return Ok(ReloadCheck::Unchanged);
        }

        let _guard = self.build_lock.lock();
        if self.options().cache_identity_eq(&new_options) {
            *self.options.write() = new_options;
            return Ok(ReloadCheck::Unchanged);
        }

        let old = self.cache.read().get(&key).cloned();
        let mut index = Codebase::index(&root, new_options.clone())?;
        index.changed_files = match old.as_deref() {
            Some(old) => diff_changes(old, &index),
            None => initial_changes(&index),
        };
        let index = Arc::new(index);
        {
            let mut cache = self.cache.write();
            cache.clear();
            cache.insert(key, index.clone());
        }
        *self.options.write() = new_options;
        self.options_revision.fetch_add(1, Ordering::AcqRel);
        Ok(ReloadCheck::Reindexed(index))
    }

    pub fn default_root(&self) -> PathBuf {
        self.default_root.clone()
    }

    pub fn options(&self) -> IndexOptions {
        self.options.read().clone()
    }

    pub fn options_revision(&self) -> u64 {
        self.options_revision.load(Ordering::Acquire)
    }

    pub fn projects(&self) -> Vec<String> {
        let mut projects = self.cache.read().keys().cloned().collect::<Vec<_>>();
        projects.sort();
        projects
    }
}

fn changed_files_from_paths(
    root: &Path,
    changed_paths: &[String],
    deleted_paths: &[String],
) -> Vec<ChangedFile> {
    let mut seen = BTreeSet::new();
    let mut changes = Vec::new();
    for path in changed_paths {
        let path = normalize_rel_path(path);
        if !seen.insert(path.clone()) {
            continue;
        }
        let size = fs::metadata(root.join(&path))
            .map(|metadata| metadata.len() as usize)
            .unwrap_or_default();
        changes.push(ChangedFile {
            path,
            op: "modified",
            size,
        });
    }
    for path in deleted_paths {
        let path = normalize_rel_path(path);
        if !seen.insert(path.clone()) {
            continue;
        }
        changes.push(ChangedFile {
            path,
            op: "deleted",
            size: 0,
        });
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

pub fn dispatch_cached_cli_tool(
    default_root: &Path,
    options: &IndexOptions,
    name: &str,
    args: &Value,
) -> Result<Option<String>> {
    if !matches!(
        name,
        "codedb_status" | "codedb_find" | "codedb_deps" | "codedb_callers" | "codedb_outline"
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
        "codedb_outline" => handle_cached_outline(&cache, options, args),
        "codedb_deps" => Ok(cache
            .load_deps_snapshot(options)?
            .map(|snapshot| handle_cached_deps(&snapshot, args))
            .transpose()?),
        "codedb_callers" => handle_cached_callers(&cache, options, args),
        _ => Ok(None),
    }
}

fn tool_project_root(default_root: &Path, args: &Value) -> Result<PathBuf> {
    let root = requested_project_path(default_root, args.get("project").and_then(Value::as_str));
    Ok(root.canonicalize()?)
}

fn requested_project_path(default_root: &Path, project: Option<&str>) -> PathBuf {
    let Some(project) = project.map(str::trim).filter(|project| !project.is_empty()) else {
        return default_root.to_path_buf();
    };
    let requested = Path::new(project);
    let is_single_name = requested.components().count() == 1;
    let matches_default_name = default_root.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .eq_ignore_ascii_case(&requested.as_os_str().to_string_lossy())
    });
    if is_single_name && matches_default_name {
        default_root.to_path_buf()
    } else {
        requested.to_path_buf()
    }
}

fn format_cached_status(
    options: &IndexOptions,
    status: &crate::cache::CachedStatusSnapshot,
) -> String {
    format!(
        "codedb status:\n  seq: {}\n  files: {}\n  outlines: {}\n  chunks: {}\n  graph: {} nodes, {} edges, {} communities\n  retrieval: property graph query + lazy semantic expansion\n  scan: ready\n  extensions: {}\n  cache: hit\n  storage: {}\n",
        status.seq,
        status.files,
        status.files,
        status.chunks,
        status.graph_stats.nodes,
        status.graph_stats.edges,
        status.graph_stats.communities,
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

fn handle_cached_outline(
    cache: &ProjectCache,
    options: &IndexOptions,
    args: &Value,
) -> Result<Option<String>> {
    if get_bool_default(args, "include_body_followups", true) {
        return Ok(None);
    }
    let path = required_str(args, "path")?;
    let compact = get_bool(args, "compact");
    let Some(file) = cache.load_outline_file(options, &path)? else {
        return Ok(None);
    };
    Ok(Some(format_cached_outline(&file, compact)))
}

fn format_cached_outline(file: &CachedFileEntry, compact: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{} ({}, {} lines, {} bytes)\n",
        file.path, file.language, file.line_count, file.byte_size
    ));
    out.push_str(&format!(
        "same-file atomicity: if several listed members are needed, rerun codedb_outline path={} compact=true include_connected_ranges=true to get graph-connected joint read ranges; do not open each member separately.\n",
        file.path
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
    out
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
    dispatch_tool_with_context(manager, name, args, ToolLogContext::direct())
}

fn dispatch_tool_with_context(
    manager: &ProjectManager,
    name: &str,
    args: &Value,
    context: ToolLogContext,
) -> String {
    if name == "codedb_bundle" {
        return dispatch_tool_inner(manager, name, args);
    }
    if !event_log::enabled() {
        return dispatch_tool_inner(manager, name, args);
    }
    let started = Instant::now();
    let output = dispatch_tool_inner(manager, name, args);
    event_log::log_tool_result(name, context, started, &output);
    output
}

fn dispatch_tool_inner(manager: &ProjectManager, name: &str, args: &Value) -> String {
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
        "codedb_tree" => handle_tree(index, args),
        "codedb_outline" => handle_outline(index, args),
        "codedb_symbol" => handle_symbol(index, args),
        "codedb_search" => handle_search(index, args),
        "codedb_word" => handle_word(index, args),
        "codedb_callers" => handle_callers(index, args),
        "codedb_callpath" => handle_callpath(index, args),
        "codedb_graph_query" => handle_graph_query(index, args),
        "codedb_diagnostics" => handle_diagnostics(args),
        "codedb_hot" => Ok(handle_hot(index, args)),
        "codedb_deps" => handle_deps(index, args),
        "codedb_read" => handle_read(index, args),
        "codedb_changes" => Ok(handle_changes(index, args)),
        "codedb_status" => Ok(handle_status(index)),
        "codedb_snapshot" => Ok(handle_snapshot(index)),
        "codedb_find" => handle_find(index, args),
        "codedb_context" => handle_context(index, args),
        "codedb_flow" => handle_flow(index, args),
        "codedb_module_atlas" => handle_module_atlas(index, args),
        "codedb_glob" => handle_glob(index, args),
        "codedb_ls" => handle_ls(index, args),
        "codedb_query" => handle_query(index, args),
        _ => Ok(format!("error: unknown tool: {name}")),
    }
}

fn handle_tree(index: &Codebase, args: &Value) -> Result<String> {
    let full = get_bool(args, "full");
    let max_depth = get_usize(args, "max_depth").unwrap_or(3).clamp(1, 12);
    let max_results = if full {
        get_usize(args, "max_results")
            .unwrap_or(5_000)
            .clamp(1, 5_000)
    } else {
        get_usize(args, "max_results")
            .unwrap_or(120)
            .clamp(1, 5_000)
    };
    let requested_include_files = args.get("include_files").and_then(Value::as_bool);
    let path_prefix = get_str(args, "path_prefix")
        .or_else(|| get_str(args, "target_path"))
        .or_else(|| get_str(args, "target"))
        .or_else(|| get_str(args, "prefix"))
        .map(|prefix| normalize_rel_path(&prefix))
        .filter(|prefix| !prefix.is_empty());
    let path_glob = get_str(args, "path_glob").or_else(|| get_str(args, "glob"));
    let globset = path_glob.as_deref().map(build_globset).transpose()?;

    let mut dirs = BTreeMap::<String, (usize, usize, usize)>::new();
    let mut shown_files = Vec::new();
    let mut matched_files = 0usize;
    let mut matched_lines = 0usize;
    let mut matched_symbols = 0usize;
    for file in index.files.values() {
        if path_prefix
            .as_ref()
            .is_some_and(|prefix| !path_matches_prefix(&file.path, prefix))
        {
            continue;
        }
        if globset
            .as_ref()
            .is_some_and(|glob| !glob.is_match(&file.path))
        {
            continue;
        }
        matched_files += 1;
        matched_lines += file.line_count;
        matched_symbols += file.symbols.len();
        add_tree_dir_summaries(&mut dirs, file, max_depth);
        if shown_files.len() < max_results {
            shown_files.push(file);
        }
    }
    let include_files = requested_include_files.unwrap_or_else(|| {
        full || path_prefix.is_some() || path_glob.is_some() || matched_files <= max_results
    });

    let mut out = String::new();
    out.push_str(&format!("{}\n", index.root.display()));
    out.push_str(&format!(
        "tree summary: files={} lines={} symbols={} max_depth={} showing_files={}/{}\n",
        matched_files,
        matched_lines,
        matched_symbols,
        max_depth,
        shown_files.len(),
        matched_files
    ));
    if let Some(prefix) = &path_prefix {
        out.push_str(&format!("path_prefix: {prefix}\n"));
    }
    if let Some(glob) = path_glob {
        out.push_str(&format!("path_glob: {glob}\n"));
    }
    if !dirs.is_empty() {
        out.push_str("directories:\n");
        for (path, (files, lines, symbols)) in dirs.iter().take(max_results.min(300)) {
            out.push_str(&format!(
                "  {} ({files} files, {lines}L, {symbols} sym)\n",
                path
            ));
        }
        if dirs.len() > max_results.min(300) {
            out.push_str(&format!(
                "  [dirs {}/{}]\n",
                max_results.min(300),
                dirs.len()
            ));
        }
    }
    if include_files && !shown_files.is_empty() {
        out.push_str("files:\n");
        for file in shown_files {
            out.push_str(&format!(
                "  {} ({}, {}L, {} sym)\n",
                file.path,
                file.language,
                file.line_count,
                file.symbols.len()
            ));
        }
    } else if matched_files > 0 {
        out.push_str("files omitted; set include_files=true for focused pages.\n");
    }
    if include_files && matched_files > max_results {
        out.push_str(&format!(
            "[tree {max_results}/{matched_files}; narrow with path_prefix/path_glob]\n"
        ));
    }
    Ok(out)
}

fn add_tree_dir_summaries(
    dirs: &mut BTreeMap<String, (usize, usize, usize)>,
    file: &FileEntry,
    max_depth: usize,
) {
    let parts = file
        .path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let dir_count = parts.len().saturating_sub(1);
    for depth in 1..=dir_count.min(max_depth) {
        let entry = dirs.entry(parts[..depth].join("/")).or_default();
        entry.0 += 1;
        entry.1 += file.line_count;
        entry.2 += file.symbols.len();
    }
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
    if !get_bool(args, "include_connected_ranges")
        && file
            .symbols
            .iter()
            .filter(|symbol| {
                matches!(
                    symbol.kind.as_str(),
                    "method" | "function" | "constructor" | "procedure" | "macro" | "property"
                )
            })
            .take(2)
            .count()
            >= 2
    {
        out.push_str(&format!(
            "same-file atomicity: if the answer needs two or more listed members, rerun codedb_outline path={} compact=true include_connected_ranges=true before opening individual symbol bodies.\n",
            file.path
        ));
    }
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
    if get_bool(args, "include_connected_ranges") {
        append_outline_connected_member_ranges(index, file, &mut out)?;
    }
    if get_bool_default(args, "include_body_followups", true) {
        append_outline_body_followup_candidates(index, file, &mut out)?;
        append_outline_literal_bridge_leads(index, file, &mut out)?;
    }
    Ok(out)
}

fn append_outline_connected_member_ranges(
    index: &Codebase,
    file: &FileEntry,
    out: &mut String,
) -> Result<()> {
    let members = file
        .symbols
        .iter()
        .filter(|symbol| {
            matches!(
                symbol.kind.as_str(),
                "method" | "function" | "constructor" | "procedure" | "macro" | "property"
            )
        })
        .collect::<Vec<_>>();
    if members.len() < 2 {
        return Ok(());
    }
    let mut by_name = BTreeMap::<String, Vec<usize>>::new();
    for (idx, symbol) in members.iter().enumerate() {
        by_name.entry(symbol.name.clone()).or_default().push(idx);
    }
    let content = index.file_content(file)?;
    let active_content = mask_comments(file.language.as_str(), &content);
    let mut adjacency = vec![BTreeSet::<usize>::new(); members.len()];
    for (idx, symbol) in members.iter().enumerate() {
        let body = source_line_slice(
            &active_content,
            symbol.line_start,
            symbol.line_end.max(symbol.line_start),
        );
        let mut referenced = raw_identifiers(&body);
        referenced.sort();
        referenced.dedup();
        for name in referenced {
            for &target in by_name.get(&name).into_iter().flatten() {
                if target == idx {
                    continue;
                }
                adjacency[idx].insert(target);
                adjacency[target].insert(idx);
            }
        }
    }

    let mut visited = vec![false; members.len()];
    let mut components = Vec::<Vec<usize>>::new();
    for start in 0..members.len() {
        if visited[start] || adjacency[start].is_empty() {
            continue;
        }
        visited[start] = true;
        let mut queue = VecDeque::from([start]);
        let mut component = Vec::new();
        while let Some(current) = queue.pop_front() {
            component.push(current);
            for &neighbor in &adjacency[current] {
                if !visited[neighbor] {
                    visited[neighbor] = true;
                    queue.push_back(neighbor);
                }
            }
        }
        if component.len() >= 2 {
            component.sort_by_key(|idx| members[*idx].line_start);
            components.push(component);
        }
    }
    if components.is_empty() {
        return Ok(());
    }
    components.sort_by_key(|component| members[component[0]].line_start);
    out.push_str("outline connected member ranges (same-file call/reference components; use one compact range when several members are needed):\n");
    for component in components {
        let line_start = component
            .iter()
            .map(|idx| members[*idx].line_start)
            .min()
            .unwrap_or(1);
        let line_end = component
            .iter()
            .map(|idx| members[*idx].line_end.max(members[*idx].line_start))
            .max()
            .unwrap_or(line_start);
        let names = component
            .iter()
            .map(|idx| members[*idx].name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "  L{line_start}-L{line_end} members=[{names}] -> codedb_read path={} line_start={line_start} line_end={line_end} compact=true connected_range=true include_symbol_leads=true\n",
            file.path
        ));
    }
    Ok(())
}

fn append_outline_literal_bridge_leads(
    index: &Codebase,
    file: &FileEntry,
    out: &mut String,
) -> Result<()> {
    if file.line_count > OUTLINE_LITERAL_BRIDGE_MAX_FILE_LINES {
        return Ok(());
    }
    let content = index.file_content(file)?;
    let active_content = mask_comments(file.language.as_str(), &content);
    let leads = symbol_body_literal_bridge_leads(
        index,
        file,
        &active_content,
        OUTLINE_LITERAL_BRIDGE_LEAD_LIMIT,
    );
    if leads.is_empty() {
        return Ok(());
    }
    out.push_str("outline literal bridge leads:\n");
    for lead in leads {
        out.push_str(&format!(
            "  \"{}\" [{}] -> {}:{} ({}) // {}\n",
            compact_inline_text(&lead.literal, 80),
            lead.matched.into_iter().collect::<Vec<_>>().join("/"),
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            lead.target.detail
        ));
    }
    Ok(())
}

fn handle_symbol(index: &Codebase, args: &Value) -> Result<String> {
    let name = get_str(args, "name");
    let prefix = get_str(args, "prefix");
    let pattern = get_str(args, "pattern");
    let kind = get_str(args, "kind");
    if name.is_none() && prefix.is_none() && pattern.is_none() && kind.is_none() {
        return Ok("error: pass 'name', 'prefix', 'pattern', or 'kind'".to_string());
    }
    let include_body = get_bool(args, "body");
    let expand_body_evidence = get_bool(args, "expand") || get_bool(args, "include_refs");
    let fuzzy = get_bool(args, "fuzzy");
    let max_results = get_usize(args, "max_results").unwrap_or(50).clamp(1, 200);
    let path_filter = get_str(args, "path")
        .or_else(|| get_str(args, "definition_path"))
        .map(|path| normalize_project_path_filter(index, &path));
    let definition_line = get_usize(args, "definition_line").or_else(|| get_usize(args, "line"));
    let path_glob = get_str(args, "path_glob").or_else(|| get_str(args, "glob"));
    let globset = path_glob.as_deref().map(build_globset).transpose()?;
    let mut results = Vec::new();
    for file in index.files.values() {
        if let Some(path_filter) = &path_filter
            && file.path != *path_filter
        {
            continue;
        }
        if let Some(globset) = &globset
            && !globset.is_match(&file.path)
        {
            continue;
        }
        for symbol in &file.symbols {
            if let Some(filter_kind) = &kind
                && symbol.kind != filter_kind.as_str()
            {
                continue;
            }
            if let Some(line) = definition_line
                && symbol.line_start != line
                && !(symbol.line_start <= line && line <= symbol.line_end)
            {
                continue;
            }
            let score = if let Some(name) = &name {
                if symbol.name == *name {
                    Some(1.0)
                } else if fuzzy {
                    fuzzy_score(&symbol.name, name)
                } else {
                    None
                }
            } else if let Some(prefix) = &prefix {
                symbol_prefix_or_token_score(&symbol.name, prefix)
            } else if let Some(pattern) = &pattern {
                wildcard_match(pattern, &symbol.name).then_some(0.8)
            } else {
                Some(0.5)
            };
            if let Some(score) = score {
                results.push((
                    file,
                    symbol,
                    symbol_lookup_score(index, file, symbol, score),
                ));
            }
        }
    }
    results.sort_by(|a, b| {
        b.2.total_cmp(&a.2)
            .then_with(|| a.0.path.cmp(&b.0.path))
            .then_with(|| a.1.line_start.cmp(&b.1.line_start))
    });
    if name.is_none() && !include_body {
        results = diversify_symbol_results(results, max_results, SYMBOL_EXPLORATORY_PER_FILE_LIMIT);
    } else {
        results.truncate(max_results);
    }
    if results.is_empty() {
        let label = name.or(prefix).or(pattern).or(kind).unwrap_or_default();
        if let Some(path) = path_filter {
            return Ok(format!(
                "no exact symbol '{label}' in {path}. Do not probe another invented lifecycle name. Inspect this file once with codedb_outline path={path} compact=true skeleton=true include_body_followups=true, then request only a symbol returned verbatim by that outline."
            ));
        }
        return Ok(format!("no results for: {label}"));
    }
    if get_str(args, "format").as_deref() == Some("json") {
        let items = results
            .iter()
            .map(|(file, symbol, score)| {
                json!({
                    "path": file.path,
                    "line": symbol.line_start,
                    "line_start": symbol.line_start,
                    "line_end": symbol.line_end,
                    "kind": symbol.kind,
                    "name": symbol.name,
                    "detail": symbol.detail,
                    "score": score
                })
            })
            .collect::<Vec<_>>();
        return serde_json::to_string_pretty(&json!({
            "ok": true,
            "tool": "codedb_symbol",
            "results": items
        }))
        .map_err(Into::into);
    }
    let mut out = format!("{} symbol results:\n", results.len());
    for (file, symbol, score) in results {
        out.push_str(&format!(
            "  {}:{} ({}) score={:.3}  // {}\n",
            file.path, symbol.line_start, symbol.kind, score, symbol.detail
        ));
        if include_body {
            let content = index.file_content(file)?;
            let active_content = mask_comments(file.language.as_str(), &content);
            let body = source_line_slice(&active_content, symbol.line_start, symbol.line_end);
            let span = symbol
                .line_end
                .max(symbol.line_start)
                .saturating_sub(symbol.line_start)
                + 1;
            if is_large_container_symbol(symbol, span) {
                append_symbol_body_container_outline(file, symbol, &mut out);
                out.push_str(&format!(
                    "body omitted: {span}L container; use codedb_symbol name=<member> path={} body=true max_results=1.\n",
                    file.path
                ));
                continue;
            }
            let primary_evidence = symbol_body_primary_evidence(index, file, symbol, &body);
            append_symbol_body_evidence_card(file, symbol, &primary_evidence, &mut out);
            append_symbol_activity_summary(index, file, symbol, &mut out)?;
            append_symbol_body_primary_handoff_leads(
                index,
                file,
                symbol,
                &body,
                &primary_evidence,
                &mut out,
            );
            if !expand_body_evidence {
                append_symbol_body_ordered_leads(index, file, symbol, &body, &mut out, false)?;
                let literal_source =
                    symbol_body_literal_source(file, symbol, &active_content, &body);
                append_symbol_body_literal_bridge_leads(index, file, &literal_source, &mut out);
            }
            if span > READ_FULL_RANGE_MAX_LINES {
                out.push_str("body lines (active code; comments omitted):\n");
                out.push_str(&extract_lines(
                    &active_content,
                    symbol.line_start,
                    symbol.line_end,
                    true,
                ));
            } else {
                out.push_str("body lines:\n");
                out.push_str(&extract_lines(
                    &content,
                    symbol.line_start,
                    symbol.line_end,
                    false,
                ));
            }
            if expand_body_evidence {
                append_symbol_body_leads(
                    index,
                    file,
                    symbol,
                    &body,
                    &active_content,
                    &mut out,
                    false,
                )?;
            } else {
                out.push_str("graph follow-up: use codedb_graph_query with this exact symbol and typed CALLS/DISPATCHES_TO/HAS_CALLSITE edges when another evidence phase is required; do not reopen forwarding wrappers one by one.\n");
            }
        }
    }
    Ok(out)
}

fn symbol_dispatch_candidates(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
) -> Vec<SymbolTarget> {
    let Some(enclosing) = enclosing_type_symbol(file, symbol) else {
        return Vec::new();
    };
    if !matches!(enclosing.kind.as_str(), "interface" | "trait") {
        return Vec::new();
    }
    let parameter_count = signature_parameter_count(&symbol.detail);
    let mut candidates = index
        .symbols_named(&symbol.name)
        .into_iter()
        .filter(|(candidate_file, candidate_symbol)| {
            is_context_handoff_source_symbol(candidate_symbol)
                && (candidate_file.path != file.path
                    || candidate_symbol.line_start != symbol.line_start)
                && parameter_count.is_none_or(|count| {
                    signature_accepts_argument_count(&candidate_symbol.detail, count)
                })
                && enclosing_type_symbol(candidate_file, candidate_symbol).is_some_and(
                    |candidate_enclosing| {
                        raw_identifiers(&candidate_enclosing.detail)
                            .into_iter()
                            .any(|identifier| identifier == enclosing.name)
                    },
                )
        })
        .map(|(candidate_file, candidate_symbol)| {
            target_from_symbol(candidate_file, candidate_symbol)
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.line_start.cmp(&right.line_start))
    });
    candidates.dedup_by(|left, right| {
        left.path == right.path && left.line_start == right.line_start && left.name == right.name
    });
    candidates
}

fn symbol_target_dispatch_candidates(index: &Codebase, target: &SymbolTarget) -> Vec<SymbolTarget> {
    let Some(file) = index.file(&target.path) else {
        return Vec::new();
    };
    let Some(symbol) = symbol_for_target(file, target) else {
        return Vec::new();
    };
    symbol_dispatch_candidates(index, file, symbol)
}

#[allow(dead_code)]
fn append_dispatch_candidates(
    index: &Codebase,
    source: &Symbol,
    candidates: &[SymbolTarget],
    indent: &str,
    out: &mut String,
) {
    for candidate in candidates.iter().take(SYMBOL_BODY_DISPATCH_PREVIEW_LIMIT) {
        out.push_str(&format!(
            "{indent}{} -> {}:{} ({})\n{indent}  follow-up: codedb_symbol name={} path={} body=true max_results=1\n",
            source.name,
            candidate.path,
            candidate.line_start,
            candidate.kind,
            candidate.name,
            candidate.path
        ));
        if let Some(snippet) = compact_symbol_target_snippet_limited(
            index,
            candidate,
            SYMBOL_BODY_DISPATCH_PREVIEW_MAX_LINES,
            SYMBOL_BODY_DISPATCH_PREVIEW_MAX_CHARS,
        ) {
            out.push_str(&format!("{indent}  exact branch preview: {}\n", snippet));
        }
        if !append_dispatch_candidate_contracted_leaf(index, candidate, indent, out) {
            append_parameter_control_evidence(index, candidate, &format!("{indent}  "), out);
        }
    }
}

#[allow(dead_code)]
fn append_dispatch_candidate_contracted_leaf(
    index: &Codebase,
    candidate: &SymbolTarget,
    indent: &str,
    out: &mut String,
) -> bool {
    let Some(file) = index.file(&candidate.path) else {
        return false;
    };
    let Some(symbol) = symbol_for_target(file, candidate) else {
        return false;
    };
    let Ok(content) = index.file_content(file) else {
        return false;
    };
    let active_content = mask_comments(file.language.as_str(), &content);
    let body = source_line_slice(
        &active_content,
        symbol.line_start,
        symbol.line_end.max(symbol.line_start),
    );
    if !is_short_flow_wrapper_body(&body) {
        return false;
    }
    let Ok(chain) = continue_symbol_target(index, candidate.clone(), 0) else {
        return false;
    };
    let Some(terminal) = chain.steps.last() else {
        return false;
    };
    out.push_str(&format!(
        "{indent}  contracted implementation leaf: {}:{} {}",
        chain.source.path, chain.source.line_start, chain.source.name
    ));
    for step in &chain.steps {
        out.push_str(&format!(
            " -> {}:{} {}",
            step.path, step.line_start, step.name
        ));
    }
    out.push('\n');
    if let Some(snippet) = compact_symbol_target_snippet_limited(
        index,
        terminal,
        SYMBOL_BODY_DISPATCH_PREVIEW_MAX_LINES,
        SYMBOL_BODY_DISPATCH_PREVIEW_MAX_CHARS,
    ) {
        out.push_str(&format!("{indent}    exact leaf preview: {snippet}\n"));
    }
    append_parameter_control_evidence(index, terminal, &format!("{indent}    "), out);
    true
}

#[allow(dead_code)]
fn append_parameter_control_evidence(
    index: &Codebase,
    target: &SymbolTarget,
    indent: &str,
    out: &mut String,
) {
    let Some(file) = index.file(&target.path) else {
        return;
    };
    let Some(symbol) = symbol_for_target(file, target) else {
        return;
    };
    let Some(parameters) = signature_parameters(&symbol.detail) else {
        return;
    };
    let parameter_names = parameters
        .iter()
        .filter_map(|parameter| {
            let declaration = parameter.split('=').next().unwrap_or(parameter);
            raw_identifiers(declaration).into_iter().next_back()
        })
        .collect::<BTreeSet<_>>();
    if parameter_names.is_empty() {
        return;
    }
    let Ok(content) = index.file_content(file) else {
        return;
    };
    let active_content = mask_comments(file.language.as_str(), &content);
    let body = source_line_slice(
        &active_content,
        symbol.line_start,
        symbol.line_end.max(symbol.line_start),
    );
    let lines = body.lines().collect::<Vec<_>>();
    let mut tracked = parameter_names
        .iter()
        .map(|parameter| (parameter.clone(), parameter.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut rows = Vec::<String>::new();
    let mut seen = BTreeSet::<(usize, String)>::new();
    for (offset, line) in lines.iter().enumerate() {
        let code_line = strip_strings_and_line_comment(line);
        let mut roots = tracked
            .iter()
            .filter(|(name, _)| line_contains_identifier_token(&code_line, name))
            .map(|(_, root)| root.clone())
            .collect::<BTreeSet<_>>();
        if roots.is_empty() {
            continue;
        }
        let line_number = symbol.line_start + offset;
        if let Some(alias) = assignment_target_identifier(&code_line)
            && !roots.contains(&alias)
        {
            let root = roots.iter().next().cloned().unwrap_or_default();
            if !root.is_empty() {
                tracked.insert(alias, root);
            }
        }
        let trimmed = code_line.trim_start();
        let condition = trimmed.starts_with("if")
            || trimmed.starts_with("else if")
            || trimmed.starts_with("while")
            || trimmed.starts_with("switch")
            || trimmed.starts_with("match ");
        let control = condition
            || trimmed.starts_with("return ")
            || assignment_operator_position(&code_line).is_some();
        if !control {
            continue;
        }
        roots.retain(|root| !root.is_empty());
        let roots_text = roots.into_iter().collect::<Vec<_>>().join(", ");
        let mut row = format!(
            "[{roots_text}] L{line_number} {}",
            compact_inline_text(&code_line, 220)
        );
        let action = lines.iter().enumerate().skip(offset).take(5).find_map(
            |(candidate_offset, candidate)| {
                let candidate_code = strip_strings_and_line_comment(candidate);
                let candidate_trimmed = candidate_code.trim();
                let is_action = candidate_trimmed == "continue;"
                    || candidate_trimmed == "break;"
                    || candidate_trimmed.starts_with("return ")
                    || candidate_trimmed.starts_with("throw ")
                    || candidate_trimmed.starts_with("yield ")
                    || candidate_trimmed.contains(" continue;")
                    || candidate_trimmed.contains(" break;");
                is_action.then(|| (candidate_offset, candidate_trimmed.to_string()))
            },
        );
        if let Some((action_offset, action)) = &action {
            row.push_str(&format!(
                " => L{} {}",
                symbol.line_start + *action_offset,
                compact_inline_text(action, 120)
            ));
        }
        if condition && let Some((action_offset, _)) = action {
            let condition_false = tracked.iter().any(|(name, _)| {
                line_contains_identifier_token(&code_line, name)
                    && identifier_condition_is_negated(&code_line, name)
            });
            let predicate_state = if condition_false { "false" } else { "true" };
            if let Some((append_offset, append_line)) = lines
                .iter()
                .enumerate()
                .skip(action_offset.saturating_add(1))
                .take(24)
                .find_map(|(candidate_offset, candidate)| {
                    let candidate_code = strip_strings_and_line_comment(candidate);
                    collection_append_shape(&candidate_code)
                        .then(|| (candidate_offset, candidate_code.trim().to_string()))
                })
            {
                row.push_str(&format!(
                    " | admission consequence: parameter-derived condition {predicate_state} is excluded before L{} {} | the opposite branch can continue toward that append",
                    symbol.line_start + append_offset,
                    compact_inline_text(&append_line, 140)
                ));
            }
        }
        if seen.insert((line_number, row.clone())) {
            rows.push(row);
        }
    }
    if rows.is_empty() {
        return;
    }
    out.push_str(&format!(
        "{indent}parameter/control evidence (source order; no task keywords):\n"
    ));
    for row in rows.into_iter().take(8) {
        out.push_str(&format!("{indent}  {row}\n"));
    }
}

fn identifier_condition_is_negated(line: &str, identifier: &str) -> bool {
    let mut from = 0usize;
    while let Some(relative) = line.get(from..).and_then(|tail| tail.find(identifier)) {
        let start = from + relative;
        let end = start + identifier.len();
        let before_ok = line
            .get(..start)
            .and_then(|prefix| prefix.chars().next_back())
            .is_none_or(|ch| !is_identifier_char(ch));
        let after_ok = line
            .get(end..)
            .and_then(|suffix| suffix.chars().next())
            .is_none_or(|ch| !is_identifier_char(ch));
        if before_ok && after_ok {
            let prefix = line.get(..start).unwrap_or_default().trim_end();
            let suffix = line.get(end..).unwrap_or_default().trim_start();
            if prefix.ends_with('!')
                || prefix.ends_with("not")
                || suffix.starts_with("== false")
                || suffix.starts_with("is false")
            {
                return true;
            }
        }
        from = end.max(from + 1);
    }
    false
}

#[allow(dead_code)]
fn collection_append_shape(line: &str) -> bool {
    [
        ".Add(",
        ".add(",
        ".append(",
        ".push(",
        ".insert(",
        ".push_back(",
    ]
    .into_iter()
    .any(|shape| line.contains(shape))
}

fn append_symbol_activity_summary(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    out: &mut String,
) -> Result<()> {
    if symbol.name.len() < 4 || index.symbols_named(&symbol.name).len() != 1 {
        return Ok(());
    }
    let mut hits = reference_candidates_with_limit(index, &symbol.name, Some(512))?;
    hits.retain(|hit| {
        !(hit.path == file.path
            && hit.line >= symbol.line_start
            && hit.line <= symbol.line_end.max(symbol.line_start))
    });
    let executable_refs = hits
        .iter()
        .filter(|hit| {
            hit.scope.as_ref().is_some_and(|scope| {
                matches!(
                    scope.kind.as_str(),
                    "method" | "function" | "constructor" | "macro"
                )
            })
        })
        .count();
    out.push_str(&format!(
        "activity: active_refs={} executable_refs={}\n",
        hits.len(),
        executable_refs
    ));
    if executable_refs == 0 && is_context_handoff_source_symbol(symbol) {
        out.push_str(
            "activity caution: no indexed executable reference; verify runtime/framework wiring before treating this as the primary active path.\n",
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct OutlineBodyFollowupCandidate<'a> {
    symbol: &'a Symbol,
    incoming: usize,
    outgoing: usize,
    executable_outgoing: usize,
    span: usize,
    score: usize,
}

fn append_outline_body_followup_candidates(
    index: &Codebase,
    file: &FileEntry,
    out: &mut String,
) -> Result<()> {
    let candidates = outline_body_followup_candidates(index, file, OUTLINE_BODY_FOLLOWUP_LIMIT)?;
    if candidates.is_empty() {
        return Ok(());
    }
    out.push_str("outline body follow-up candidates:\n");
    for candidate in candidates {
        out.push_str(&format!(
            "  - s={} in={} out={} exec={} span={}L; codedb_symbol name={} path={} definition_line={} body=true max_results=1 // {} {}\n",
            candidate.score,
            candidate.incoming,
            candidate.outgoing,
            candidate.executable_outgoing,
            candidate.span,
            candidate.symbol.name,
            file.path,
            candidate.symbol.line_start,
            candidate.symbol.kind,
            candidate.symbol.detail
        ));
    }
    Ok(())
}

fn outline_body_followup_candidates<'a>(
    index: &Codebase,
    file: &'a FileEntry,
    limit: usize,
) -> Result<Vec<OutlineBodyFollowupCandidate<'a>>> {
    if limit == 0 || file.symbols.len() < 2 {
        return Ok(Vec::new());
    }
    let content = index.file_content(file)?;
    let active_content = mask_comments(file.language.as_str(), &content);
    let scan_len = file
        .symbols
        .len()
        .min(OUTLINE_BODY_FOLLOWUP_SCAN_SYMBOL_LIMIT);
    let scan_symbols = &file.symbols[..scan_len];
    let local_names = scan_symbols
        .iter()
        .filter(|symbol| is_context_handoff_source_symbol(symbol))
        .map(|symbol| symbol.name.clone())
        .collect::<BTreeSet<_>>();
    if local_names.len() < 2 {
        return Ok(Vec::new());
    }
    let known_symbol_names = index
        .files
        .values()
        .flat_map(|candidate_file| candidate_file.symbols.iter())
        .map(|symbol| symbol.name.as_str())
        .collect::<BTreeSet<_>>();
    let known_executable_names = index
        .files
        .values()
        .flat_map(|candidate_file| candidate_file.symbols.iter())
        .filter(|symbol| {
            matches!(
                symbol.kind.as_str(),
                "method" | "function" | "constructor" | "procedure" | "macro"
            )
        })
        .map(|symbol| symbol.name.as_str())
        .collect::<BTreeSet<_>>();

    let mut incoming_by_name = BTreeMap::<String, usize>::new();
    let mut outgoing_by_line = BTreeMap::<usize, usize>::new();
    let mut executable_outgoing_by_line = BTreeMap::<usize, usize>::new();
    for symbol in scan_symbols {
        if !is_context_handoff_source_symbol(symbol) {
            continue;
        }
        let body = source_line_slice(&active_content, symbol.line_start, symbol.line_end);
        let mut outgoing_names = BTreeSet::<String>::new();
        for identifier in raw_identifiers(&body) {
            if identifier == symbol.name || !known_symbol_names.contains(identifier.as_str()) {
                continue;
            }
            if !local_names.contains(&identifier) && !looks_like_context_identifier(&identifier) {
                continue;
            }
            outgoing_names.insert(identifier);
        }
        if !outgoing_names.is_empty() {
            outgoing_by_line.insert(symbol.line_start, outgoing_names.len());
            executable_outgoing_by_line.insert(
                symbol.line_start,
                outgoing_names
                    .iter()
                    .filter(|name| known_executable_names.contains(name.as_str()))
                    .count(),
            );
        }
        for target_name in outgoing_names {
            if local_names.contains(&target_name) {
                incoming_by_name
                    .entry(target_name)
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
            }
        }
    }

    let mut candidates = Vec::new();
    for symbol in scan_symbols {
        if !is_context_handoff_source_symbol(symbol) {
            continue;
        }
        let incoming = incoming_by_name
            .get(&symbol.name)
            .copied()
            .unwrap_or_default();
        let outgoing = outgoing_by_line
            .get(&symbol.line_start)
            .copied()
            .unwrap_or_default();
        let executable_outgoing = executable_outgoing_by_line
            .get(&symbol.line_start)
            .copied()
            .unwrap_or_default();
        if incoming == 0 && outgoing == 0 {
            continue;
        }
        let span = symbol
            .line_end
            .saturating_sub(symbol.line_start)
            .saturating_add(1);
        let bridge_bonus = if incoming > 0 && outgoing > 0 { 70 } else { 0 };
        let score = incoming * 80
            + outgoing * 55
            + bridge_bonus
            + span.min(120)
            + symbol_kind_lead_weight(symbol)
            + symbol_name_specificity_weight(symbol);
        candidates.push(OutlineBodyFollowupCandidate {
            symbol,
            incoming,
            outgoing,
            executable_outgoing,
            span,
            score,
        });
    }

    candidates.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.incoming.cmp(&left.incoming))
            .then_with(|| right.outgoing.cmp(&left.outgoing))
            .then_with(|| left.symbol.line_start.cmp(&right.symbol.line_start))
    });
    let mut reserved_surfaces = candidates
        .iter()
        .filter(|candidate| {
            candidate.outgoing > 0 && matches!(candidate.symbol.kind.as_str(), "property" | "field")
        })
        .take(1)
        .cloned()
        .collect::<Vec<_>>();
    if let Some(late_short_bridge) = candidates
        .iter()
        .filter(|candidate| {
            candidate.incoming > 0
                && candidate.executable_outgoing >= 2
                && candidate.span <= 20
                && matches!(
                    candidate.symbol.kind.as_str(),
                    "method" | "function" | "constructor" | "procedure"
                )
        })
        .max_by(|left, right| {
            left.symbol
                .line_start
                .cmp(&right.symbol.line_start)
                .then_with(|| left.score.cmp(&right.score))
        })
        .cloned()
    {
        reserved_surfaces.push(late_short_bridge);
    }
    let reserved_surface_keys = reserved_surfaces
        .iter()
        .map(|candidate| (candidate.symbol.line_start, candidate.symbol.name.clone()))
        .collect::<BTreeSet<_>>();
    let mut selected = candidates.into_iter().take(limit).collect::<Vec<_>>();
    for reserved_surface in reserved_surfaces {
        if selected.iter().any(|candidate| {
            candidate.symbol.line_start == reserved_surface.symbol.line_start
                && candidate.symbol.name == reserved_surface.symbol.name
        }) {
            continue;
        }
        if selected.len() >= limit {
            let replace = selected
                .iter()
                .rposition(|candidate| {
                    !reserved_surface_keys
                        .contains(&(candidate.symbol.line_start, candidate.symbol.name.clone()))
                })
                .unwrap_or(selected.len().saturating_sub(1));
            selected.remove(replace);
        }
        selected.push(reserved_surface);
    }
    selected.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.symbol.line_start.cmp(&right.symbol.line_start))
    });
    Ok(selected)
}

fn diversify_symbol_results<'a>(
    results: Vec<(&'a FileEntry, &'a Symbol, f32)>,
    max_results: usize,
    per_file_limit: usize,
) -> Vec<(&'a FileEntry, &'a Symbol, f32)> {
    if results.len() <= max_results {
        return results;
    }
    let mut groups = Vec::<Vec<(&'a FileEntry, &'a Symbol, f32)>>::new();
    let mut group_index = BTreeMap::<String, usize>::new();
    let mut per_file = BTreeMap::<String, usize>::new();
    for item in results {
        let count = per_file.entry(item.0.path.clone()).or_default();
        if *count >= per_file_limit {
            continue;
        }
        *count += 1;
        let key = symbol_result_group_key(&item.0.path);
        let idx = if let Some(idx) = group_index.get(&key).copied() {
            idx
        } else {
            let idx = groups.len();
            groups.push(Vec::new());
            group_index.insert(key, idx);
            idx
        };
        groups[idx].push(item);
    }

    let mut selected = Vec::new();
    let mut offset = 0usize;
    loop {
        let mut progressed = false;
        for group in &groups {
            if let Some(item) = group.get(offset).copied() {
                selected.push(item);
                progressed = true;
                if selected.len() >= max_results {
                    return selected;
                }
            }
        }
        if !progressed {
            break;
        }
        offset += 1;
    }
    selected
}

fn symbol_result_group_key(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .unwrap_or_else(|| path.to_string())
}

fn symbol_prefix_or_token_score(name: &str, query: &str) -> Option<f32> {
    if name.starts_with(query) {
        return Some(0.9);
    }
    let query_lower = query.to_ascii_lowercase();
    if query_lower.is_empty() {
        return None;
    }
    split_identifier(name)
        .into_iter()
        .any(|part| part == query_lower)
        .then_some(0.82)
}

fn symbol_lookup_score(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    base_score: f32,
) -> f32 {
    let kind_bonus = match symbol.kind.as_str() {
        "method" | "function" | "constructor" => 0.08,
        "class" | "interface" | "struct" | "enum" | "record" => 0.05,
        "property" | "field" => 0.01,
        _ => 0.0,
    };
    let graph_bonus = (file_graph_degree(index, &file.path).min(80) as f32) * 0.001;
    base_score + kind_bonus + graph_bonus
}

fn normalize_project_path_filter(index: &Codebase, path: &str) -> String {
    let normalized = normalize_rel_path(path);
    let root = normalize_rel_path(index.root.to_string_lossy().as_ref());
    let root = root.trim_end_matches('/');
    if !root.is_empty()
        && normalized
            .to_ascii_lowercase()
            .starts_with(&format!("{}/", root.to_ascii_lowercase()))
    {
        normalized[root.len() + 1..].to_string()
    } else {
        normalized
    }
}

#[derive(Clone)]
struct BodySymbolLead {
    order: usize,
    score: usize,
    query_matches: BTreeSet<String>,
    target: SymbolTarget,
}

#[allow(dead_code)]
struct BodySymbolContinuationChain {
    source: SymbolTarget,
    steps: Vec<SymbolTarget>,
    score: usize,
}

struct BodyLiteralBridgeLead {
    literal: String,
    matched: BTreeSet<String>,
    target: SymbolTarget,
    score: f32,
}

struct BodyAssignmentTargetLead {
    order: usize,
    line: usize,
    text: String,
    target: SymbolTarget,
}

#[derive(Clone)]
struct BodyFlowHandoffLead {
    line: usize,
    text: String,
    score: usize,
    target: SymbolTarget,
}

fn is_large_container_symbol(symbol: &Symbol, span: usize) -> bool {
    span > SYMBOL_BODY_LARGE_CONTAINER_MAX_LINES && is_container_symbol_kind(symbol.kind.as_str())
}

fn is_container_symbol_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class"
            | "interface"
            | "struct"
            | "record"
            | "module"
            | "namespace"
            | "enum"
            | "trait"
            | "impl"
            | "object"
    )
}

fn append_symbol_body_container_outline(file: &FileEntry, symbol: &Symbol, out: &mut String) {
    let mut nested = file
        .symbols
        .iter()
        .filter(|candidate| {
            candidate.line_start > symbol.line_start
                && candidate.line_end <= symbol.line_end
                && !(candidate.line_start == symbol.line_start && candidate.name == symbol.name)
        })
        .collect::<Vec<_>>();
    if nested.is_empty() {
        return;
    }
    nested.sort_by(|left, right| {
        left.line_start
            .cmp(&right.line_start)
            .then_with(|| left.line_end.cmp(&right.line_end))
            .then_with(|| left.name.cmp(&right.name))
    });
    out.push_str("large container member outline:\n");
    for item in nested.iter().take(SYMBOL_BODY_NESTED_SYMBOL_LIMIT) {
        out.push_str(&format!(
            "  L{}-L{} {} {} // {}\n",
            item.line_start,
            item.line_end,
            item.kind,
            item.name,
            compact_inline_text(&item.detail, 120)
        ));
    }
    if nested.len() > SYMBOL_BODY_NESTED_SYMBOL_LIMIT {
        out.push_str(&format!(
            "  [...{} more]\n",
            nested.len() - SYMBOL_BODY_NESTED_SYMBOL_LIMIT
        ));
    }
    let mut followups = nested
        .iter()
        .filter(|item| is_context_handoff_source_symbol(item))
        .take(SYMBOL_BODY_NESTED_FOLLOWUP_LIMIT)
        .collect::<Vec<_>>();
    if followups.is_empty() {
        followups = nested
            .iter()
            .take(SYMBOL_BODY_NESTED_FOLLOWUP_LIMIT)
            .collect::<Vec<_>>();
    }
    if followups.is_empty() {
        return;
    }
    out.push_str("member followups:\n");
    for item in followups {
        out.push_str(&format!(
            "  codedb_symbol name={} path={} body=true max_results=1\n",
            item.name, file.path
        ));
    }
}

fn append_symbol_body_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    active_content: &str,
    out: &mut String,
    include_primary_handoffs: bool,
) -> Result<()> {
    let short_wrapper = is_short_flow_wrapper_body(body);
    let leads = symbol_body_leads(index, file, symbol, body, SYMBOL_BODY_LEAD_LIMIT);
    append_symbol_body_ordered_leads(index, file, symbol, body, out, !short_wrapper)?;
    let literal_source = symbol_body_literal_source(file, symbol, active_content, body);
    append_symbol_body_literal_bridge_leads(index, file, &literal_source, out);
    if include_primary_handoffs {
        let evidence = symbol_body_primary_evidence(index, file, symbol, body);
        append_symbol_body_primary_handoff_leads(index, file, symbol, body, &evidence, out);
    }
    if short_wrapper {
        out.push_str("short wrapper: extra refs omitted; inspect shown handoff if needed.\n");
        return Ok(());
    }
    if !leads.is_empty() {
        out.push_str("body symbol leads:\n");
        for lead in &leads {
            out.push_str(&format!(
                "  {} -> {}:{} ({})  // {}\n",
                lead.target.name,
                lead.target.path,
                lead.target.line_start,
                lead.target.kind,
                lead.target.detail
            ));
        }
        append_symbol_body_lead_previews(index, &leads, out)?;
        append_same_file_callee_exact_reference_leads(index, file, &leads, out)?;
    }
    append_symbol_body_data_type_leads(index, file, symbol, body, out)?;
    append_symbol_incoming_reference_leads(
        index,
        file,
        symbol,
        out,
        "",
        SYMBOL_BODY_INCOMING_REF_LIMIT,
    )?;
    append_symbol_body_exact_reference_leads(index, file, symbol, body, out)?;
    Ok(())
}

fn symbol_body_literal_source(
    file: &FileEntry,
    symbol: &Symbol,
    active_content: &str,
    body: &str,
) -> String {
    let referenced_names = source_code_identifiers(body)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut source = body.to_string();
    for candidate in file.symbols.iter().filter(|candidate| {
        candidate.line_start != symbol.line_start
            && referenced_names.contains(&candidate.name)
            && matches!(
                candidate.kind.as_str(),
                "field" | "constant" | "variable" | "property"
            )
    }) {
        source.push_str(&source_line_slice(
            active_content,
            candidate.line_start,
            candidate.line_end.max(candidate.line_start),
        ));
    }
    let mut added_assignment_lines = 0usize;
    for (idx, line) in active_content.lines().enumerate() {
        let line_number = idx + 1;
        if (symbol.line_start..=symbol.line_end.max(symbol.line_start)).contains(&line_number)
            || !line.contains('"')
            || assignment_operator_position(line).is_none()
            || !referenced_names
                .iter()
                .any(|name| name.len() >= 4 && line_contains_identifier_token(line, name))
        {
            continue;
        }
        source.push_str(line);
        source.push('\n');
        added_assignment_lines += 1;
        if added_assignment_lines >= 16 {
            break;
        }
    }
    source
}

fn append_symbol_body_literal_bridge_leads(
    index: &Codebase,
    source_file: &FileEntry,
    body: &str,
    out: &mut String,
) {
    let leads = symbol_body_literal_bridge_leads(
        index,
        source_file,
        body,
        SYMBOL_BODY_LITERAL_BRIDGE_LEAD_LIMIT,
    );
    if leads.is_empty() {
        return;
    }
    out.push_str("body literal bridge leads:\n");
    for lead in leads {
        out.push_str(&format!(
            "  \"{}\" [{}] -> {}:{} ({}) // {}\n",
            compact_inline_text(&lead.literal, 80),
            lead.matched.into_iter().collect::<Vec<_>>().join("/"),
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            lead.target.detail
        ));
    }
}

fn symbol_body_literal_bridge_leads(
    index: &Codebase,
    source_file: &FileEntry,
    body: &str,
    limit: usize,
) -> Vec<BodyLiteralBridgeLead> {
    if limit == 0 {
        return Vec::new();
    }
    let mut seen_literals = BTreeSet::new();
    let literals = quoted_context_terms(body)
        .into_iter()
        .filter(|literal| {
            seen_literals.insert(literal.to_ascii_lowercase())
                && literal_looks_like_metadata_bridge(literal)
        })
        .take(SYMBOL_BODY_LITERAL_BRIDGE_LITERAL_LIMIT)
        .map(|literal| {
            let terms = identity_terms_from_text(&literal);
            (literal, terms)
        })
        .filter(|(_, terms)| terms.len() >= 2)
        .collect::<Vec<_>>();
    if literals.is_empty() {
        return Vec::new();
    }

    let mut leads = Vec::new();
    let mut seen_targets = BTreeSet::<(String, usize, String)>::new();
    for file in index.files.values() {
        if file.path == source_file.path || generic_source_path_score(&file.path) < 0.0 {
            continue;
        }
        let path_terms = identity_terms_from_text(&file.path);
        for (literal, literal_terms) in &literals {
            let query_terms = literal_terms.iter().cloned().collect::<Vec<_>>();
            let path_matches = matched_identity_terms(&query_terms, &path_terms)
                .into_iter()
                .collect::<BTreeSet<_>>();
            let mut best_symbol = None::<(&Symbol, BTreeSet<String>)>;
            for symbol in file
                .symbols
                .iter()
                .take(SYMBOL_BODY_LITERAL_BRIDGE_SYMBOL_SCAN_LIMIT)
                .filter(|symbol| is_context_handoff_source_symbol(symbol))
            {
                let symbol_terms = identity_terms_from_text(&symbol.name);
                let matches = matched_identity_terms(&query_terms, &symbol_terms)
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                let replace = best_symbol
                    .as_ref()
                    .is_none_or(|(_, current)| matches.len() > current.len());
                if replace {
                    best_symbol = Some((symbol, matches));
                }
            }
            let symbol_matches = best_symbol
                .as_ref()
                .map(|(_, matches)| matches.clone())
                .unwrap_or_default();
            let matched = path_matches
                .union(&symbol_matches)
                .cloned()
                .collect::<BTreeSet<_>>();
            if matched.len() < 2 {
                continue;
            }
            let Some(symbol) = best_symbol
                .as_ref()
                .map(|(symbol, _)| *symbol)
                .or_else(|| file.symbols.first())
            else {
                continue;
            };
            let target = target_from_symbol(file, symbol);
            if !seen_targets.insert((target.path.clone(), target.line_start, target.name.clone())) {
                continue;
            }
            let coverage = matched.len() as f32 / literal_terms.len().max(1) as f32;
            let score = matched.len() as f32 * 8.0
                + path_matches.len() as f32 * 4.0
                + symbol_matches.len() as f32 * 3.0
                + coverage * 4.0
                + literal_identifier_prefix_score(literal, file, symbol)
                + configured_scan_root_order_score(index, &file.path)
                + graph_connectivity_prior(index, &file.path).clamp(-4.0, 4.0);
            leads.push(BodyLiteralBridgeLead {
                literal: literal.clone(),
                matched,
                target,
                score,
            });
        }
    }
    leads.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    leads.truncate(limit);
    leads
}

fn literal_identifier_prefix_score(literal: &str, file: &FileEntry, symbol: &Symbol) -> f32 {
    let literal_sequences = literal
        .split('_')
        .filter(|segment| !segment.is_empty())
        .map(identifier_component_sequence)
        .filter(|sequence| sequence.len() >= 2)
        .collect::<Vec<_>>();
    if literal_sequences.is_empty() {
        return 0.0;
    }
    let mut target_sequences = raw_identifiers(&file.path)
        .into_iter()
        .map(|identifier| identifier_component_sequence(&identifier))
        .filter(|sequence| !sequence.is_empty())
        .collect::<Vec<_>>();
    target_sequences.push(identifier_component_sequence(&symbol.name));
    let mut best = 0.0f32;
    for (segment_index, literal_sequence) in literal_sequences.iter().enumerate() {
        for target_sequence in &target_sequences {
            let shared = literal_sequence
                .iter()
                .zip(target_sequence.iter())
                .take_while(|(left, right)| left == right)
                .count();
            if shared < 2 {
                continue;
            }
            let position_weight = 48.0 / (segment_index + 1) as f32;
            best = best.max(position_weight + shared.min(4) as f32 * 4.0);
        }
    }
    best
}

fn identifier_component_sequence(identifier: &str) -> Vec<String> {
    let parts = split_identifier(identifier);
    if parts.len() >= 2 {
        parts.into_iter().skip(1).collect()
    } else {
        parts
    }
}

fn literal_looks_like_metadata_bridge(literal: &str) -> bool {
    if literal.len() < 5 || literal.chars().any(char::is_whitespace) {
        return false;
    }
    literal
        .chars()
        .any(|ch| matches!(ch, '_' | '-' | '.' | '/' | '\\' | ':'))
        || (literal.chars().any(|ch| ch.is_ascii_uppercase())
            && literal.chars().any(|ch| ch.is_ascii_lowercase()))
        || literal.chars().any(|ch| ch.is_ascii_digit())
}

fn append_symbol_body_primary_handoff_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    evidence: &SymbolBodyPrimaryEvidence,
    out: &mut String,
) {
    append_symbol_body_qualified_tail_call_leads(&evidence.qualified, out);
    append_symbol_body_flow_handoff_leads(index, file, &evidence.flow, out);
    append_symbol_body_assignment_target_leads(index, file, symbol, body, out);
}

struct SymbolBodyPrimaryEvidence {
    qualified: Vec<BodyFlowHandoffLead>,
    flow: Vec<BodyFlowHandoffLead>,
}

fn symbol_body_primary_evidence(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
) -> SymbolBodyPrimaryEvidence {
    SymbolBodyPrimaryEvidence {
        qualified: symbol_body_qualified_tail_call_leads(
            index,
            file,
            symbol,
            body,
            SYMBOL_BODY_QUALIFIED_TAIL_CALL_LEAD_LIMIT,
        ),
        flow: symbol_body_flow_handoff_leads(
            index,
            file,
            symbol,
            body,
            SYMBOL_BODY_FLOW_HANDOFF_LEAD_LIMIT,
        ),
    }
}

fn append_symbol_body_evidence_card(
    file: &FileEntry,
    symbol: &Symbol,
    evidence: &SymbolBodyPrimaryEvidence,
    out: &mut String,
) {
    out.push_str(&format!(
        "exact body evidence: {}:L{}-L{} {} {} (complete active body follows)\n",
        file.path,
        symbol.line_start,
        symbol.line_end.max(symbol.line_start),
        symbol.kind,
        symbol.name
    ));
    out.push_str("closure: this complete body is exact local evidence. Direct calls prove handoffs only; use typed graph paths, argument bindings, dispatch edges, and control facts for cross-body semantics. A preprocessor guard applies only to the call edge carrying it.\n");
    let mut selected = evidence
        .qualified
        .iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    let mut flow_tail = evidence.flow.iter().collect::<Vec<_>>();
    flow_tail.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    if flow_tail.len() > 2 {
        flow_tail = flow_tail.split_off(flow_tail.len() - 2);
    }
    selected.extend(flow_tail);
    selected.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    selected.dedup_by(|left, right| {
        left.target.path == right.target.path
            && left.target.line_start == right.target.line_start
            && left.target.name == right.target.name
    });
    if !selected.is_empty() {
        out.push_str("next handoff candidates (source order; not a checklist):\n");
        for lead in selected {
            out.push_str(&format!(
                "  L{} {} -> {}:{} ({})\n",
                lead.line,
                lead.target.name,
                lead.target.path,
                lead.target.line_start,
                lead.target.kind
            ));
        }
    }
}

fn append_symbol_body_qualified_tail_call_leads(leads: &[BodyFlowHandoffLead], out: &mut String) {
    if leads.is_empty() {
        return;
    }
    out.push_str(
        "body qualified tail call leads (exact source-derived cross-file handoffs; follow before search/find):\n",
    );
    for lead in leads {
        out.push_str(&format!(
            "  L{} {} -> {}:{} ({}) // {}\n",
            lead.line,
            lead.target.name,
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            compact_inline_text(&lead.text, 180)
        ));
    }
}

fn symbol_body_qualified_tail_call_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    limit: usize,
) -> Vec<BodyFlowHandoffLead> {
    if limit == 0 {
        return Vec::new();
    }
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let receiver_types = qualified_receiver_type_hints(index, file, symbol, body);
    let mut by_target = BTreeMap::<(usize, String, usize, String), BodyFlowHandoffLead>::new();
    for (offset, line) in body.lines().enumerate() {
        let code_line = strip_strings_and_line_comment(line);
        for token in qualified_call_tokens(&code_line) {
            let receiver_type = qualified_token_receiver(&token)
                .and_then(|receiver| receiver_types.get(receiver))
                .map(String::as_str);
            let Some((target, score)) = resolve_qualified_call_target(
                index,
                file,
                symbol,
                &deps,
                &code_line,
                &token,
                true,
                receiver_type,
            ) else {
                continue;
            };
            let lead = BodyFlowHandoffLead {
                line: symbol.line_start + offset,
                text: line.trim().to_string(),
                score,
                target,
            };
            let key = (
                lead.line,
                lead.target.path.clone(),
                lead.target.line_start,
                lead.target.name.clone(),
            );
            by_target.insert(key, lead);
        }
    }
    let mut leads = by_target.into_values().collect::<Vec<_>>();
    leads.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    if leads.len() > limit {
        leads = leads.split_off(leads.len() - limit);
    }
    leads
}

fn qualified_call_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    for ch in line.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':') {
            token.push(ch);
            continue;
        }
        push_qualified_call_token(line, &token, &mut tokens);
        token.clear();
    }
    push_qualified_call_token(line, &token, &mut tokens);
    tokens
}

fn qualified_member_tokens(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut token = String::new();
    for ch in line.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':') {
            token.push(ch);
            continue;
        }
        push_qualified_member_token(line, &token, &mut tokens);
        token.clear();
    }
    push_qualified_member_token(line, &token, &mut tokens);
    tokens
}

fn push_qualified_member_token(line: &str, raw: &str, out: &mut Vec<String>) {
    let member_token = raw.trim_matches(|ch: char| ch == '.' || ch == ':');
    if member_token.len() < 3 {
        return;
    }
    let token = member_token.replace("::", ".");
    if !token.contains('.') {
        return;
    }
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() < 2 || parts.len() > 8 || !parts.iter().all(|part| is_identifier_segment(part)) {
        return;
    }
    let member = if line_contains_identifier_call(line, member_token) {
        if parts.len() <= 2 {
            return;
        }
        parts[..parts.len() - 1].join(".")
    } else {
        token
    };
    if member.contains('.') && !out.contains(&member) {
        out.push(member);
    }
}

fn push_qualified_call_token(line: &str, raw: &str, out: &mut Vec<String>) {
    let call_token = raw.trim_matches(|ch: char| ch == '.' || ch == ':');
    if call_token.len() < 5 || !line_contains_identifier_call(line, call_token) {
        return;
    }
    let token = call_token.replace("::", ".");
    if token.len() < 5 || !token.contains('.') {
        return;
    }
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() < 2 || parts.len() > 8 || !parts.iter().all(|part| is_identifier_segment(part)) {
        return;
    }
    if !out.contains(&token) {
        out.push(token);
    }
}

fn resolve_qualified_call_target(
    index: &Codebase,
    source_file: &FileEntry,
    source_symbol: &Symbol,
    deps: &BTreeSet<String>,
    line: &str,
    token: &str,
    allow_unique_without_qualifier: bool,
    receiver_type: Option<&str>,
) -> Option<(SymbolTarget, usize)> {
    let member = token.rsplit('.').next()?;
    let call_arity = call_argument_count(line, token);
    let candidates = index
        .symbols_named(member)
        .into_iter()
        .filter(|(candidate_file, candidate_symbol)| {
            is_context_handoff_source_symbol(candidate_symbol)
                && (candidate_file.path != source_file.path
                    || candidate_symbol.line_start != source_symbol.line_start
                    || candidate_symbol.name != source_symbol.name)
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return None;
    }
    let unique = candidates.len() == 1;
    let unique_fallback = allow_unique_without_qualifier
        && unique
        && qualified_token_has_graph_receiver(source_file, source_symbol, token);
    let mut candidates = candidates
        .into_iter()
        .filter(|(candidate_file, candidate_symbol)| {
            unique_fallback
                || qualified_call_qualifier_matches(index, token, candidate_file, candidate_symbol)
                    > 0
                || receiver_type.is_some_and(|receiver_type| {
                    qualified_call_receiver_type_matches(
                        receiver_type,
                        candidate_file,
                        candidate_symbol,
                    ) > 0
                })
        })
        .map(|(candidate_file, candidate_symbol)| {
            let score = qualified_call_candidate_score(
                index,
                token,
                source_file,
                deps,
                candidate_file,
                candidate_symbol,
                call_arity,
                receiver_type,
            );
            (score, target_from_symbol(candidate_file, candidate_symbol))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return None;
    }
    let unique = candidates.len() == 1;
    candidates.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.path.cmp(&right.1.path))
            .then_with(|| left.1.line_start.cmp(&right.1.line_start))
    });
    if !unique
        && candidates
            .get(1)
            .is_some_and(|second| second.0 == candidates[0].0)
    {
        return None;
    }
    candidates
        .into_iter()
        .next()
        .map(|(score, target)| (target, score))
}

fn qualified_token_has_graph_receiver(
    source_file: &FileEntry,
    source_symbol: &Symbol,
    token: &str,
) -> bool {
    let normalized = token.replace("::", ".");
    let receiver = normalized.split('.').next().unwrap_or_default();
    if receiver.is_empty() {
        return false;
    }
    if matches!(receiver, "this" | "base" | "self" | "super")
        || receiver.starts_with('_')
        || receiver
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
    {
        return true;
    }
    if raw_identifiers(&source_symbol.detail)
        .into_iter()
        .any(|identifier| identifier == receiver)
    {
        return true;
    }
    source_file
        .symbols
        .iter()
        .any(|symbol| symbol.name == receiver)
}

fn qualified_call_qualifier_matches(
    index: &Codebase,
    token: &str,
    candidate_file: &FileEntry,
    candidate_symbol: &Symbol,
) -> usize {
    let qualifiers = token
        .split('.')
        .take(token.split('.').count().saturating_sub(1))
        .map(|part| (part.to_string(), part.to_ascii_lowercase()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|(_, lower)| lower.len() >= 2)
        .collect::<Vec<_>>();
    let enclosing = candidate_file
        .symbols
        .iter()
        .filter(|symbol| {
            matches!(
                symbol.kind.as_str(),
                "class" | "interface" | "struct" | "record" | "trait" | "impl" | "module"
            ) && symbol.line_start <= candidate_symbol.line_start
                && candidate_symbol.line_start <= symbol.line_end
        })
        .min_by_key(|symbol| symbol.line_end.saturating_sub(symbol.line_start));
    let mut matched = BTreeSet::<String>::new();
    if let Some(enclosing) = enclosing {
        let enclosing_name = enclosing.name.to_ascii_lowercase();
        for (_, qualifier) in &qualifiers {
            if enclosing_name == *qualifier
                || (qualifier.len() >= 3
                    && (enclosing_name.starts_with(qualifier)
                        || enclosing_name.ends_with(qualifier)))
            {
                matched.insert(qualifier.clone());
            }
        }
    }
    for (raw, qualifier) in &qualifiers {
        if matched.contains(qualifier) {
            continue;
        }
        if index
            .symbols_named(raw)
            .into_iter()
            .any(|(file, _)| file.path == candidate_file.path)
        {
            matched.insert(qualifier.clone());
        }
    }
    matched.len()
}

fn qualified_call_candidate_score(
    index: &Codebase,
    token: &str,
    source_file: &FileEntry,
    deps: &BTreeSet<String>,
    candidate_file: &FileEntry,
    candidate_symbol: &Symbol,
    call_arity: Option<usize>,
    receiver_type: Option<&str>,
) -> usize {
    let qualifier_matches =
        qualified_call_qualifier_matches(index, token, candidate_file, candidate_symbol);
    let receiver_type_matches = receiver_type
        .map(|receiver_type| {
            qualified_call_receiver_type_matches(receiver_type, candidate_file, candidate_symbol)
        })
        .unwrap_or_default();
    let arity_bonus = match (
        call_arity,
        signature_parameter_count(&candidate_symbol.detail),
    ) {
        (Some(call_arity), Some(parameter_count)) if call_arity == parameter_count => 90,
        _ => 0,
    };
    receiver_type_matches * 120
        + qualifier_matches * 40
        + usize::from(deps.contains(&candidate_file.path)) * 30
        + usize::from(same_path_family(&source_file.path, &candidate_file.path)) * 15
        + symbol_kind_lead_weight(candidate_symbol)
        + arity_bonus
}

fn qualified_token_receiver(token: &str) -> Option<&str> {
    let normalized = token.strip_prefix("::").unwrap_or(token);
    normalized.split(['.', ':']).find(|part| !part.is_empty())
}

fn qualified_receiver_type_hints(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
) -> BTreeMap<String, String> {
    let receivers = body
        .lines()
        .flat_map(|line| {
            let code_line = strip_strings_and_line_comment(line);
            qualified_call_tokens(&code_line)
                .into_iter()
                .chain(qualified_member_tokens(&code_line))
                .filter_map(|token| qualified_token_receiver(&token).map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|receiver| !matches!(receiver.as_str(), "this" | "base" | "self" | "super"))
        .collect::<BTreeSet<_>>();
    if receivers.is_empty() {
        return BTreeMap::new();
    }
    let Ok(content) = index.file_content(file) else {
        return BTreeMap::new();
    };
    receivers
        .into_iter()
        .filter_map(|receiver| {
            declared_receiver_type(index, &content, symbol, &receiver)
                .map(|receiver_type| (receiver, receiver_type))
        })
        .collect()
}

fn declared_receiver_type(
    index: &Codebase,
    content: &str,
    source_symbol: &Symbol,
    receiver: &str,
) -> Option<String> {
    let mut found = None;
    for line in content
        .lines()
        .take(source_symbol.line_end.max(source_symbol.line_start))
    {
        let code_line = strip_strings_and_line_comment(line);
        let mut from = 0usize;
        while let Some(relative) = code_line.get(from..).and_then(|tail| tail.find(receiver)) {
            let start = from + relative;
            let end = start + receiver.len();
            let before = code_line
                .get(..start)
                .and_then(|prefix| prefix.chars().next_back());
            let after = code_line
                .get(end..)
                .and_then(|suffix| suffix.chars().next());
            if before.is_some_and(is_identifier_char)
                || after.is_some_and(is_identifier_char)
                || matches!(before, Some('.') | Some(':'))
            {
                from = end.max(from + 1);
                continue;
            }
            let prefix = code_line.get(..start).unwrap_or_default();
            let candidate = raw_identifiers(prefix).into_iter().next_back();
            if let Some(candidate) = candidate
                && candidate != receiver
                && index
                    .symbols_named(&candidate)
                    .into_iter()
                    .any(|(_, symbol)| {
                        matches!(
                            symbol.kind.as_str(),
                            "class" | "interface" | "struct" | "record" | "trait" | "type_alias"
                        )
                    })
            {
                found = Some(candidate);
            }
            from = end.max(from + 1);
        }
    }
    found
}

fn qualified_call_receiver_type_matches(
    receiver_type: &str,
    candidate_file: &FileEntry,
    candidate_symbol: &Symbol,
) -> usize {
    let Some(enclosing) = enclosing_type_symbol(candidate_file, candidate_symbol) else {
        return 0;
    };
    if enclosing.name == receiver_type {
        return 3;
    }
    usize::from(
        raw_identifiers(&enclosing.detail)
            .into_iter()
            .any(|identifier| identifier == receiver_type),
    )
}

fn enclosing_type_symbol<'a>(file: &'a FileEntry, symbol: &Symbol) -> Option<&'a Symbol> {
    file.symbols
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.kind.as_str(),
                "class" | "interface" | "struct" | "record" | "trait" | "impl" | "module"
            ) && candidate.line_start <= symbol.line_start
        })
        .max_by_key(|candidate| candidate.line_start)
}

fn call_argument_count(line: &str, token: &str) -> Option<usize> {
    let start = line.find(token)? + token.len();
    let suffix = line.get(start..)?.trim_start();
    let open = suffix.find('(')?;
    delimited_argument_count(suffix.get(open..)?)
}

fn identifier_call_argument_counts(line: &str, identifier: &str) -> BTreeSet<usize> {
    let mut counts = BTreeSet::new();
    let mut from = 0usize;
    while let Some(relative) = line.get(from..).and_then(|tail| tail.find(identifier)) {
        let start = from + relative;
        let end = start + identifier.len();
        let before_ok = line
            .get(..start)
            .and_then(|prefix| prefix.chars().next_back())
            .is_none_or(|ch| !is_identifier_char(ch));
        let after_ok = line
            .get(end..)
            .and_then(|suffix| suffix.chars().next())
            .is_none_or(|ch| !is_identifier_char(ch));
        if before_ok
            && after_ok
            && let Some(suffix) = line.get(end..)
        {
            let suffix = suffix.trim_start();
            if suffix.starts_with('(')
                && let Some(count) = delimited_argument_count(suffix)
            {
                counts.insert(count);
            }
        }
        from = end.max(from + 1);
    }
    counts
}

fn signature_parameter_count(detail: &str) -> Option<usize> {
    let open = detail.find('(')?;
    delimited_argument_count(detail.get(open..)?)
}

fn signature_accepts_argument_count(detail: &str, argument_count: usize) -> bool {
    let Some(parameters) = signature_parameters(detail) else {
        return false;
    };
    let variadic = parameters.iter().any(|parameter| {
        let trimmed = parameter.trim_start();
        trimmed.starts_with("params ") || trimmed.contains("...")
    });
    let required = parameters
        .iter()
        .filter(|parameter| {
            let trimmed = parameter.trim_start();
            !parameter.contains('=') && !trimmed.starts_with("params ") && !trimmed.contains("...")
        })
        .count();
    argument_count >= required && (variadic || argument_count <= parameters.len())
}

fn signature_parameters(detail: &str) -> Option<Vec<String>> {
    let open = detail.find('(')?;
    let suffix = detail.get(open + 1..)?;
    let mut paren_depth = 1usize;
    let mut bracket_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut current = String::new();
    let mut parameters = Vec::<String>::new();
    for ch in suffix.chars() {
        match ch {
            '(' => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                if paren_depth == 0 {
                    if !current.trim().is_empty() {
                        parameters.push(current.trim().to_string());
                    }
                    return Some(parameters);
                }
                current.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                current.push(ch);
            }
            ']' => {
                bracket_depth = bracket_depth.saturating_sub(1);
                current.push(ch);
            }
            '<' => {
                angle_depth += 1;
                current.push(ch);
            }
            '>' => {
                angle_depth = angle_depth.saturating_sub(1);
                current.push(ch);
            }
            '{' => {
                brace_depth += 1;
                current.push(ch);
            }
            '}' => {
                brace_depth = brace_depth.saturating_sub(1);
                current.push(ch);
            }
            ',' if paren_depth == 1
                && bracket_depth == 0
                && angle_depth == 0
                && brace_depth == 0 =>
            {
                parameters.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    None
}

fn delimited_argument_count(value: &str) -> Option<usize> {
    let mut chars = value.chars();
    if chars.next()? != '(' {
        return None;
    }
    let mut paren_depth = 1usize;
    let mut bracket_depth = 0usize;
    let mut angle_depth = 0usize;
    let mut commas = 0usize;
    let mut has_content = false;
    for ch in chars {
        match ch {
            '(' => {
                paren_depth += 1;
                has_content = true;
            }
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                if paren_depth == 0 {
                    return Some(if has_content { commas + 1 } else { 0 });
                }
            }
            '[' => {
                bracket_depth += 1;
                has_content = true;
            }
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '<' => {
                angle_depth += 1;
                has_content = true;
            }
            '>' => angle_depth = angle_depth.saturating_sub(1),
            ',' if paren_depth == 1 && bracket_depth == 0 && angle_depth == 0 => {
                commas += 1;
            }
            ch if paren_depth == 1 && !ch.is_whitespace() => has_content = true,
            _ => {}
        }
    }
    None
}

fn is_short_flow_wrapper_body(body: &str) -> bool {
    let content_lines = body
        .lines()
        .filter(|line| !is_comment_or_blank(line))
        .count();
    content_lines <= SYMBOL_BODY_SHORT_WRAPPER_MAX_LINES
        && body.lines().any(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("return ") || trimmed == "return;" || line.contains("=>")
        })
}

fn append_symbol_body_data_type_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    out: &mut String,
) -> Result<()> {
    let leads =
        symbol_body_data_type_leads(index, file, symbol, body, SYMBOL_BODY_DATA_TYPE_LEAD_LIMIT);
    if leads.is_empty() {
        return Ok(());
    }
    out.push_str("body data/type leads:\n");
    for lead in &leads {
        out.push_str(&format!(
            "  #{} {} -> {}:{} ({})  // {}\n",
            lead.order,
            lead.target.name,
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            lead.target.detail
        ));
    }
    append_symbol_body_data_type_reference_leads(index, file, symbol, &leads, out)?;
    Ok(())
}

fn append_symbol_body_ordered_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    out: &mut String,
    include_previews: bool,
) -> Result<()> {
    let leads =
        symbol_body_ordered_leads(index, file, symbol, body, SYMBOL_BODY_ORDERED_LEAD_LIMIT);
    if leads.is_empty() {
        return Ok(());
    }
    out.push_str("body ordered direct leads:\n");
    for lead in &leads {
        out.push_str(&format!(
            "  #{} {} -> {}:{} ({})  // {}\n",
            lead.order,
            lead.target.name,
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            lead.target.detail
        ));
    }
    if include_previews {
        append_symbol_body_ordered_tail_previews(index, file, &leads, out)?;
    }
    Ok(())
}

fn append_symbol_body_flow_handoff_leads(
    index: &Codebase,
    file: &FileEntry,
    leads: &[BodyFlowHandoffLead],
    out: &mut String,
) {
    if leads.is_empty() {
        return;
    }
    out.push_str("body flow handoff leads:\n");
    for lead in leads {
        out.push_str(&format!(
            "  L{} {} -> {}:{} ({}) // {}\n",
            lead.line,
            lead.target.name,
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            compact_inline_text(&lead.text, 180)
        ));
    }
    let mut preview_candidates = leads
        .iter()
        .filter(|lead| {
            same_path_family(&lead.target.path, &file.path)
                && matches!(
                    lead.target.kind.as_str(),
                    "method" | "function" | "constructor" | "procedure" | "macro"
                )
        })
        .collect::<Vec<_>>();
    preview_candidates.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    let previews = if preview_candidates.len() <= SYMBOL_BODY_FLOW_HANDOFF_PREVIEW_LIMIT {
        preview_candidates
    } else {
        let mut selected = Vec::new();
        if let Some(first) = preview_candidates.first().copied() {
            selected.push(first);
        }
        if SYMBOL_BODY_FLOW_HANDOFF_PREVIEW_LIMIT > 1
            && let Some(last) = preview_candidates.last().copied()
            && !selected.iter().any(|lead| {
                lead.target.path == last.target.path
                    && lead.target.line_start == last.target.line_start
                    && lead.target.name == last.target.name
            })
        {
            selected.push(last);
        }
        selected
    };
    if previews.is_empty() {
        return;
    }
    out.push_str("body handoff previews:\n");
    for lead in previews {
        if let Some(snippet) = compact_symbol_target_snippet_limited(
            index,
            &lead.target,
            SYMBOL_BODY_FLOW_HANDOFF_PREVIEW_MAX_LINES,
            SYMBOL_BODY_FLOW_HANDOFF_PREVIEW_MAX_CHARS,
        ) {
            out.push_str(&format!(
                "  {} -> {}:{}  {}\n",
                lead.target.name, lead.target.path, lead.target.line_start, snippet
            ));
        }
    }
}

fn symbol_body_flow_handoff_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    limit: usize,
) -> Vec<BodyFlowHandoffLead> {
    if limit == 0 {
        return Vec::new();
    }
    let mut leads = Vec::<BodyFlowHandoffLead>::new();
    let mut seen_targets = BTreeSet::<(String, usize, String)>::new();
    for (offset, line) in body.lines().enumerate() {
        let line_score = flow_handoff_line_score(line);
        if line_score == 0 {
            continue;
        }
        let line_number = symbol.line_start + offset;
        let mut seen_names = BTreeSet::<String>::new();
        for identifier in raw_identifiers(line) {
            if identifier.len() < 3 || !seen_names.insert(identifier.clone()) {
                continue;
            }
            let direct_call = line_contains_identifier_call(line, &identifier);
            let value_handoff =
                line_score >= 35 && line_contains_identifier_token(line, &identifier);
            if !direct_call && !value_handoff {
                continue;
            }
            for (candidate_file, candidate_symbol) in index.symbols_named(&identifier) {
                if !same_path_family(&candidate_file.path, &file.path)
                    || !is_context_handoff_source_symbol(candidate_symbol)
                {
                    continue;
                }
                if candidate_symbol.line_start == symbol.line_start
                    && candidate_symbol.name == symbol.name
                {
                    continue;
                }
                let target = target_from_symbol(candidate_file, candidate_symbol);
                let key = (target.path.clone(), target.line_start, target.name.clone());
                if !seen_targets.insert(key) {
                    continue;
                }
                leads.push(BodyFlowHandoffLead {
                    line: line_number,
                    text: line.trim().to_string(),
                    score: line_score
                        + if direct_call { 28 } else { 0 }
                        + symbol_kind_lead_weight(candidate_symbol)
                        + symbol_name_specificity_weight(candidate_symbol),
                    target,
                });
            }
        }
    }
    leads.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    leads.truncate(limit);
    leads
}

fn flow_handoff_line_score(line: &str) -> usize {
    let trimmed = line.trim_start();
    let mut score = 0usize;
    if assignment_operator_position(line).is_some() {
        score += 70;
    }
    if trimmed.starts_with("return ") || trimmed == "return;" {
        score += 55;
    }
    if line.contains("=>") {
        score += 45;
    }
    if line_has_callable_handoff_shape(trimmed) {
        score += 35;
    }
    if line.contains('.') || line.contains("::") {
        score += 8;
    }
    score
}

fn line_has_callable_handoff_shape(trimmed: &str) -> bool {
    if !(trimmed.contains('(') && trimmed.contains(')')) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    !matches!(
        lower.split_whitespace().next().unwrap_or_default(),
        "if" | "for" | "foreach" | "while" | "switch" | "catch"
    )
}

fn line_contains_identifier_token(line: &str, identifier: &str) -> bool {
    let mut from = 0usize;
    while let Some(relative) = line[from..].find(identifier) {
        let start = from + relative;
        let end = start + identifier.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        if before.is_some_and(is_identifier_char) || after.is_some_and(is_identifier_char) {
            from = start + 1;
            continue;
        }
        return true;
    }
    false
}

fn line_contains_identifier_call(line: &str, identifier: &str) -> bool {
    let mut from = 0usize;
    while let Some(relative) = line[from..].find(identifier) {
        let start = from + relative;
        let end = start + identifier.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        if before.is_some_and(is_identifier_char) || after.is_some_and(is_identifier_char) {
            from = start + 1;
            continue;
        }
        let suffix = line[end..].trim_start();
        if suffix.starts_with('(') || suffix.starts_with('<') {
            return true;
        }
        from = end;
    }
    false
}

fn append_symbol_body_ordered_tail_previews(
    index: &Codebase,
    file: &FileEntry,
    leads: &[BodySymbolLead],
    out: &mut String,
) -> Result<()> {
    if SYMBOL_BODY_ORDERED_TAIL_PREVIEW_LIMIT == 0 {
        return Ok(());
    }
    let selected = symbol_body_ordered_tail_preview_leads(
        index,
        file,
        leads,
        SYMBOL_BODY_ORDERED_TAIL_PREVIEW_LIMIT,
    );
    if selected.is_empty() {
        return Ok(());
    }
    out.push_str("body ordered tail previews:\n");
    for lead in selected {
        if let Some(snippet) = compact_symbol_target_snippet_limited(
            index,
            &lead.target,
            SYMBOL_BODY_ORDERED_TAIL_PREVIEW_MAX_LINES,
            SYMBOL_BODY_ORDERED_TAIL_PREVIEW_MAX_CHARS,
        ) {
            out.push_str(&format!(
                "  {} -> {}:{}  {}\n",
                lead.target.name, lead.target.path, lead.target.line_start, snippet
            ));
        }
        append_symbol_body_ordered_tail_nested_previews(index, &lead.target, out)?;
    }
    Ok(())
}

fn append_symbol_body_data_type_reference_leads(
    index: &Codebase,
    source_file: &FileEntry,
    source_symbol: &Symbol,
    leads: &[BodySymbolLead],
    out: &mut String,
) -> Result<()> {
    let mut total_hits = 0usize;
    let mut scope_continuations = 0usize;
    let mut seen_hits = BTreeSet::<(String, usize, String)>::new();
    let mut section = String::new();
    let mut reference_leads = leads.iter().collect::<Vec<_>>();
    reference_leads.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    for lead in reference_leads {
        if total_hits >= SYMBOL_BODY_DATA_TYPE_REF_TOTAL_HITS {
            break;
        }
        let Some(target_file) = index.file(&lead.target.path) else {
            continue;
        };
        let Some(target_symbol) = symbol_for_target(target_file, &lead.target) else {
            continue;
        };
        let remaining = SYMBOL_BODY_DATA_TYPE_REF_TOTAL_HITS.saturating_sub(total_hits);
        let hits = body_data_type_reference_hits(
            index,
            source_file,
            source_symbol,
            target_file,
            target_symbol,
            SYMBOL_BODY_DATA_TYPE_REF_HITS_PER_LEAD.min(remaining),
        )?;
        if hits.is_empty() {
            continue;
        }
        if section.is_empty() {
            section.push_str("body data/type external refs:\n");
        }
        section.push_str(&format!("  {} references:\n", lead.target.name));
        for hit in hits {
            let hit_key = (hit.path.clone(), hit.line, lead.target.name.clone());
            if !seen_hits.insert(hit_key) {
                continue;
            }
            total_hits += 1;
            let scope = hit
                .scope
                .as_ref()
                .map(|scope| format!(" [in {} {}]", scope.kind, scope.name))
                .unwrap_or_default();
            section.push_str(&format!(
                "    {}:{}{}  {}\n",
                hit.path,
                hit.line,
                scope,
                compact_inline_text(&hit.text, 180)
            ));
            append_data_type_reference_scope_continuation_leads(
                index,
                &hit,
                &source_file.path,
                &lead.target,
                &mut scope_continuations,
                &mut section,
            )?;
            if total_hits >= SYMBOL_BODY_DATA_TYPE_REF_TOTAL_HITS {
                break;
            }
        }
    }
    if !section.is_empty() {
        out.push_str(&section);
    }
    Ok(())
}

fn append_data_type_reference_scope_continuation_leads(
    index: &Codebase,
    hit: &SearchHit,
    source_path: &str,
    referenced: &SymbolTarget,
    emitted: &mut usize,
    out: &mut String,
) -> Result<()> {
    if *emitted >= SYMBOL_BODY_DATA_TYPE_REF_SCOPE_LEAD_LIMIT {
        return Ok(());
    }
    let Some(scope) = hit.scope.as_ref() else {
        return Ok(());
    };
    let span = scope.end.saturating_sub(scope.start) + 1;
    if span > SYMBOL_BODY_DATA_TYPE_REF_SCOPE_MAX_LINES {
        return Ok(());
    }
    let Some(file) = index.file(&hit.path) else {
        return Ok(());
    };
    if hit.path != source_path && !data_type_reference_scope_path_allowed(index, source_path, file)
    {
        return Ok(());
    }
    let fallback_symbol;
    let symbol = if let Some(symbol) = symbol_for_scope(file, scope) {
        symbol
    } else {
        fallback_symbol = Symbol {
            name: scope.name.clone(),
            kind: SymbolKind::from(scope.kind.as_str()),
            line_start: scope.start,
            line_end: scope.end,
            detail: String::new(),
        };
        &fallback_symbol
    };
    let symbol_end = symbol.line_end.max(symbol.line_start);
    if hit.line > symbol_end {
        return Ok(());
    }
    let content = index.file_content(file)?;
    let tail_body = source_line_slice(&content, hit.line, symbol_end);
    let mut leads = symbol_body_data_type_leads(
        index,
        file,
        symbol,
        &tail_body,
        SYMBOL_BODY_DATA_TYPE_REF_SCOPE_DATA_LEADS + 2,
    );
    leads.retain(|lead| {
        lead.target.path != referenced.path
            || lead.target.line_start != referenced.line_start
            || lead.target.name != referenced.name
    });
    leads.truncate(SYMBOL_BODY_DATA_TYPE_REF_SCOPE_DATA_LEADS);
    if leads.is_empty() {
        return Ok(());
    }
    out.push_str(&format!(
        "      reference scope continuation leads {}:{} {} after {}:\n",
        file.path, symbol.line_start, symbol.name, referenced.name
    ));
    for lead in leads {
        out.push_str(&format!(
            "        {} -> {}:{} ({})  // {}\n",
            lead.target.name,
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            lead.target.detail
        ));
    }
    *emitted += 1;
    Ok(())
}

fn data_type_reference_scope_path_allowed(
    index: &Codebase,
    source_path: &str,
    target_file: &FileEntry,
) -> bool {
    if target_file.path == source_path {
        return true;
    }
    if same_parent_path(source_path, &target_file.path)
        || same_path_family(source_path, &target_file.path)
    {
        return true;
    }
    let source_deps = index
        .deps_for(source_path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    if source_deps.contains(&target_file.path) {
        return true;
    }
    let source_reverse_deps = index
        .reverse_deps_for(source_path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    source_reverse_deps.contains(&target_file.path)
}

fn symbol_body_ordered_tail_preview_leads(
    index: &Codebase,
    file: &FileEntry,
    leads: &[BodySymbolLead],
    limit: usize,
) -> Vec<BodySymbolLead> {
    if limit == 0 {
        return Vec::new();
    }
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let candidates = leads
        .iter()
        .filter(|lead| is_previewable_symbol_kind(&lead.target.kind))
        .filter(|lead| generic_source_path_score(&lead.target.path) >= 0.0)
        .cloned()
        .collect::<Vec<_>>();
    let mut selected = take_tail_preview_leads(
        candidates
            .iter()
            .filter(|lead| same_path_family(&file.path, &lead.target.path))
            .cloned()
            .collect(),
        limit,
    );
    if selected.len() < limit {
        let used = selected
            .iter()
            .map(|lead| {
                (
                    lead.target.path.clone(),
                    lead.target.line_start,
                    lead.target.name.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        let remaining = limit.saturating_sub(selected.len());
        selected.extend(take_tail_preview_leads(
            candidates
                .into_iter()
                .filter(|lead| deps.contains(&lead.target.path))
                .filter(|lead| {
                    !used.contains(&(
                        lead.target.path.clone(),
                        lead.target.line_start,
                        lead.target.name.clone(),
                    ))
                })
                .collect(),
            remaining,
        ));
    }
    selected
}

fn take_tail_preview_leads(
    mut candidates: Vec<BodySymbolLead>,
    limit: usize,
) -> Vec<BodySymbolLead> {
    if candidates.len() > limit {
        candidates = candidates[candidates.len().saturating_sub(limit)..].to_vec();
    }
    candidates
}

fn append_symbol_body_ordered_tail_nested_previews(
    index: &Codebase,
    target: &SymbolTarget,
    out: &mut String,
) -> Result<()> {
    let Some(file) = index.file(&target.path) else {
        return Ok(());
    };
    let Some(symbol) = symbol_for_target(file, target) else {
        return Ok(());
    };
    let symbol_end = symbol.line_end.max(symbol.line_start);
    let span = symbol_end.saturating_sub(symbol.line_start) + 1;
    if span > CONTEXT_SYMBOL_HANDOFF_MAX_SOURCE_LINES {
        return Ok(());
    }
    let content = index.file_content(file)?;
    let body = source_line_slice(&content, symbol.line_start, symbol_end);
    let leads =
        symbol_body_ordered_leads(index, file, symbol, &body, SYMBOL_BODY_ORDERED_LEAD_LIMIT);
    let selected = symbol_body_ordered_tail_preview_leads(
        index,
        file,
        &leads,
        SYMBOL_BODY_ORDERED_TAIL_NESTED_PREVIEW_LIMIT,
    );
    for nested in selected {
        if nested.target.path == target.path
            && nested.target.line_start == target.line_start
            && nested.target.name == target.name
        {
            continue;
        }
        if let Some(snippet) = compact_symbol_target_snippet_limited(
            index,
            &nested.target,
            SYMBOL_BODY_ORDERED_TAIL_NESTED_PREVIEW_MAX_LINES,
            SYMBOL_BODY_ORDERED_TAIL_NESTED_PREVIEW_MAX_CHARS,
        ) {
            out.push_str(&format!(
                "    tail nested {} -> {}:{}  {}\n",
                nested.target.name, nested.target.path, nested.target.line_start, snippet
            ));
        }
        append_symbol_body_ordered_tail_grandchild_previews(index, &nested.target, out)?;
    }
    Ok(())
}

fn append_symbol_body_ordered_tail_grandchild_previews(
    index: &Codebase,
    target: &SymbolTarget,
    out: &mut String,
) -> Result<()> {
    let Some(file) = index.file(&target.path) else {
        return Ok(());
    };
    let Some(symbol) = symbol_for_target(file, target) else {
        return Ok(());
    };
    let symbol_end = symbol.line_end.max(symbol.line_start);
    let span = symbol_end.saturating_sub(symbol.line_start) + 1;
    if span > CONTEXT_SYMBOL_HANDOFF_MAX_SOURCE_LINES {
        return Ok(());
    }
    let content = index.file_content(file)?;
    let body = source_line_slice(&content, symbol.line_start, symbol_end);
    let leads =
        symbol_body_ordered_leads(index, file, symbol, &body, SYMBOL_BODY_ORDERED_LEAD_LIMIT);
    let selected = symbol_body_ordered_tail_preview_leads(
        index,
        file,
        &leads,
        SYMBOL_BODY_ORDERED_TAIL_GRANDCHILD_PREVIEW_LIMIT,
    );
    for nested in selected {
        if nested.target.path == target.path
            && nested.target.line_start == target.line_start
            && nested.target.name == target.name
        {
            continue;
        }
        if let Some(snippet) = compact_symbol_target_snippet_limited(
            index,
            &nested.target,
            SYMBOL_BODY_ORDERED_TAIL_GRANDCHILD_PREVIEW_MAX_LINES,
            SYMBOL_BODY_ORDERED_TAIL_GRANDCHILD_PREVIEW_MAX_CHARS,
        ) {
            out.push_str(&format!(
                "      tail nested next {} -> {}:{}  {}\n",
                nested.target.name, nested.target.path, nested.target.line_start, snippet
            ));
        }
    }
    Ok(())
}

fn append_symbol_body_lead_previews(
    index: &Codebase,
    leads: &[BodySymbolLead],
    out: &mut String,
) -> Result<()> {
    let preview_targets = leads
        .iter()
        .filter(|lead| is_previewable_symbol_kind(&lead.target.kind))
        .take(SYMBOL_BODY_LEAD_PREVIEW_LIMIT)
        .collect::<Vec<_>>();
    if preview_targets.is_empty() {
        return Ok(());
    }
    out.push_str("body lead previews:\n");
    for lead in preview_targets {
        if let Some(snippet) = compact_symbol_target_snippet_limited(
            index,
            &lead.target,
            SYMBOL_BODY_LEAD_PREVIEW_MAX_LINES,
            SYMBOL_BODY_LEAD_PREVIEW_MAX_CHARS,
        ) {
            out.push_str(&format!(
                "  {} -> {}:{}  {}\n",
                lead.target.name, lead.target.path, lead.target.line_start, snippet
            ));
        }
        append_symbol_body_lead_nested_previews(index, &lead.target, out)?;
    }
    Ok(())
}

fn append_symbol_body_lead_nested_previews(
    index: &Codebase,
    target: &SymbolTarget,
    out: &mut String,
) -> Result<()> {
    if SYMBOL_BODY_LEAD_NESTED_PREVIEW_LIMIT == 0 {
        return Ok(());
    }
    let mut seen = BTreeSet::new();
    seen.insert(symbol_target_key(target));
    append_symbol_body_lead_nested_previews_inner(
        index,
        target,
        out,
        SYMBOL_BODY_LEAD_NESTED_PREVIEW_DEPTH,
        1,
        &mut seen,
    )
}

fn append_symbol_body_lead_nested_previews_inner(
    index: &Codebase,
    target: &SymbolTarget,
    out: &mut String,
    depth_remaining: usize,
    depth: usize,
    seen: &mut BTreeSet<String>,
) -> Result<()> {
    if depth_remaining == 0 {
        return Ok(());
    }
    let Some(file) = index.file(&target.path) else {
        return Ok(());
    };
    let Some(symbol) = symbol_for_target(file, target) else {
        return Ok(());
    };
    let content = index.file_content(file)?;
    let symbol_end = symbol.line_end.max(symbol.line_start);
    let body = source_line_slice(&content, symbol.line_start, symbol_end);
    let nested = symbol_body_expansion_leads(
        index,
        file,
        symbol,
        &body,
        SYMBOL_BODY_LEAD_NESTED_PREVIEW_LIMIT,
        &[],
    );
    for nested_lead in nested
        .into_iter()
        .filter(|lead| is_previewable_symbol_kind(&lead.target.kind))
    {
        if !seen.insert(symbol_target_key(&nested_lead.target)) {
            continue;
        }
        if let Some(snippet) = compact_symbol_target_snippet_limited(
            index,
            &nested_lead.target,
            SYMBOL_BODY_LEAD_NESTED_PREVIEW_MAX_LINES,
            SYMBOL_BODY_LEAD_NESTED_PREVIEW_MAX_CHARS,
        ) {
            out.push_str(&format!(
                "{}nested {} -> {}:{}  {}\n",
                "  ".repeat(depth + 1),
                nested_lead.target.name,
                nested_lead.target.path,
                nested_lead.target.line_start,
                snippet
            ));
        }
        append_symbol_body_lead_nested_previews_inner(
            index,
            &nested_lead.target,
            out,
            depth_remaining.saturating_sub(1),
            depth + 1,
            seen,
        )?;
    }
    Ok(())
}

fn is_previewable_symbol_kind(kind: &str) -> bool {
    matches!(kind, "function" | "method" | "property")
}

fn append_same_file_callee_exact_reference_leads(
    index: &Codebase,
    file: &FileEntry,
    leads: &[BodySymbolLead],
    out: &mut String,
) -> Result<()> {
    let total_limit = SYMBOL_BODY_CALLEE_EXACT_REF_TOTAL_HITS;
    if total_limit == 0 {
        return Ok(());
    }
    let content = index.file_content(file)?;
    let mut total_hits = 0usize;
    let mut scope_leads = 0usize;
    let mut seen_terms = BTreeSet::<String>::new();
    let mut seen_hits = BTreeSet::<(String, usize, String)>::new();
    let mut section = String::new();

    for lead in leads
        .iter()
        .filter(|lead| lead.target.path == file.path)
        .take(SYMBOL_BODY_CALLEE_EXACT_REF_SYMBOL_LIMIT)
    {
        if total_hits == total_limit {
            break;
        }
        let Some(callee) = symbol_for_target(file, &lead.target) else {
            continue;
        };
        let callee_end = callee.line_end.max(callee.line_start);
        let span = callee_end.saturating_sub(callee.line_start) + 1;
        if span > SYMBOL_BODY_CALLEE_EXACT_REF_MAX_LINES {
            continue;
        }
        let callee_body = source_line_slice(&content, callee.line_start, callee_end);
        let terms =
            body_exact_reference_terms(&callee_body, SYMBOL_BODY_CALLEE_EXACT_REF_TERM_LIMIT);
        for term in terms {
            if total_hits == total_limit {
                break;
            }
            if !seen_terms.insert(term.clone()) {
                continue;
            }
            let mut hits = index.text_line_hits(
                &term,
                SYMBOL_BODY_CALLEE_EXACT_REF_HITS_PER_TERM * 8,
                false,
                None,
                true,
                true,
            )?;
            hits.retain(|hit| {
                hit.path != file.path || hit.line < callee.line_start || hit.line > callee_end
            });
            if hits.is_empty() {
                continue;
            }
            hits.truncate(
                SYMBOL_BODY_CALLEE_EXACT_REF_HITS_PER_TERM
                    .min(total_limit.saturating_sub(total_hits)),
            );
            if section.is_empty() {
                section.push_str("same-file callee exact reference leads (exact member/path-shaped terms from small same-file callees reached by this body):\n");
            }
            section.push_str(&format!("  {} -> {term}\n", lead.target.name));
            for hit in hits {
                let hit_key = (hit.path.clone(), hit.line, term.clone());
                if !seen_hits.insert(hit_key) {
                    continue;
                }
                total_hits += 1;
                let scope = hit
                    .scope
                    .as_ref()
                    .map(|scope| format!(" [in {} {}]", scope.kind, scope.name))
                    .unwrap_or_default();
                section.push_str(&format!(
                    "    {}:{}{}  {}\n",
                    hit.path,
                    hit.line,
                    scope,
                    compact_inline_text(&hit.text, 160)
                ));
                append_exact_reference_hit_scope_leads(
                    index,
                    &hit,
                    &mut scope_leads,
                    &mut section,
                )?;
            }
        }
    }

    out.push_str(&section);
    Ok(())
}

fn append_symbol_body_exact_reference_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    out: &mut String,
) -> Result<()> {
    let total_limit = SYMBOL_BODY_EXACT_REF_TOTAL_HITS;
    if total_limit == 0 {
        return Ok(());
    }
    let terms = body_exact_reference_terms(body, SYMBOL_BODY_EXACT_REF_TERM_LIMIT);
    if terms.is_empty() {
        return Ok(());
    }
    let mut total_hits = 0usize;
    let mut scope_leads = 0usize;
    let mut section = String::new();
    for term in terms {
        if total_hits == total_limit {
            break;
        }
        let mut hits = index.text_line_hits(
            &term,
            SYMBOL_BODY_EXACT_REF_HITS_PER_TERM * 8,
            false,
            None,
            true,
            true,
        )?;
        hits.retain(|hit| {
            hit.path != file.path
                || hit.line < symbol.line_start
                || hit.line > symbol.line_end.max(symbol.line_start)
        });
        if hits.is_empty() {
            continue;
        }
        hits.truncate(
            SYMBOL_BODY_EXACT_REF_HITS_PER_TERM.min(total_limit.saturating_sub(total_hits)),
        );
        if section.is_empty() {
            section.push_str("body exact reference leads (exact member/path-shaped terms from this body; use as downstream/upstream handles):\n");
        }
        section.push_str(&format!("  {term}\n"));
        for hit in hits {
            total_hits += 1;
            let scope = hit
                .scope
                .as_ref()
                .map(|scope| format!(" [in {} {}]", scope.kind, scope.name))
                .unwrap_or_default();
            section.push_str(&format!(
                "    {}:{}{}  {}\n",
                hit.path,
                hit.line,
                scope,
                compact_inline_text(&hit.text, 160)
            ));
            append_exact_reference_hit_scope_leads(index, &hit, &mut scope_leads, &mut section)?;
        }
    }
    out.push_str(&section);
    Ok(())
}

fn append_exact_reference_hit_scope_leads(
    index: &Codebase,
    hit: &SearchHit,
    emitted: &mut usize,
    out: &mut String,
) -> Result<()> {
    if *emitted >= SYMBOL_BODY_EXACT_REF_SCOPE_LEAD_LIMIT {
        return Ok(());
    }
    let Some(scope) = hit.scope.as_ref() else {
        return Ok(());
    };
    let span = scope.end.saturating_sub(scope.start) + 1;
    if span > SYMBOL_BODY_EXACT_REF_SCOPE_MAX_LINES {
        return Ok(());
    }
    let Some(file) = index.file(&hit.path) else {
        return Ok(());
    };
    let Some(symbol) = symbol_for_scope(file, scope) else {
        return Ok(());
    };
    let content = fs::read_to_string(index.root.join(&file.path))?;
    let body = source_line_slice(
        &content,
        symbol.line_start,
        symbol.line_end.max(symbol.line_start),
    );
    let leads = symbol_body_leads(
        index,
        file,
        symbol,
        &body,
        SYMBOL_BODY_EXACT_REF_SCOPE_LEADS_PER_HIT,
    );
    if leads.is_empty() {
        return Ok(());
    }
    out.push_str(&format!(
        "      enclosing scope leads for {}:{} {}:\n",
        file.path, symbol.line_start, symbol.name
    ));
    for lead in leads {
        out.push_str(&format!(
            "        {} -> {}:{} ({})  // {}\n",
            lead.target.name,
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            lead.target.detail
        ));
    }
    append_symbol_incoming_reference_leads(
        index,
        file,
        symbol,
        out,
        "      ",
        SYMBOL_BODY_INCOMING_REF_LIMIT,
    )?;
    *emitted += 1;
    Ok(())
}

fn append_symbol_incoming_reference_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    out: &mut String,
    indent: &str,
    limit: usize,
) -> Result<()> {
    let hits = symbol_incoming_reference_hits(index, file, symbol, limit)?;
    if hits.is_empty() {
        return Ok(());
    }
    out.push_str(&format!("{indent}body incoming refs:\n"));
    for hit in hits {
        let scope = hit
            .scope
            .as_ref()
            .map(|scope| format!(" [in {} {}]", scope.kind, scope.name))
            .unwrap_or_default();
        out.push_str(&format!(
            "{indent}  {}:{}{}  {}\n",
            hit.path,
            hit.line,
            scope,
            compact_inline_text(&hit.text, 180)
        ));
        append_incoming_reference_scope_terms(index, &hit, indent, out)?;
    }
    Ok(())
}

fn symbol_incoming_reference_hits(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    if limit == 0 || symbol.name.len() < 3 || symbol.name.starts_with("__") {
        return Ok(Vec::new());
    }
    let all_definition_sites = index
        .symbols_named(&symbol.name)
        .into_iter()
        .map(|(definition_file, definition_symbol)| {
            (definition_file.path.clone(), definition_symbol.line_start)
        })
        .collect::<BTreeSet<_>>();
    let mut hits = reference_candidates_with_limit(
        index,
        &symbol.name,
        Some(SYMBOL_BODY_INCOMING_REF_MAX_WORD_HITS),
    )?;
    hits.retain(|hit| {
        if all_definition_sites.contains(&(hit.path.clone(), hit.line)) {
            return false;
        }
        if hit.path == file.path && hit.line >= symbol.line_start && hit.line <= symbol.line_end {
            return false;
        }
        true
    });
    hits.sort_by(|left, right| {
        incoming_reference_hit_score(index, file, symbol, right)
            .cmp(&incoming_reference_hit_score(index, file, symbol, left))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn incoming_reference_hit_score(
    index: &Codebase,
    source_file: &FileEntry,
    source_symbol: &Symbol,
    hit: &SearchHit,
) -> usize {
    let mut score = 0usize;
    if hit.path != source_file.path {
        score += 30;
    } else {
        score += 10;
    }
    if let Some(scope) = &hit.scope {
        score += match scope.kind.as_str() {
            "method" | "function" | "constructor" => 40,
            "class" | "interface" | "struct" | "enum" | "record" => 12,
            _ => 6,
        };
        if scope.name != source_symbol.name {
            score += symbol_name_specificity_weight_from_name(&scope.name).min(24);
        }
    }
    if hit.text.contains('(') {
        score += 12;
    }
    if hit.text.contains('.') || hit.text.contains("::") {
        score += 8;
    }
    if generic_source_path_score(&hit.path) >= 0.0 {
        score += 10;
    }
    let degree = file_graph_degree(index, &hit.path);
    if degree <= 120 {
        score += 10;
    }
    score
}

fn append_incoming_reference_scope_terms(
    index: &Codebase,
    hit: &SearchHit,
    indent: &str,
    out: &mut String,
) -> Result<()> {
    let Some(scope) = hit.scope.as_ref() else {
        return Ok(());
    };
    let span = scope.end.saturating_sub(scope.start) + 1;
    if span > SYMBOL_BODY_INCOMING_SCOPE_MAX_LINES {
        return Ok(());
    }
    let Some(file) = index.file(&hit.path) else {
        return Ok(());
    };
    let Some(symbol) = symbol_for_scope(file, scope) else {
        return Ok(());
    };
    let content = index.file_content(file)?;
    let body = source_line_slice(
        &content,
        symbol.line_start,
        symbol.line_end.max(symbol.line_start),
    );
    let terms = body_exact_reference_terms(&body, SYMBOL_BODY_INCOMING_SCOPE_TERM_LIMIT);
    if !terms.is_empty() {
        out.push_str(&format!(
            "{indent}    incoming scope exact terms: {}\n",
            terms.join(", ")
        ));
    }
    Ok(())
}

fn symbol_for_scope<'a>(file: &'a FileEntry, scope: &crate::types::Scope) -> Option<&'a Symbol> {
    file.symbols
        .iter()
        .find(|symbol| {
            symbol.name == scope.name
                && symbol.kind.as_str() == scope.kind
                && symbol.line_start == scope.start
        })
        .or_else(|| {
            file.symbols.iter().find(|symbol| {
                symbol.name == scope.name
                    && symbol.line_start <= scope.start
                    && symbol.line_end.max(symbol.line_start) >= scope.end
            })
        })
}

fn body_exact_reference_terms(body: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let mut terms = BTreeSet::<String>::new();
    let mut token = String::new();
    for ch in body.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == ':' {
            token.push(ch);
        } else {
            add_body_exact_reference_token(&mut terms, &token);
            token.clear();
        }
    }
    add_body_exact_reference_token(&mut terms, &token);

    let mut ranked = terms
        .into_iter()
        .map(|term| (body_exact_reference_term_score(&term), term))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, term)| term)
        .collect()
}

fn add_body_exact_reference_token(out: &mut BTreeSet<String>, token: &str) {
    let token = token
        .trim_matches(|ch: char| ch == '.' || ch == ':')
        .replace("::", ".");
    if token.len() < 5 || token.len() > 96 || !token.contains('.') {
        return;
    }
    let parts = token.split('.').collect::<Vec<_>>();
    if parts.len() < 2 || parts.len() > 5 || !parts.iter().all(|part| is_identifier_segment(part)) {
        return;
    }
    out.insert(token);
}

fn is_identifier_segment(part: &str) -> bool {
    let mut chars = part.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn body_exact_reference_term_score(term: &str) -> usize {
    let parts = term.split('.').collect::<Vec<_>>();
    let uppercase = term
        .chars()
        .filter(|ch| ch.is_ascii_uppercase())
        .count()
        .min(12);
    let long_parts = parts.iter().filter(|part| part.len() >= 4).count();
    let last_len = parts
        .last()
        .map(|part| part.len())
        .unwrap_or_default()
        .min(16);
    uppercase * 4 + long_parts * 5 + parts.len() * 3 + last_len
}

fn compact_inline_text(text: &str, max_chars: usize) -> String {
    let mut compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let mut clipped = compact
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    clipped.push_str("...");
    compact = clipped;
    compact
}

fn compact_inline_text_with_tail(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let total_chars = compact.chars().count();
    if total_chars <= max_chars {
        return compact;
    }
    let separator = " ... ";
    let separator_chars = separator.chars().count();
    if max_chars <= separator_chars + 2 {
        return compact_inline_text(text, max_chars);
    }
    let available = max_chars.saturating_sub(separator_chars);
    let head_chars = (available + 1) / 2;
    let tail_chars = available / 2;
    let head = compact.chars().take(head_chars).collect::<String>();
    let tail = compact
        .chars()
        .skip(total_chars.saturating_sub(tail_chars))
        .collect::<String>();
    format!("{head}{separator}{tail}")
}

fn quoted_context_terms(text: &str) -> Vec<String> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut terms = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        let quote = chars[index];
        if !matches!(quote, '\'' | '"' | '`') {
            index += 1;
            continue;
        }
        index += 1;
        let mut value = String::new();
        let mut escaped = false;
        while index < chars.len() {
            let current = chars[index];
            index += 1;
            if escaped {
                value.push(current);
                escaped = false;
                continue;
            }
            if current == '\\' {
                escaped = true;
                continue;
            }
            if current == quote {
                break;
            }
            value.push(current);
        }
        let value = value.trim();
        if (2..=256).contains(&value.len()) {
            terms.push(value.to_string());
        }
    }
    terms
}

fn looks_like_context_identifier(value: &str) -> bool {
    value.len() >= 3
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && (value.contains('_')
            || value.chars().any(|ch| ch.is_ascii_uppercase())
            || value.chars().any(|ch| ch.is_ascii_digit()))
}

fn append_symbol_body_assignment_target_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    out: &mut String,
) {
    let leads = symbol_body_assignment_target_leads(
        index,
        file,
        symbol,
        body,
        SYMBOL_BODY_ASSIGNMENT_TARGET_LEAD_LIMIT,
    );
    if leads.is_empty() {
        return;
    }
    out.push_str("body assignment target leads:\n");
    for lead in leads {
        out.push_str(&format!(
            "  L{} {} -> {}:{} ({})  // {}\n",
            lead.line,
            lead.target.name,
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            lead.target.detail
        ));
        out.push_str(&format!(
            "    source: {}\n",
            compact_inline_text(&lead.text, 160)
        ));
        out.push_str(&format!(
            "    followup: codedb_symbol name={} path={} body=true max_results=1\n",
            lead.target.name, lead.target.path
        ));
    }
}

fn symbol_body_assignment_target_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    limit: usize,
) -> Vec<BodyAssignmentTargetLead> {
    if limit == 0 {
        return Vec::new();
    }
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut seen_names = BTreeSet::new();
    let mut seen_targets = BTreeSet::new();
    let mut leads = Vec::new();
    for (offset, line) in body.lines().enumerate() {
        let Some(name) = assignment_target_identifier(line) else {
            continue;
        };
        if name.len() < 3 || name == symbol.name || !seen_names.insert(name.clone()) {
            continue;
        }
        let candidates = index
            .symbols_named(&name)
            .into_iter()
            .filter(|(candidate_file, candidate_symbol)| {
                candidate_file.path != file.path
                    || candidate_symbol.line_start != symbol.line_start
                    || candidate_symbol.name != symbol.name
            })
            .map(|(candidate_file, candidate_symbol)| {
                (
                    assignment_target_symbol_score(file, &deps, candidate_file, candidate_symbol),
                    target_from_symbol(candidate_file, candidate_symbol),
                )
            })
            .filter(|(score, _)| *score > 0)
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            continue;
        }
        let mut ranked = candidates;
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.path.cmp(&right.1.path))
                .then_with(|| left.1.line_start.cmp(&right.1.line_start))
        });
        let target = ranked.remove(0).1;
        if seen_targets.insert((target.path.clone(), target.line_start, target.name.clone())) {
            leads.push(BodyAssignmentTargetLead {
                order: offset,
                line: symbol.line_start + offset,
                text: line.trim().to_string(),
                target,
            });
        }
    }
    leads.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    leads.truncate(limit);
    leads
}

fn assignment_target_symbol_score(
    source_file: &FileEntry,
    deps: &BTreeSet<String>,
    candidate_file: &FileEntry,
    candidate_symbol: &Symbol,
) -> usize {
    let locality = if candidate_file.path == source_file.path {
        180
    } else if same_path_family(&source_file.path, &candidate_file.path) {
        160
    } else if deps.contains(&candidate_file.path) {
        90
    } else if same_parent_path(&source_file.path, &candidate_file.path) {
        45
    } else {
        0
    };
    if locality == 0 {
        return 0;
    }
    locality
        + assignment_target_kind_weight(candidate_symbol)
        + symbol_name_specificity_weight(candidate_symbol)
}

fn assignment_target_kind_weight(symbol: &Symbol) -> usize {
    match symbol.kind.as_str() {
        "property" | "field" | "variable" | "const" | "static" => 60,
        "method" | "function" => 12,
        _ => 4,
    }
}

fn assignment_target_identifier(line: &str) -> Option<String> {
    let assignment = assignment_operator_position(line)?;
    let mut prefix = line[..assignment].trim_end();
    while prefix.ends_with(']') {
        let open = prefix.rfind('[')?;
        prefix = prefix[..open].trim_end();
    }
    let mut end = prefix.len();
    while end > 0 && !prefix.is_char_boundary(end) {
        end -= 1;
    }
    let mut start = end;
    for (idx, ch) in prefix[..end].char_indices().rev() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == ':' {
            start = idx;
        } else {
            break;
        }
    }
    let raw = prefix[start..end]
        .trim_matches(|ch: char| ch == '.' || ch == ':')
        .replace("::", ".");
    let name = raw.rsplit('.').next().unwrap_or_default();
    if is_identifier_segment(name) {
        Some(name.to_string())
    } else {
        None
    }
}

fn assignment_operator_position(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    for (idx, byte) in bytes.iter().enumerate() {
        if *byte != b'=' {
            continue;
        }
        let prev = idx.checked_sub(1).and_then(|pos| bytes.get(pos)).copied();
        let next = bytes.get(idx + 1).copied();
        if matches!(prev, Some(b'=') | Some(b'!') | Some(b'<') | Some(b'>'))
            || matches!(next, Some(b'=') | Some(b'>'))
        {
            continue;
        }
        return Some(idx);
    }
    None
}

fn same_parent_path(left: &str, right: &str) -> bool {
    path_parent(left) == path_parent(right)
}

fn same_path_family(left: &str, right: &str) -> bool {
    if !same_parent_path(left, right) {
        return false;
    }
    let left = path_family_key(left);
    let right = path_family_key(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    let left_dot_root = left.split('.').next().unwrap_or(&left);
    let right_dot_root = right.split('.').next().unwrap_or(&right);
    if left.contains('.') && right.contains('.') && left_dot_root == right_dot_root {
        return true;
    }
    path_has_family_suffix(&left, &right) || path_has_family_suffix(&right, &left)
}

fn path_parent(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn path_family_key(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);
    stem.to_ascii_lowercase()
}

fn path_has_family_suffix(base: &str, candidate: &str) -> bool {
    candidate
        .strip_prefix(base)
        .and_then(|suffix| suffix.chars().next())
        .is_some_and(|separator| matches!(separator, '.' | '_' | '-'))
}

#[allow(dead_code)]
fn append_symbol_body_continuation_chains(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    include_bodies: bool,
    out: &mut String,
) -> Result<()> {
    let mut chains = collect_symbol_body_continuation_chains(index, file, symbol, body)?;
    if !include_bodies {
        chains.retain(|chain| {
            !chain.steps.is_empty()
                || !symbol_target_dispatch_candidates(index, &chain.source).is_empty()
        });
    }
    if chains.is_empty() {
        return Ok(());
    }
    chains.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| right.steps.len().cmp(&left.steps.len()))
            .then_with(|| left.source.path.cmp(&right.source.path))
            .then_with(|| left.source.line_start.cmp(&right.source.line_start))
    });
    out.push_str("body lead continuation chains:\n");
    out.push_str(
        "  deterministic executable corridors; traversal stops at a branch, terminal, or cycle:\n",
    );
    let mut emitted_bodies = BTreeSet::<(String, usize, String)>::new();
    for chain in chains {
        out.push_str("  ");
        out.push_str(&format!(
            "{}:L{} {} ({})",
            chain.source.path, chain.source.line_start, chain.source.name, chain.source.kind
        ));
        for step in &chain.steps {
            out.push_str(&format!(
                " -> {}:L{} {} ({})",
                step.path, step.line_start, step.name, step.kind
            ));
        }
        if let Some(last) = chain.steps.last()
            && !last.detail.trim().is_empty()
        {
            out.push_str(&format!(" // {}", last.detail));
        }
        out.push('\n');
        append_continuation_terminal_evidence(index, &chain, include_bodies, out);
        if !include_bodies {
            continue;
        }
        for target in std::iter::once(&chain.source).chain(chain.steps.iter()) {
            let key = (target.path.clone(), target.line_start, target.name.clone());
            if !emitted_bodies.insert(key) {
                continue;
            }
            let Some(target_file) = index.file(&target.path) else {
                continue;
            };
            let Some(target_symbol) = symbol_for_target(target_file, target) else {
                continue;
            };
            let target_end = target_symbol.line_end.max(target_symbol.line_start);
            let span = target_end.saturating_sub(target_symbol.line_start) + 1;
            if is_large_container_symbol(target_symbol, span) {
                continue;
            }
            let content = index.file_content(target_file)?;
            let active_content = mask_comments(target_file.language.as_str(), &content);
            out.push_str(&format!(
                "    corridor exact body: {}:L{}-L{} {} {}\n",
                target.path,
                target_symbol.line_start,
                target_end,
                target_symbol.kind,
                target_symbol.name
            ));
            out.push_str(&extract_lines(
                &active_content,
                target_symbol.line_start,
                target_end,
                true,
            ));
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn append_continuation_terminal_evidence(
    index: &Codebase,
    chain: &BodySymbolContinuationChain,
    include_bodies: bool,
    out: &mut String,
) {
    let terminal = chain.steps.last().unwrap_or(&chain.source);
    let dispatch_candidates = symbol_target_dispatch_candidates(index, terminal);
    if !dispatch_candidates.is_empty() {
        let Some(file) = index.file(&terminal.path) else {
            return;
        };
        let Some(symbol) = symbol_for_target(file, terminal) else {
            return;
        };
        out.push_str("    terminal dispatch branches (mutually exclusive graph branches; use construction/assignment evidence to choose the active implementation):\n");
        append_dispatch_candidates(index, symbol, &dispatch_candidates, "      ", out);
        return;
    }
    if include_bodies || chain.steps.is_empty() {
        return;
    }
    if let Some(snippet) = compact_symbol_target_snippet_limited(
        index,
        terminal,
        SYMBOL_BODY_DISPATCH_PREVIEW_MAX_LINES,
        SYMBOL_BODY_DISPATCH_PREVIEW_MAX_CHARS,
    ) {
        out.push_str(&format!("    terminal exact body preview: {snippet}\n"));
    }
}

#[allow(dead_code)]
fn collect_symbol_body_continuation_chains(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
) -> Result<Vec<BodySymbolContinuationChain>> {
    let mut chains = Vec::new();
    let mut seen_chains = BTreeSet::<String>::new();
    let Some(lead) = deterministic_symbol_call_lead(
        &file.path,
        symbol_body_verified_call_leads(index, file, symbol, body),
    ) else {
        return Ok(chains);
    };
    let chain = continue_symbol_target(index, lead.target, lead.score)?;
    let chain_key = symbol_body_continuation_chain_key(&chain.source, &chain.steps);
    if seen_chains.insert(chain_key) {
        chains.push(chain);
    }
    Ok(chains)
}

#[allow(dead_code)]
fn continue_symbol_target(
    index: &Codebase,
    source: SymbolTarget,
    initial_score: usize,
) -> Result<BodySymbolContinuationChain> {
    let mut visited = BTreeSet::<(String, usize, String)>::new();
    visited.insert((source.path.clone(), source.line_start, source.name.clone()));
    let mut current = source.clone();
    let mut score = initial_score;
    let mut steps = Vec::new();
    loop {
        let Some(file) = index.file(&current.path) else {
            break;
        };
        let Some(symbol) = symbol_for_target(file, &current) else {
            break;
        };
        let symbol_end = symbol.line_end.max(symbol.line_start);
        let span = symbol_end.saturating_sub(symbol.line_start) + 1;
        if span > CONTEXT_SYMBOL_HANDOFF_MAX_SOURCE_LINES {
            break;
        }
        let content = index.file_content(file)?;
        let active_content = mask_comments(file.language.as_str(), &content);
        let body = source_line_slice(&active_content, symbol.line_start, symbol_end);
        let next = symbol_body_verified_call_leads(index, file, symbol, &body)
            .into_iter()
            .filter(|next| {
                !visited.contains(&(
                    next.target.path.clone(),
                    next.target.line_start,
                    next.target.name.clone(),
                ))
            })
            .collect::<Vec<_>>();
        let Some(next) = deterministic_symbol_call_lead(&file.path, next) else {
            break;
        };
        let key = (
            next.target.path.clone(),
            next.target.line_start,
            next.target.name.clone(),
        );
        visited.insert(key);
        score = score.saturating_add(next.score);
        current = next.target.clone();
        steps.push(next.target);
    }
    Ok(BodySymbolContinuationChain {
        source,
        steps,
        score,
    })
}

#[allow(dead_code)]
fn symbol_body_verified_call_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
) -> Vec<BodySymbolLead> {
    let mut by_target = BTreeMap::<(String, usize, String), BodySymbolLead>::new();
    let mut resolved_qualified_names = BTreeSet::<String>::new();
    for (order, edge) in callpath_edges_from_body(index, file, symbol, body, None, false)
        .into_iter()
        .filter(|edge| matches!(edge.relation.as_str(), "qualified_call" | "direct_call"))
        .filter(|edge| symbol_target_is_executable(index, &edge.target))
        .enumerate()
    {
        if edge.relation == "qualified_call" {
            resolved_qualified_names.insert(edge.target.name.clone());
        }
        let key = (
            edge.target.path.clone(),
            edge.target.line_start,
            edge.target.name.clone(),
        );
        by_target.insert(
            key,
            BodySymbolLead {
                order,
                score: if edge.relation == "qualified_call" {
                    120
                } else {
                    100
                },
                query_matches: BTreeSet::new(),
                target: edge.target,
            },
        );
    }
    for lead in symbol_body_leads(index, file, symbol, body, usize::MAX)
        .into_iter()
        .filter(|lead| symbol_target_is_executable(index, &lead.target))
        .filter(|lead| body_contains_direct_call(body, &lead.target.name, &symbol.name))
        .filter(|lead| {
            !resolved_qualified_names.contains(&lead.target.name)
                || body_contains_unqualified_call(body, &lead.target.name, &symbol.name)
        })
    {
        let key = (
            lead.target.path.clone(),
            lead.target.line_start,
            lead.target.name.clone(),
        );
        by_target.entry(key).or_insert(lead);
    }
    by_target.into_values().collect()
}

#[allow(dead_code)]
fn body_contains_unqualified_call(body: &str, name: &str, source_symbol_name: &str) -> bool {
    body.lines().enumerate().any(|(offset, line)| {
        if offset == 0 && name == source_symbol_name {
            return false;
        }
        let code_line = strip_strings_and_line_comment(line);
        identifier_call_receiver_kinds(&code_line, name).0
    })
}

#[allow(dead_code)]
fn deterministic_symbol_call_lead(
    source_path: &str,
    leads: Vec<BodySymbolLead>,
) -> Option<BodySymbolLead> {
    let cross_file = leads
        .iter()
        .filter(|lead| lead.target.path != source_path)
        .cloned()
        .collect::<Vec<_>>();
    if cross_file.len() == 1 {
        return cross_file.into_iter().next();
    }
    if !cross_file.is_empty() {
        return None;
    }
    (leads.len() == 1)
        .then(|| leads.into_iter().next())
        .flatten()
}

#[allow(dead_code)]
fn body_contains_direct_call(body: &str, name: &str, source_symbol_name: &str) -> bool {
    body.lines().enumerate().any(|(offset, line)| {
        if offset == 0 && name == source_symbol_name {
            return false;
        }
        let code_line = strip_strings_and_line_comment(line);
        !identifier_call_argument_counts(&code_line, name).is_empty()
    })
}

#[allow(dead_code)]
fn symbol_target_is_executable(index: &Codebase, target: &SymbolTarget) -> bool {
    index
        .file(&target.path)
        .and_then(|file| symbol_for_target(file, target))
        .is_some_and(|symbol| {
            matches!(
                symbol.kind.as_str(),
                "method" | "function" | "constructor" | "procedure" | "macro"
            )
        })
}

#[allow(dead_code)]
fn symbol_body_continuation_chain_key(source: &SymbolTarget, steps: &[SymbolTarget]) -> String {
    let mut key = format!("{}:{}:{}", source.path, source.line_start, source.name);
    for step in steps {
        key.push('|');
        key.push_str(&step.path);
        key.push(':');
        key.push_str(&step.line_start.to_string());
        key.push(':');
        key.push_str(&step.name);
    }
    key
}

fn symbol_body_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    limit: usize,
) -> Vec<BodySymbolLead> {
    symbol_body_leads_with_terms(index, file, symbol, body, limit, &[])
}

fn symbol_body_expansion_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    limit: usize,
    query_terms: &[String],
) -> Vec<BodySymbolLead> {
    if limit == 0 {
        return Vec::new();
    }
    let fetch_limit = limit
        .saturating_mul(4)
        .max(limit)
        .max(SYMBOL_BODY_LEAD_LIMIT);
    let mut leads =
        symbol_body_leads_with_terms(index, file, symbol, body, fetch_limit, query_terms);
    if leads.len() <= limit {
        return leads;
    }
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    leads.sort_by(|left, right| {
        right
            .query_matches
            .len()
            .cmp(&left.query_matches.len())
            .then_with(|| (right.target.path == file.path).cmp(&(left.target.path == file.path)))
            .then_with(|| {
                deps.contains(&right.target.path)
                    .cmp(&deps.contains(&left.target.path))
            })
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| right.score.cmp(&left.score))
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    leads.truncate(limit);
    leads
}

fn symbol_body_ordered_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    limit: usize,
) -> Vec<BodySymbolLead> {
    if limit == 0 {
        return Vec::new();
    }
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut seen_names = BTreeSet::new();
    let mut seen_targets = BTreeSet::new();
    let mut leads = Vec::new();
    for (order, identifier) in source_code_identifiers(body).into_iter().enumerate() {
        if identifier.len() < 3
            || identifier == symbol.name
            || !is_data_type_lead_identifier(&identifier)
            || !seen_names.insert(identifier.clone())
        {
            continue;
        }
        let mut candidates = index
            .symbols_named(&identifier)
            .into_iter()
            .filter(|(candidate_file, candidate_symbol)| {
                candidate_file.path != file.path
                    || candidate_symbol.line_start != symbol.line_start
                    || candidate_symbol.name != symbol.name
            })
            .map(|(candidate_file, candidate_symbol)| {
                let target = target_from_symbol(candidate_file, candidate_symbol);
                let same_file = candidate_file.path == file.path;
                let dependency = deps.contains(&candidate_file.path);
                let score = symbol_body_lead_score(file, &deps, candidate_file, candidate_symbol);
                (score, target, same_file, dependency)
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            continue;
        }
        let unique_candidate = candidates.len() == 1;
        candidates.retain(|(_, target, _, _)| {
            body_lead_target_allowed(&file.path, &deps, target, unique_candidate, false)
        });
        if candidates.is_empty() {
            continue;
        }
        candidates.sort_by(|left, right| {
            right
                .2
                .cmp(&left.2)
                .then_with(|| right.3.cmp(&left.3))
                .then_with(|| right.0.cmp(&left.0))
                .then_with(|| left.1.path.cmp(&right.1.path))
                .then_with(|| left.1.line_start.cmp(&right.1.line_start))
        });
        let (score, target, _, _) = candidates.remove(0);
        if seen_targets.insert((target.path.clone(), target.line_start, target.name.clone())) {
            leads.push(BodySymbolLead {
                order,
                score,
                query_matches: BTreeSet::new(),
                target,
            });
        }
    }
    select_source_order_body_leads(leads, limit)
}

fn symbol_body_data_type_leads(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    limit: usize,
) -> Vec<BodySymbolLead> {
    if limit == 0 {
        return Vec::new();
    }
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let identifiers = source_code_identifiers(body);
    let context_scores = body_data_type_usage_context_scores(body);
    let mut seen_names = BTreeSet::new();
    let mut seen_targets = BTreeSet::new();
    let mut leads = Vec::new();

    for (order, identifier) in identifiers.into_iter().enumerate() {
        if identifier.len() < 3
            || !is_data_type_lead_identifier(&identifier)
            || !seen_names.insert(identifier.clone())
        {
            continue;
        }
        let context_score = context_scores.get(&identifier).copied().unwrap_or_default();
        let candidates = index
            .symbols_named(&identifier)
            .into_iter()
            .filter(|(candidate_file, _)| generic_source_path_score(&candidate_file.path) >= 0.0)
            .filter(|(_, candidate_symbol)| {
                is_body_data_type_symbol_kind(candidate_symbol)
                    && body_data_type_context_allows_symbol(candidate_symbol, context_score)
            })
            .filter(|(candidate_file, candidate_symbol)| {
                body_data_type_candidate_has_signal(
                    index,
                    file,
                    &deps,
                    candidate_file,
                    candidate_symbol,
                    context_score,
                )
            })
            .filter(|(candidate_file, candidate_symbol)| {
                candidate_file.path != file.path
                    || candidate_symbol.line_start != symbol.line_start
                    || candidate_symbol.name != symbol.name
            })
            .map(|(candidate_file, candidate_symbol)| {
                let target = target_from_symbol(candidate_file, candidate_symbol);
                let score =
                    body_data_type_lead_score(index, file, &deps, candidate_file, candidate_symbol)
                        + context_score;
                (
                    score,
                    target,
                    candidate_file.path == file.path,
                    deps.contains(&candidate_file.path),
                )
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            continue;
        }
        let unique_candidate = candidates.len() == 1;
        let mut ranked = candidates
            .into_iter()
            .filter(|(_, target, _, _)| {
                body_lead_target_allowed(&file.path, &deps, target, unique_candidate, false)
            })
            .collect::<Vec<_>>();
        if ranked.is_empty() {
            continue;
        }
        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.2.cmp(&left.2))
                .then_with(|| right.3.cmp(&left.3))
                .then_with(|| left.1.path.cmp(&right.1.path))
                .then_with(|| left.1.line_start.cmp(&right.1.line_start))
        });
        let (score, target, _, _) = ranked.remove(0);
        if score < SYMBOL_BODY_DATA_TYPE_MIN_SCORE {
            continue;
        }
        if seen_targets.insert((target.path.clone(), target.line_start, target.name.clone())) {
            leads.push(BodySymbolLead {
                order,
                score,
                query_matches: BTreeSet::new(),
                target,
            });
        }
    }

    let source_ordered = leads.clone();
    leads.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    select_balanced_body_data_type_leads(leads, source_ordered, limit)
}

fn select_source_order_body_leads(leads: Vec<BodySymbolLead>, limit: usize) -> Vec<BodySymbolLead> {
    if leads.len() <= limit {
        return leads;
    }
    let mut indices = BTreeSet::<usize>::new();
    let head = (limit / 3).max(2).min(limit);
    let tail = (limit / 3).max(2).min(limit.saturating_sub(head));
    for idx in 0..head.min(leads.len()) {
        if indices.len() >= limit {
            break;
        }
        indices.insert(idx);
    }
    for idx in leads.len().saturating_sub(tail)..leads.len() {
        if indices.len() >= limit {
            break;
        }
        indices.insert(idx);
    }
    if limit > 1 {
        for slot in 0..limit {
            if indices.len() >= limit {
                break;
            }
            let idx = slot * (leads.len() - 1) / (limit - 1);
            indices.insert(idx);
        }
    }
    for idx in 0..leads.len() {
        if indices.len() >= limit {
            break;
        }
        indices.insert(idx);
    }
    indices
        .into_iter()
        .filter_map(|idx| leads.get(idx).cloned())
        .collect()
}

fn symbol_body_leads_with_terms(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    limit: usize,
    query_terms: &[String],
) -> Vec<BodySymbolLead> {
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut seen_names = BTreeSet::new();
    let mut seen_targets = BTreeSet::new();
    let mut leads = Vec::new();
    let identifiers = source_code_identifiers(body);
    let body_query_counts = body_query_term_counts(&identifiers, query_terms);

    for (order, identifier) in identifiers.into_iter().enumerate() {
        if identifier.len() < 3 || !seen_names.insert(identifier.clone()) {
            continue;
        }

        let candidates = index
            .symbols_named(&identifier)
            .into_iter()
            .filter(|(candidate_file, candidate_symbol)| {
                candidate_file.path != file.path
                    || candidate_symbol.line_start != symbol.line_start
                    || candidate_symbol.name != symbol.name
            })
            .map(|(candidate_file, candidate_symbol)| {
                let target = target_from_symbol(candidate_file, candidate_symbol);
                let score = symbol_body_lead_score(file, &deps, candidate_file, candidate_symbol)
                    + symbol_target_query_score(&target, query_terms, &body_query_counts);
                let query_matches =
                    target_distinctive_query_matches(&target, query_terms, &body_query_counts);
                (
                    score,
                    query_matches,
                    target,
                    candidate_file.path == file.path,
                    deps.contains(&candidate_file.path),
                )
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            continue;
        }

        let unique_candidate = candidates.len() == 1;
        let mut ranked = candidates
            .into_iter()
            .filter(|(_, query_matches, target, _, _)| {
                body_lead_target_allowed(
                    &file.path,
                    &deps,
                    target,
                    unique_candidate,
                    !query_matches.is_empty(),
                )
            })
            .collect::<Vec<_>>();
        if ranked.is_empty() {
            continue;
        }

        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.2.path.cmp(&right.2.path))
                .then_with(|| left.2.line_start.cmp(&right.2.line_start))
        });
        let (score, query_matches, target, _, _) = ranked.remove(0);
        if seen_targets.insert((target.path.clone(), target.line_start, target.name.clone())) {
            leads.push(BodySymbolLead {
                order,
                score,
                query_matches,
                target,
            });
        }
    }

    leads.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    select_diverse_body_symbol_leads(leads, &file.path, limit)
}

fn select_balanced_body_data_type_leads(
    ranked: Vec<BodySymbolLead>,
    source_ordered: Vec<BodySymbolLead>,
    limit: usize,
) -> Vec<BodySymbolLead> {
    if ranked.len() <= limit {
        let mut selected = ranked;
        selected.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then_with(|| left.target.path.cmp(&right.target.path))
                .then_with(|| left.target.line_start.cmp(&right.target.line_start))
        });
        return selected;
    }
    let mut selected = Vec::new();
    let mut used = BTreeSet::<(String, usize, String)>::new();
    let mut push = |lead: BodySymbolLead, selected: &mut Vec<BodySymbolLead>| {
        if selected.len() >= limit {
            return;
        }
        let key = (
            lead.target.path.clone(),
            lead.target.line_start,
            lead.target.name.clone(),
        );
        if used.insert(key) {
            selected.push(lead);
        }
    };
    let source_order_limit = if limit <= 3 {
        limit.saturating_sub(1).max(1)
    } else {
        ((limit * 2) / 3).max(1).min(limit.saturating_sub(2))
    };
    for lead in select_source_order_body_leads(source_ordered, source_order_limit) {
        push(lead, &mut selected);
    }
    let high_score_limit = limit.saturating_sub(selected.len()).max(2).min(limit);
    for lead in ranked.iter().take(high_score_limit).cloned() {
        push(lead, &mut selected);
    }
    for lead in ranked {
        push(lead, &mut selected);
    }
    selected.sort_by(|left, right| {
        left.order
            .cmp(&right.order)
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    selected
}

fn body_data_type_reference_hits(
    index: &Codebase,
    source_file: &FileEntry,
    source_symbol: &Symbol,
    target_file: &FileEntry,
    target_symbol: &Symbol,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    if limit == 0 || target_symbol.name.len() < 3 || target_symbol.name.starts_with("__") {
        return Ok(Vec::new());
    }
    let all_definition_sites = index
        .symbols_named(&target_symbol.name)
        .into_iter()
        .map(|(definition_file, definition_symbol)| {
            (definition_file.path.clone(), definition_symbol.line_start)
        })
        .collect::<BTreeSet<_>>();
    let mut hits = reference_candidates_with_limit(
        index,
        &target_symbol.name,
        Some(SYMBOL_BODY_DATA_TYPE_REF_MAX_WORD_HITS),
    )?;
    hits.retain(|hit| {
        if all_definition_sites.contains(&(hit.path.clone(), hit.line)) {
            return false;
        }
        if hit.path == source_file.path
            && hit.line >= source_symbol.line_start
            && hit.line <= source_symbol.line_end
        {
            return false;
        }
        if hit.path == target_file.path
            && hit.line >= target_symbol.line_start
            && hit.line <= target_symbol.line_end
        {
            return false;
        }
        true
    });
    hits.sort_by(|left, right| {
        body_data_type_reference_hit_score(index, source_file, target_file, right)
            .cmp(&body_data_type_reference_hit_score(
                index,
                source_file,
                target_file,
                left,
            ))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
    });
    hits.truncate(limit);
    Ok(hits)
}

fn body_data_type_reference_hit_score(
    index: &Codebase,
    source_file: &FileEntry,
    target_file: &FileEntry,
    hit: &SearchHit,
) -> usize {
    let mut score = 0usize;
    if hit.path != source_file.path {
        score += 50;
    } else {
        score += 10;
    }
    if index
        .reverse_deps_for(&target_file.path)
        .contains(&hit.path)
    {
        score += 30;
    }
    if same_parent_path(&source_file.path, &hit.path) {
        score += 20;
    } else if same_path_family(&source_file.path, &hit.path) {
        score += 15;
    }
    if let Some(scope) = &hit.scope {
        score += match scope.kind.as_str() {
            "method" | "function" | "constructor" => 35,
            "class" | "interface" | "struct" | "enum" | "record" => 12,
            _ => 6,
        };
        score += symbol_name_specificity_weight_from_name(&scope.name).min(18);
    }
    if generic_source_path_score(&hit.path) >= 0.0 {
        score += 10;
    }
    let degree = file_graph_degree(index, &hit.path);
    if degree <= 160 {
        score += 8;
    }
    score
}

fn body_query_term_counts(
    identifiers: &[String],
    query_terms: &[String],
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    if query_terms.is_empty() {
        return counts;
    }
    for identifier in identifiers {
        let identity_terms = identity_terms_from_text(identifier);
        for term in query_terms {
            let key = term.to_ascii_lowercase();
            if identity_terms
                .iter()
                .any(|identity| context_identity_terms_match(&key, identity))
            {
                *counts.entry(key).or_insert(0) += 1;
            }
        }
    }
    counts
}

fn symbol_target_query_score(
    target: &SymbolTarget,
    query_terms: &[String],
    body_query_counts: &BTreeMap<String, usize>,
) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    let name_terms = identity_terms_from_text(&target.name);
    let path_terms = identity_terms_from_text(&target.path);
    let detail_terms = identity_terms_from_text(&target.detail);
    let mut strong_matches = 0usize;
    let mut score = 0usize;
    for term in query_terms {
        let key = term.to_ascii_lowercase();
        if name_terms
            .iter()
            .any(|identity| context_identity_terms_match(&key, identity))
        {
            let weight = body_local_query_term_weight(&key, body_query_counts);
            strong_matches += 1;
            score += weight;
        } else if detail_terms
            .iter()
            .any(|identity| context_identity_terms_match(&key, identity))
        {
            let weight = body_local_query_term_weight(&key, body_query_counts).min(18);
            strong_matches += 1;
            score += weight;
        } else if path_terms
            .iter()
            .any(|identity| context_identity_terms_match(&key, identity))
        {
            score += 4;
        }
    }
    if strong_matches >= 2 {
        score += 24;
    }
    score
}

fn target_distinctive_query_matches(
    target: &SymbolTarget,
    query_terms: &[String],
    body_query_counts: &BTreeMap<String, usize>,
) -> BTreeSet<String> {
    let mut matches = BTreeSet::new();
    if query_terms.is_empty() {
        return matches;
    }
    let name_terms = identity_terms_from_text(&target.name);
    let detail_terms = identity_terms_from_text(&target.detail);
    for term in query_terms {
        let key = term.to_ascii_lowercase();
        if body_local_query_term_weight(&key, body_query_counts) < 24 {
            continue;
        }
        if name_terms
            .iter()
            .any(|identity| context_identity_terms_match(&key, identity))
            || detail_terms
                .iter()
                .any(|identity| context_identity_terms_match(&key, identity))
        {
            matches.insert(key);
        }
    }
    matches
}

fn body_local_query_term_weight(term: &str, body_query_counts: &BTreeMap<String, usize>) -> usize {
    match body_query_counts.get(term).copied().unwrap_or(0) {
        0 | 1 => 60,
        2 | 3 => 45,
        4..=8 => 24,
        _ => 8,
    }
}

fn select_diverse_body_symbol_leads(
    leads: Vec<BodySymbolLead>,
    source_path: &str,
    limit: usize,
) -> Vec<BodySymbolLead> {
    if leads.len() <= limit {
        return leads;
    }
    let mut selected = Vec::new();
    let mut used = BTreeSet::<(String, usize, String)>::new();
    let mut covered_query_terms = BTreeSet::<String>::new();
    let mut push_lead = |lead: BodySymbolLead,
                         selected: &mut Vec<BodySymbolLead>,
                         covered_query_terms: &mut BTreeSet<String>| {
        if selected.len() >= limit {
            return;
        }
        let key = (
            lead.target.path.clone(),
            lead.target.line_start,
            lead.target.name.clone(),
        );
        if used.insert(key) {
            for term in &lead.query_matches {
                covered_query_terms.insert(term.clone());
            }
            selected.push(lead);
        }
    };
    if let Some(position) = leads
        .iter()
        .position(|lead| lead.target.path == source_path)
    {
        push_lead(
            leads[position].clone(),
            &mut selected,
            &mut covered_query_terms,
        );
    }
    if let Some(position) = leads
        .iter()
        .position(|lead| lead.target.path != source_path)
    {
        push_lead(
            leads[position].clone(),
            &mut selected,
            &mut covered_query_terms,
        );
    }
    for lead in &leads {
        if selected.len() >= limit {
            break;
        }
        if lead.query_matches.is_empty()
            || lead
                .query_matches
                .iter()
                .all(|term| covered_query_terms.contains(term))
        {
            continue;
        }
        push_lead(lead.clone(), &mut selected, &mut covered_query_terms);
    }
    for lead in leads {
        if selected.len() >= limit {
            break;
        }
        push_lead(lead, &mut selected, &mut covered_query_terms);
    }
    selected
}

fn symbol_body_lead_score(
    source_file: &FileEntry,
    deps: &BTreeSet<String>,
    candidate_file: &FileEntry,
    candidate_symbol: &Symbol,
) -> usize {
    let structure = related_path_structure_evidence(&source_file.path, &candidate_file.path);
    let locality = if candidate_file.path == source_file.path {
        140
    } else if structure >= 4.0 {
        105
    } else if deps.contains(&candidate_file.path) && structure >= 2.0 {
        95
    } else if deps.contains(&candidate_file.path) {
        65
    } else {
        50
    };
    let structure_bonus = (structure * 8.0) as usize;
    let centrality_penalty = file_graph_degree_score_penalty(candidate_file);
    (locality
        + structure_bonus
        + symbol_kind_lead_weight(candidate_symbol)
        + symbol_name_specificity_weight(candidate_symbol))
    .saturating_sub(centrality_penalty)
}

fn body_lead_target_allowed(
    source_path: &str,
    deps: &BTreeSet<String>,
    target: &SymbolTarget,
    unique_candidate: bool,
    has_query_match: bool,
) -> bool {
    if target.path == source_path {
        return true;
    }
    let structure = related_path_structure_evidence(source_path, &target.path);
    if structure >= 3.0 {
        return true;
    }
    if has_query_match {
        return true;
    }
    let dependency = deps.contains(&target.path);
    if dependency && (structure >= 2.0 || generic_source_path_score(&target.path) >= 0.0) {
        return true;
    }
    unique_candidate
        && structure >= 2.0
        && (generic_source_path_score(&target.path) >= 0.0
            || symbol_name_specificity_weight_from_name(&target.name) > 0)
}

fn file_graph_degree_score_penalty(file: &FileEntry) -> usize {
    if generic_source_path_score(&file.path) >= 0.0 {
        0
    } else {
        24
    }
}

fn symbol_kind_lead_weight(symbol: &Symbol) -> usize {
    symbol_kind_lead_weight_from_kind(symbol.kind.as_str())
}

fn symbol_kind_lead_weight_from_kind(kind: &str) -> usize {
    match kind {
        "method" | "function" | "constructor" => 30,
        "class" | "interface" | "struct" | "trait" | "impl" => 18,
        "enum" | "const" | "static" | "type_alias" => 12,
        _ => 6,
    }
}

fn is_body_data_type_symbol_kind(symbol: &Symbol) -> bool {
    matches!(
        symbol.kind.as_str(),
        "class"
            | "interface"
            | "struct"
            | "enum"
            | "record"
            | "trait"
            | "module"
            | "type_alias"
            | "property"
            | "field"
            | "const"
            | "static"
            | "variable"
    )
}

fn is_body_data_type_definition_kind(symbol: &Symbol) -> bool {
    matches!(
        symbol.kind.as_str(),
        "class" | "interface" | "struct" | "enum" | "record" | "trait" | "module" | "type_alias"
    )
}

fn body_data_type_context_allows_symbol(symbol: &Symbol, context_score: usize) -> bool {
    if is_body_data_type_definition_kind(symbol) {
        return true;
    }
    context_score >= 36
}

fn body_data_type_candidate_has_signal(
    index: &Codebase,
    source_file: &FileEntry,
    deps: &BTreeSet<String>,
    candidate_file: &FileEntry,
    candidate_symbol: &Symbol,
    context_score: usize,
) -> bool {
    if candidate_file.path == source_file.path {
        return true;
    }
    if context_score >= 48 {
        return true;
    }
    let specificity = symbol_name_specificity_weight(candidate_symbol);
    let structure = related_path_structure_evidence(&source_file.path, &candidate_file.path);
    if specificity >= 8 {
        return true;
    }
    if structure >= 3.0 && specificity >= 4 {
        return true;
    }
    if deps.contains(&candidate_file.path)
        && specificity >= 4
        && file_graph_degree(index, &candidate_file.path) <= 80
    {
        return true;
    }
    false
}

fn is_data_type_lead_identifier(identifier: &str) -> bool {
    identifier.len() >= 5
        || identifier.contains('_')
        || identifier.chars().any(|ch| ch.is_ascii_uppercase())
}

fn body_data_type_usage_context_scores(body: &str) -> BTreeMap<String, usize> {
    let mut scores = BTreeMap::<String, usize>::new();
    let lines = body.lines().collect::<Vec<_>>();
    for (idx, line) in lines.iter().enumerate() {
        if is_comment_or_blank(line) {
            continue;
        }
        for identifier in raw_identifiers(line) {
            let lookahead = lines
                .iter()
                .skip(idx + 1)
                .copied()
                .find(|next| !is_comment_or_blank(next));
            let score = body_data_type_usage_context_score(line, lookahead, &identifier);
            if score > 0 {
                scores
                    .entry(identifier)
                    .and_modify(|current| *current = (*current).max(score))
                    .or_insert(score);
            }
        }
    }
    scores
}

fn body_data_type_usage_context_score(
    line: &str,
    lookahead: Option<&str>,
    identifier: &str,
) -> usize {
    let mut best = 0usize;
    let mut from = 0usize;
    while let Some(relative) = line[from..].find(identifier) {
        let start = from + relative;
        let end = start + identifier.len();
        let before = line[..start].chars().next_back();
        let after = line[end..].chars().next();
        if before.is_some_and(is_identifier_char) || after.is_some_and(is_identifier_char) {
            from = start + 1;
            continue;
        }
        let prefix = line[..start].trim_end();
        let suffix = line[end..].trim_start();
        let mut score = 0usize;
        if suffix.starts_with('(') || suffix.starts_with('{') {
            score += 120;
        } else if suffix.starts_with('<') {
            score += 48;
        } else if suffix.is_empty()
            && lookahead
                .map(str::trim_start)
                .is_some_and(|next| next.starts_with('{') || next.starts_with('('))
        {
            score += 120;
        }
        if line.contains('=') || line.contains(':') {
            score += 8;
        }
        if prefix.ends_with('.') {
            score = score.saturating_sub(40);
        }
        best = best.max(score);
        from = end;
    }
    best
}

fn body_data_type_lead_score(
    index: &Codebase,
    source_file: &FileEntry,
    deps: &BTreeSet<String>,
    candidate_file: &FileEntry,
    candidate_symbol: &Symbol,
) -> usize {
    let locality = if candidate_file.path == source_file.path {
        90
    } else if deps.contains(&candidate_file.path) {
        95
    } else if same_parent_path(&source_file.path, &candidate_file.path) {
        70
    } else {
        35
    };
    let structure =
        (related_path_structure_evidence(&source_file.path, &candidate_file.path) * 18.0) as usize;
    let specificity = symbol_name_specificity_weight(candidate_symbol) * 3;
    let source_order_bonus = candidate_symbol.line_start.min(240) / 6;
    let centrality_penalty = file_graph_degree(index, &candidate_file.path)
        .saturating_sub(80)
        .min(120);
    (locality
        + structure
        + body_data_type_kind_weight(candidate_symbol.kind.as_str())
        + specificity
        + source_order_bonus)
        .saturating_sub(centrality_penalty)
}

fn body_data_type_kind_weight(kind: &str) -> usize {
    match kind {
        "class" | "interface" | "struct" | "record" | "type_alias" => 40,
        "enum" | "const" | "static" => 28,
        "property" | "field" | "variable" => 18,
        "trait" | "module" => 12,
        _ => 4,
    }
}

fn symbol_name_specificity_weight(symbol: &Symbol) -> usize {
    symbol_name_specificity_weight_from_name(&symbol.name)
}

fn symbol_name_specificity_weight_from_name(name: &str) -> usize {
    let mut parts = split_identifier(name);
    if parts.len() > 1
        && parts
            .first()
            .is_some_and(|part| *part == name.to_ascii_lowercase())
    {
        parts.remove(0);
    }
    let part_weight = parts.len().saturating_sub(2) * 8;
    let length_weight = name.len().saturating_sub(8).min(16);
    part_weight + length_weight
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
    let wants_json = get_str(args, "format").as_deref() == Some("json");
    let mut max_results = get_usize(args, "max_results")
        .unwrap_or(20)
        .clamp(1, 10_000);
    let json_results_clipped = wants_json && max_results > SEARCH_JSON_MAX_RESULTS;
    if wants_json {
        max_results = max_results.min(SEARCH_JSON_MAX_RESULTS);
    }
    let compact = get_bool(args, "compact");
    let scope = get_bool(args, "scope") || compact;
    let regex = get_bool(args, "regex");
    let path_glob = get_str(args, "path_glob").filter(|glob| !glob.trim().is_empty());
    let offset = get_usize(args, "offset").unwrap_or(0);
    let requested = max_results.saturating_add(offset).clamp(1, 20_000);
    let diversify = wants_json || compact || get_bool(args, "diverse");
    let fetch_limit = if diversify {
        requested
            .saturating_mul(SEARCH_DIVERSE_FETCH_MULTIPLIER)
            .clamp(requested, 20_000)
    } else {
        requested
    };
    let hits = if !regex && is_ranked_text_query(&query) {
        let ranked = ranked_line_hits(
            index,
            &query,
            fetch_limit,
            path_glob.as_deref(),
            compact,
            scope,
        )?;
        if ranked.is_empty() {
            index.text_line_hits(
                &query,
                fetch_limit,
                false,
                path_glob.as_deref(),
                compact,
                scope,
            )?
        } else {
            ranked
        }
    } else {
        index.text_line_hits(
            &query,
            fetch_limit,
            regex,
            path_glob.as_deref(),
            compact,
            scope,
        )?
    };
    let hits = if diversify {
        diversify_search_hits(index, &query, hits, requested)
    } else {
        hits
    };
    let paged = hits
        .into_iter()
        .skip(offset)
        .take(max_results)
        .collect::<Vec<_>>();
    if get_bool(args, "paths_only") {
        return Ok(format_paths_only_line_hits(&query, paged, offset));
    }
    if wants_json {
        return format_json_line_hits(&query, paged, regex, offset, json_results_clipped);
    }
    Ok(format_line_hits(&query, paged, compact))
}

fn diversify_search_hits(
    index: &Codebase,
    query: &str,
    hits: Vec<SearchHit>,
    limit: usize,
) -> Vec<SearchHit> {
    if hits.len() <= limit || limit == 0 {
        return hits;
    }
    let mut groups = Vec::<Vec<SearchHit>>::new();
    let mut group_indices = BTreeMap::<String, usize>::new();
    for hit in hits {
        let key = search_hit_group_key(&hit);
        let idx = if let Some(idx) = group_indices.get(&key).copied() {
            idx
        } else {
            let idx = groups.len();
            groups.push(Vec::new());
            group_indices.insert(key, idx);
            idx
        };
        groups[idx].push(hit);
    }
    for group in &mut groups {
        group.sort_by(|left, right| {
            search_hit_signal_score(index, query, right)
                .cmp(&search_hit_signal_score(index, query, left))
                .then_with(|| left.line.cmp(&right.line))
                .then_with(|| left.path.cmp(&right.path))
        });
    }
    groups.sort_by(|left, right| {
        let left_score = left
            .first()
            .map(|hit| search_hit_signal_score(index, query, hit))
            .unwrap_or_default();
        let right_score = right
            .first()
            .map(|hit| search_hit_signal_score(index, query, hit))
            .unwrap_or_default();
        right_score
            .cmp(&left_score)
            .then_with(|| {
                left.first()
                    .map(|hit| hit.path.as_str())
                    .unwrap_or_default()
                    .cmp(
                        right
                            .first()
                            .map(|hit| hit.path.as_str())
                            .unwrap_or_default(),
                    )
            })
            .then_with(|| {
                left.first()
                    .map(|hit| hit.line)
                    .unwrap_or_default()
                    .cmp(&right.first().map(|hit| hit.line).unwrap_or_default())
            })
    });

    let mut selected = Vec::<SearchHit>::new();
    let mut seen = BTreeSet::<(String, usize)>::new();
    let mut path_counts = BTreeMap::<String, usize>::new();
    push_diverse_search_round(
        &groups,
        limit,
        0,
        1,
        &mut selected,
        &mut seen,
        &mut path_counts,
    );
    let mut group_offset = 0usize;
    while selected.len() < limit {
        let before = selected.len();
        push_diverse_search_round(
            &groups,
            limit,
            group_offset,
            SEARCH_DIVERSE_GROUP_LIMIT,
            &mut selected,
            &mut seen,
            &mut path_counts,
        );
        if selected.len() == before {
            break;
        }
        group_offset += 1;
    }
    selected
}

fn push_diverse_search_round(
    groups: &[Vec<SearchHit>],
    limit: usize,
    group_offset: usize,
    path_soft_limit: usize,
    selected: &mut Vec<SearchHit>,
    seen: &mut BTreeSet<(String, usize)>,
    path_counts: &mut BTreeMap<String, usize>,
) {
    for group in groups {
        if selected.len() >= limit {
            return;
        }
        let Some(hit) = group.get(group_offset) else {
            continue;
        };
        let count = path_counts.get(&hit.path).copied().unwrap_or_default();
        if count >= path_soft_limit {
            continue;
        }
        if seen.insert((hit.path.clone(), hit.line)) {
            *path_counts.entry(hit.path.clone()).or_default() += 1;
            selected.push(hit.clone());
        }
    }
}

fn search_hit_group_key(hit: &SearchHit) -> String {
    if let Some(scope) = &hit.scope {
        format!("{}:{}:{}:{}", hit.path, scope.kind, scope.start, scope.name)
    } else {
        hit.path.clone()
    }
}

fn search_hit_signal_score(index: &Codebase, query: &str, hit: &SearchHit) -> usize {
    let mut score = 0usize;
    let definition_names = search_definition_names(index, query);
    if let Some(file) = index.file(&hit.path) {
        let defines_query = file.symbols.iter().any(|symbol| {
            definition_names
                .iter()
                .any(|name| symbol.name.eq_ignore_ascii_case(name))
        });
        let is_definition_line = file.symbols.iter().any(|symbol| {
            symbol.line_start == hit.line
                && definition_names
                    .iter()
                    .any(|name| symbol.name.eq_ignore_ascii_case(name))
        });
        if defines_query {
            score += 80;
        }
        if is_definition_line {
            score += 200;
        }
    }
    if let Some(scope) = &hit.scope {
        score += match scope.kind.as_str() {
            "method" | "function" => 30,
            "constructor" => 8,
            "class" | "interface" | "struct" | "enum" | "record" => 6,
            _ => 4,
        };
        score += symbol_name_specificity_weight_from_name(&scope.name).min(32);
    }
    if generic_source_path_score(&hit.path) >= 0.0 {
        score += 16;
    }
    let text = hit.text.trim();
    if text.contains('(') {
        score += 12;
    }
    if text.contains('.') || text.contains("::") {
        score += 10;
    }
    if text.contains('=') {
        score += 4;
    }
    if text.len() > 220 {
        score = score.saturating_sub(8);
    }
    score
}

fn handle_word(index: &Codebase, args: &Value) -> Result<String> {
    let word = required_str(args, "word")?;
    let max_results = get_usize(args, "max_results")
        .unwrap_or(WORD_DEFAULT_MAX_RESULTS)
        .clamp(1, 1_000);
    let compact = get_bool_default(args, "compact", true);
    let path_glob = get_str(args, "path_glob").or_else(|| get_str(args, "glob"));
    let globset = path_glob.as_deref().map(build_globset).transpose()?;
    let raw_hits = index.word_hits(&word)?;
    let mut scoped_hits = Vec::<SearchHit>::new();
    if raw_hits.len() <= WORD_SCOPE_HIT_LIMIT {
        let mut content_by_file = HashMap::<u32, String>::new();
        for hit in &raw_hits {
            let Some(file) = index.file_by_id(hit.file_id) else {
                continue;
            };
            if globset
                .as_ref()
                .is_some_and(|glob| !glob.is_match(file.path.as_str()))
            {
                continue;
            }
            if !content_by_file.contains_key(&hit.file_id) {
                content_by_file.insert(hit.file_id, index.file_content(file)?);
            }
            let Some(content) = content_by_file.get(&hit.file_id) else {
                continue;
            };
            let line = hit.line as usize;
            let Some(text) = content.lines().nth(line.saturating_sub(1)) else {
                continue;
            };
            scoped_hits.push(SearchHit {
                path: file.path.clone(),
                line,
                text: text.trim().to_string(),
                scope: scope_for_line(&file.symbols, line),
            });
        }
    }
    if !scoped_hits.is_empty() {
        let hits = if compact {
            diversify_search_hits(index, &word, scoped_hits, max_results)
        } else {
            scoped_hits.into_iter().take(max_results).collect()
        };
        if get_str(args, "format").as_deref() == Some("json") {
            return format_json_line_hits(&word, hits, false, 0, false);
        }
        return Ok(format_line_hits(&word, hits, compact));
    }

    let mut out = format!("{} hits for '{}':\n", raw_hits.len(), word);
    if raw_hits.len() > WORD_SCOPE_HIT_LIMIT {
        out.push_str(&format!(
            "  [too many exact word hits for scoped preview; showing first {max_results} path lines]\n"
        ));
    }
    for hit in raw_hits.iter().take(max_results) {
        if let Some(file) = index.file_by_id(hit.file_id) {
            if globset
                .as_ref()
                .is_some_and(|glob| !glob.is_match(file.path.as_str()))
            {
                continue;
            }
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
            if candidates.len() <= CALLERS_AMBIGUOUS_AUTO_LIMIT {
                return format_ambiguous_callers_references(index, &name, candidates, max_results);
            }
            return Ok(format_ambiguous_callers_target(&name, candidates));
        }
    };
    let all_definition_sites = index
        .symbols_named(&name)
        .into_iter()
        .map(|(file, symbol)| (file.path.clone(), symbol.line_start))
        .collect::<BTreeSet<_>>();
    let mut hits = reference_candidates(index, &name)?;
    hits.retain(|hit| !all_definition_sites.contains(&(hit.path.clone(), hit.line)));
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
    reference_candidates_with_limit(index, name, None)
}

fn reference_candidates_with_limit(
    index: &Codebase,
    name: &str,
    max_word_hits: Option<usize>,
) -> Result<Vec<SearchHit>> {
    let word_hits = index.word_hits(name)?;
    if word_hits.is_empty() {
        return Ok(Vec::new());
    };
    if max_word_hits.is_some_and(|max| word_hits.len() > max) {
        return Ok(Vec::new());
    }
    let mut content_by_file = HashMap::<u32, (String, String)>::new();
    let mut results = Vec::new();
    for hit in &word_hits {
        let Some(file) = index.file_by_id(hit.file_id) else {
            continue;
        };
        if !content_by_file.contains_key(&hit.file_id) {
            let content = index.file_content(file)?;
            let active_content = mask_comments(file.language.as_str(), &content);
            content_by_file.insert(hit.file_id, (content, active_content));
        }
        if let Some((content, active_content)) = content_by_file.get(&hit.file_id) {
            let line = hit.line as usize;
            let Some(active_text) = active_content.lines().nth(line.saturating_sub(1)) else {
                continue;
            };
            if !raw_identifiers(active_text)
                .into_iter()
                .any(|identifier| identifier == name)
            {
                continue;
            }
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

fn format_ambiguous_callers_references(
    index: &Codebase,
    name: &str,
    mut candidates: Vec<SymbolTarget>,
    max_results: usize,
) -> Result<String> {
    candidates.sort_by(|a, b| {
        a.path
            .cmp(&b.path)
            .then_with(|| a.line_start.cmp(&b.line_start))
    });
    let all_definition_sites = index
        .symbols_named(name)
        .into_iter()
        .map(|(file, symbol)| (file.path.clone(), symbol.line_start))
        .collect::<BTreeSet<_>>();
    let mut hits = reference_candidates(index, name)?;
    hits.retain(|hit| !all_definition_sites.contains(&(hit.path.clone(), hit.line)));
    hits.truncate(max_results);

    let mut out = format!(
        "ambiguous symbol '{name}': {} definitions; returning shared references. Pass definition_path and definition_line to resolve one target.\n",
        candidates.len()
    );
    out.push_str("definitions:\n");
    for candidate in &candidates {
        out.push_str(&format!(
            "  {}:{} ({})  // {}\n",
            candidate.path, candidate.line_start, candidate.kind, candidate.detail
        ));
    }
    out.push_str(&format!(
        "{} shared references for '{name}' across ambiguous definitions:\n",
        hits.len()
    ));
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
    if args.get("paths").is_some() {
        let Some(items) = args.get("paths").and_then(Value::as_array) else {
            return Ok("error: 'paths' must be an array".to_string());
        };
        return handle_read_batch(index, args, items);
    }
    handle_read_one(index, args)
}

fn handle_read_batch(index: &Codebase, base_args: &Value, items: &[Value]) -> Result<String> {
    if items.is_empty() {
        return Ok("error: 'paths' must not be empty".to_string());
    }
    let mut out = format!(
        "{} codedb_read batch items:\n",
        items.len().min(MAX_BATCH_ITEMS)
    );
    for (idx, item) in items.iter().take(MAX_BATCH_ITEMS).enumerate() {
        let args = batch_item_args(base_args, "paths", item, "path")?;
        let path = get_str(&args, "path").unwrap_or_default();
        out.push_str(&format!("--- [{idx}] codedb_read: {path} ---\n"));
        out.push_str(&handle_read_one(index, &args)?);
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

fn handle_read_one(index: &Codebase, args: &Value) -> Result<String> {
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
    let requested_compact = get_bool(args, "compact");
    let connected_range = get_bool(args, "connected_range");
    let start = get_usize(args, "line_start").unwrap_or(1);
    let requested_end = get_usize(args, "line_end").unwrap_or(file.line_count.max(1));
    let has_explicit_range = args.get("line_start").is_some() || args.get("line_end").is_some();
    if connected_range && !has_explicit_range {
        return Ok("error: connected_range=true requires line_start and line_end".to_string());
    }
    let requested_span = requested_end.saturating_sub(start).saturating_add(1);
    let forced_compact_range =
        !requested_compact && has_explicit_range && requested_span > READ_FULL_RANGE_MAX_LINES;
    let compact = requested_compact || forced_compact_range || connected_range;
    let end = requested_end;
    if start == 0 || end == 0 {
        return Ok("error: line_start and line_end must be >= 1".to_string());
    }
    if start > end {
        return Ok(format!("error: line_start ({start}) > line_end ({end})"));
    }
    let compact_content = compact.then(|| mask_comments(file.language.as_str(), &content));
    let (compact_body, compact_truncated) = if let Some(compact_content) =
        compact_content.as_deref()
    {
        if connected_range {
            (
                Some(extract_lines(compact_content, start, end, true)),
                false,
            )
        } else {
            let (body, truncated) =
                extract_lines_limited(compact_content, start, end, true, READ_COMPACT_MAX_LINES);
            (Some(body), truncated)
        }
    } else {
        (None, false)
    };
    let mut out = format!("hash:{hash}\n");
    if forced_compact_range {
        out.push_str(&format!(
            "[full read range exceeds {READ_FULL_RANGE_MAX_LINES} lines; returned compact output. Use codedb_symbol body=true for named bodies or a smaller range when exact formatting is required.]\n"
        ));
    }
    if compact_truncated {
        out.push_str(&format!(
            "[compact read capped at {READ_COMPACT_MAX_LINES} content lines; requested L{start}-L{requested_end}]\n"
        ));
    }
    if start != 1 || end != file.line_count || compact {
        if let Some(body) = compact_body {
            out.push_str(&body);
        } else {
            out.push_str(&extract_lines(&content, start, end, compact));
        }
    } else {
        out.push_str(&content);
    }
    if connected_range {
        let members = file
            .symbols
            .iter()
            .filter(|symbol| {
                symbol.line_start >= start
                    && symbol.line_end.max(symbol.line_start) <= end
                    && matches!(
                        symbol.kind.as_str(),
                        "method" | "function" | "constructor" | "procedure" | "macro" | "property"
                    )
            })
            .map(|symbol| symbol.name.clone())
            .collect::<Vec<_>>();
        out.push_str(&format!(
            "connected range closure: active code for L{start}-L{end} is complete; contained members=[{}]. Do not reopen these members individually with codedb_symbol or overlapping reads unless this range contradicts another exact body. Follow only a cross-file handoff still required by the task.\n",
            members.join(", ")
        ));
    }
    if connected_range || get_bool(args, "include_symbol_leads") {
        append_read_symbol_leads(
            index,
            file,
            &content,
            start,
            end,
            has_explicit_range,
            requested_span,
            connected_range,
            &mut out,
        )?;
    }
    Ok(out)
}

fn append_read_symbol_leads(
    index: &Codebase,
    file: &FileEntry,
    content: &str,
    start: usize,
    end: usize,
    has_explicit_range: bool,
    requested_span: usize,
    connected_range: bool,
    out: &mut String,
) -> Result<()> {
    if !has_explicit_range {
        return Ok(());
    }
    let wide_range = requested_span > READ_SYMBOL_LEAD_MAX_RANGE_LINES;
    let mut symbols = file
        .symbols
        .iter()
        .filter(|symbol| is_context_handoff_source_symbol(symbol))
        .filter(|symbol| symbol.line_start >= start && symbol.line_start <= end)
        .filter(|symbol| {
            let symbol_end = symbol.line_end.max(symbol.line_start);
            ranges_overlap(start, end, symbol.line_start, symbol_end)
                && symbol_end.saturating_sub(symbol.line_start)
                    < CONTEXT_SYMBOL_HANDOFF_MAX_SOURCE_LINES
        })
        .collect::<Vec<_>>();
    if symbols.is_empty() || (!wide_range && symbols.len() > READ_SYMBOL_LEAD_MAX_SYMBOLS) {
        return Ok(());
    }
    if connected_range {
        append_connected_range_handoff_frontier(index, file, content, &symbols, out);
        append_connected_range_incoming_frontier(index, file, &symbols, out)?;
        return Ok(());
    }
    symbols.sort_by(|left, right| {
        symbol_kind_lead_weight(right)
            .cmp(&symbol_kind_lead_weight(left))
            .then_with(|| {
                symbol_name_specificity_weight(right).cmp(&symbol_name_specificity_weight(left))
            })
            .then_with(|| left.line_start.cmp(&right.line_start))
    });
    let mut section = String::new();
    let emit_limit = if wide_range {
        READ_WIDE_SYMBOL_LEAD_EMIT_SYMBOLS
    } else {
        READ_SYMBOL_LEAD_EMIT_SYMBOLS
    };
    for symbol in symbols.into_iter().take(emit_limit) {
        let symbol_end = symbol.line_end.max(symbol.line_start);
        let body = source_line_slice(content, symbol.line_start, symbol_end);
        if section.is_empty() {
            section.push_str("read range symbol leads (lightweight; use codedb_symbol body=true for deep body-reference evidence instead of repeating this read):\n");
        }
        section.push_str(&format!(
            "  {}:L{} {} ({})\n",
            file.path, symbol.line_start, symbol.name, symbol.kind
        ));
        if !connected_range {
            section.push_str(&format!(
                "    follow-up: codedb_symbol name={} path={} body=true max_results=1\n",
                symbol.name, file.path
            ));
        }
        let evidence = symbol_body_primary_evidence(index, file, symbol, &body);
        let mut leads = evidence
            .qualified
            .into_iter()
            .chain(evidence.flow)
            .collect::<Vec<_>>();
        leads.sort_by(|left, right| {
            left.line
                .cmp(&right.line)
                .then_with(|| left.target.path.cmp(&right.target.path))
                .then_with(|| left.target.line_start.cmp(&right.target.line_start))
        });
        leads.dedup_by(|left, right| {
            left.line == right.line
                && left.target.path == right.target.path
                && left.target.line_start == right.target.line_start
                && left.target.name == right.target.name
        });
        for lead in leads.into_iter().take(READ_SYMBOL_LEAD_HANDOFF_LIMIT) {
            section.push_str(&format!(
                "    handoff candidate: L{} {} -> {}:{} ({}) // {}\n",
                lead.line,
                lead.target.name,
                lead.target.path,
                lead.target.line_start,
                lead.target.kind,
                compact_inline_text(&lead.text, 160)
            ));
        }
    }
    out.push_str(&section);
    Ok(())
}

fn append_connected_range_handoff_frontier(
    index: &Codebase,
    file: &FileEntry,
    content: &str,
    symbols: &[&Symbol],
    out: &mut String,
) {
    let mut symbols = symbols.to_vec();
    symbols.sort_by_key(|symbol| symbol.line_start);
    let mut frontier = BTreeMap::<(usize, String), (&Symbol, BodyFlowHandoffLead, usize)>::new();
    for symbol in symbols {
        let symbol_end = symbol.line_end.max(symbol.line_start);
        let body = source_line_slice(content, symbol.line_start, symbol_end);
        let evidence = symbol_body_primary_evidence(index, file, symbol, &body);
        for lead in evidence.qualified.into_iter().chain(evidence.flow) {
            if lead.target.path == file.path {
                continue;
            }
            let boundary_score = flow_handoff_line_score(&lead.text);
            let rank = lead.score.saturating_add(boundary_score * 4);
            let key = (symbol.line_start, lead.target.path.clone());
            match frontier.get(&key) {
                Some((_, current, current_rank))
                    if *current_rank > rank
                        || (*current_rank == rank && current.line <= lead.line) => {}
                _ => {
                    frontier.insert(key, (symbol, lead, rank));
                }
            }
        }
    }
    if frontier.is_empty() {
        return;
    }
    let mut frontier = frontier.into_values().collect::<Vec<_>>();
    frontier.sort_by(|left, right| {
        left.0
            .line_start
            .cmp(&right.0.line_start)
            .then_with(|| left.1.line.cmp(&right.1.line))
            .then_with(|| left.1.target.path.cmp(&right.1.target.path))
    });
    out.push_str("connected range cross-file handoff frontier (one strongest graph/data-flow boundary per member and target file; no task keywords are used):\n");
    for (symbol, lead, _) in frontier {
        let boundary_score = flow_handoff_line_score(&lead.text);
        let boundary = if boundary_score >= 55 {
            "value/control boundary"
        } else {
            "direct handoff"
        };
        out.push_str(&format!(
            "  {} L{} -> L{} {}: {} -> {}:{} ({}) // {}\n",
            symbol.name,
            symbol.line_start,
            lead.line,
            boundary,
            lead.target.name,
            lead.target.path,
            lead.target.line_start,
            lead.target.kind,
            compact_inline_text(&lead.text, 180)
        ));
        if boundary_score >= 55 {
            out.push_str("    required before interpreting the returned/selected data: ");
        } else {
            out.push_str("    follow-up when the callee contract matters: ");
        }
        out.push_str(&format!(
            "codedb_symbol name={} path={} body=true max_results=1\n",
            lead.target.name, lead.target.path
        ));
    }
}

#[allow(dead_code)]
fn append_contracted_leaf_corridor(index: &Codebase, target: &SymbolTarget, out: &mut String) {
    let Ok(chain) = continue_symbol_target(index, target.clone(), 0) else {
        return;
    };
    let terminal = chain.steps.last().unwrap_or(&chain.source);
    let dispatch_candidates = symbol_target_dispatch_candidates(index, terminal);
    if chain.steps.is_empty() && dispatch_candidates.is_empty() {
        return;
    }
    out.push_str("    contracted leaf corridor (degree-1 forwarding path; no task keywords): ");
    out.push_str(&format!(
        "{}:{} {}",
        chain.source.path, chain.source.line_start, chain.source.name
    ));
    for step in &chain.steps {
        out.push_str(&format!(
            " -> {}:{} {}",
            step.path, step.line_start, step.name
        ));
    }
    out.push('\n');
    append_continuation_terminal_evidence(index, &chain, false, out);
}

fn append_connected_range_incoming_frontier(
    index: &Codebase,
    file: &FileEntry,
    symbols: &[&Symbol],
    out: &mut String,
) -> Result<()> {
    let mut incoming = Vec::<(&Symbol, SearchHit, Option<String>)>::new();
    let mut content_by_path = HashMap::<String, String>::new();
    for symbol in symbols {
        if symbol.name.len() < 4 || index.symbols_named(&symbol.name).len() != 1 {
            continue;
        }
        let mut by_source_file = BTreeMap::<String, SearchHit>::new();
        for hit in reference_candidates(index, &symbol.name)? {
            if hit.path == file.path
                || !hit.scope.as_ref().is_some_and(|scope| {
                    matches!(
                        scope.kind.as_str(),
                        "method" | "function" | "constructor" | "macro"
                    )
                })
            {
                continue;
            }
            let code_line = strip_strings_and_line_comment(&hit.text);
            if identifier_call_argument_counts(&code_line, &symbol.name).is_empty() {
                continue;
            }
            by_source_file
                .entry(hit.path.clone())
                .and_modify(|current| {
                    if hit.line < current.line {
                        *current = hit.clone();
                    }
                })
                .or_insert(hit);
        }
        for hit in by_source_file.into_values() {
            if !content_by_path.contains_key(&hit.path)
                && let Some(source_file) = index.file(&hit.path)
            {
                content_by_path.insert(hit.path.clone(), index.file_content(source_file)?);
            }
            let guard = content_by_path
                .get(&hit.path)
                .and_then(|content| preprocessor_guard_at_line(content, hit.line));
            incoming.push((symbol, hit, guard));
        }
    }
    if incoming.is_empty() {
        return Ok(());
    }
    incoming.sort_by(|left, right| {
        left.0
            .line_start
            .cmp(&right.0.line_start)
            .then_with(|| left.1.path.cmp(&right.1.path))
            .then_with(|| left.1.line.cmp(&right.1.line))
    });
    out.push_str("connected range incoming call frontier (exact external callers; guards apply only at the shown call site):\n");
    for (symbol, hit, guard) in incoming {
        let guard = guard.unwrap_or_else(|| "no enclosing preprocessor guard".to_string());
        out.push_str(&format!(
            "  {} L{} <- {}:{} [{}] // {}\n",
            symbol.name,
            symbol.line_start,
            hit.path,
            hit.line,
            guard,
            compact_inline_text(&hit.text, 180)
        ));
    }
    Ok(())
}

fn preprocessor_guard_at_line(content: &str, line: usize) -> Option<String> {
    let mut stack = Vec::<String>::new();
    for source_line in content.lines().take(line) {
        let trimmed = source_line.trim();
        if trimmed.starts_with("#if ") || trimmed == "#if" {
            stack.push(trimmed.to_string());
        } else if trimmed.starts_with("#elif ") || trimmed == "#else" {
            if let Some(active) = stack.last_mut() {
                *active = trimmed.to_string();
            }
        } else if trimmed == "#endif" {
            stack.pop();
        }
    }
    (!stack.is_empty()).then(|| stack.join(" && "))
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
        "codedb status:\n  seq: {}\n  files: {}\n  outlines: {}\n  chunks: {}\n  graph: {} nodes, {} edges, {} communities\n  retrieval: property graph query + lazy semantic expansion\n  scan: {}\n  extensions: {}\n  cache: {}\n  storage: {}\n",
        stats.seq,
        stats.files,
        stats.files,
        stats.chunks,
        stats.graph_nodes,
        stats.graph_edges,
        stats.graph_communities,
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
    let include_symbols = get_bool_default(args, "include_symbols", true);
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
        out.push_str(&format!("{}. {} (score: {:.2})", idx + 1, path, score));
        if include_symbols
            && idx < FIND_SYMBOL_SUMMARY_RESULT_LIMIT
            && let Some(file) = index.file(&path)
        {
            let symbols = compact_file_symbol_summary(file, FIND_SYMBOL_SUMMARY_PER_FILE);
            if !symbols.is_empty() {
                out.push_str(&format!(" symbols: {}", symbols.join("; ")));
            }
        }
        out.push('\n');
    }
    Ok(out)
}

fn compact_file_symbol_summary(file: &FileEntry, limit: usize) -> Vec<String> {
    compact_file_symbols(file, limit)
        .into_iter()
        .map(|symbol| format!("L{} {} {}", symbol.line_start, symbol.kind, symbol.name))
        .collect()
}

fn compact_file_symbols(file: &FileEntry, limit: usize) -> Vec<&Symbol> {
    if limit == 0 {
        return Vec::new();
    }

    let mut selected = Vec::<&Symbol>::new();
    let mut seen = BTreeSet::<(usize, String, String)>::new();

    let type_quota = limit.min(1);
    for symbol in file.symbols.iter().filter(|symbol| is_type_symbol(symbol)) {
        if selected.len() >= type_quota {
            break;
        }
        push_compact_symbol(&mut selected, &mut seen, symbol, limit);
    }
    let state_quota = state_symbol_summary_quota(limit);
    for symbol in file
        .symbols
        .iter()
        .filter(|symbol| is_state_summary_symbol(symbol))
    {
        if selected.len() >= type_quota.saturating_add(state_quota) {
            break;
        }
        push_compact_symbol(&mut selected, &mut seen, symbol, limit);
    }
    let executable_symbols = file
        .symbols
        .iter()
        .filter(|symbol| is_executable_symbol(symbol))
        .collect::<Vec<_>>();
    for symbol in executable_symbols.iter().take(1) {
        push_compact_symbol(&mut selected, &mut seen, symbol, limit);
    }
    for target_line in symbol_sample_target_lines(file.line_count) {
        push_nearest_compact_symbol(
            &mut selected,
            &mut seen,
            &executable_symbols,
            target_line,
            limit,
        );
    }
    let tail_quota = (limit / 2).clamp(2, 6);
    for symbol in executable_symbols.iter().rev().take(tail_quota) {
        push_compact_symbol(&mut selected, &mut seen, symbol, limit);
    }
    for symbol in executable_symbols {
        push_compact_symbol(&mut selected, &mut seen, symbol, limit);
    }
    for symbol in &file.symbols {
        push_compact_symbol(&mut selected, &mut seen, symbol, limit);
    }

    selected.sort_by(|left, right| {
        left.line_start
            .cmp(&right.line_start)
            .then_with(|| left.name.cmp(&right.name))
    });
    selected.truncate(limit);
    selected
}

fn symbol_sample_target_lines(line_count: usize) -> Vec<usize> {
    if line_count == 0 {
        return Vec::new();
    }
    [1usize, 2, 3]
        .into_iter()
        .map(|part| (line_count * part).div_ceil(4))
        .collect()
}

fn push_nearest_compact_symbol<'a>(
    selected: &mut Vec<&'a Symbol>,
    seen: &mut BTreeSet<(usize, String, String)>,
    symbols: &[&'a Symbol],
    target_line: usize,
    limit: usize,
) {
    if selected.len() >= limit {
        return;
    }
    if let Some(symbol) = symbols
        .iter()
        .min_by(|left, right| {
            left.line_start
                .abs_diff(target_line)
                .cmp(&right.line_start.abs_diff(target_line))
                .then_with(|| left.line_start.cmp(&right.line_start))
                .then_with(|| left.name.cmp(&right.name))
        })
        .copied()
    {
        push_compact_symbol(selected, seen, symbol, limit);
    }
}

fn push_compact_symbol<'a>(
    selected: &mut Vec<&'a Symbol>,
    seen: &mut BTreeSet<(usize, String, String)>,
    symbol: &'a Symbol,
    limit: usize,
) {
    if selected.len() >= limit {
        return;
    }
    let key = (
        symbol.line_start,
        symbol.kind.as_str().to_string(),
        symbol.name.clone(),
    );
    if seen.insert(key) {
        selected.push(symbol);
    }
}

fn file_graph_degree(index: &Codebase, path: &str) -> usize {
    index.deps_for(path).len() + index.reverse_deps_for(path).len()
}

fn is_type_symbol(symbol: &Symbol) -> bool {
    matches!(
        symbol.kind.as_str(),
        "class" | "interface" | "struct" | "enum" | "record" | "trait" | "module" | "impl"
    )
}

fn is_executable_symbol(symbol: &Symbol) -> bool {
    matches!(
        symbol.kind.as_str(),
        "method" | "function" | "constructor" | "macro"
    )
}

fn executable_symbol_count(file: &FileEntry) -> usize {
    file.symbols
        .iter()
        .filter(|symbol| is_executable_symbol(symbol))
        .count()
}

fn graph_ranked_paths(index: &Codebase, paths: &[String], limit: usize) -> Vec<String> {
    let mut ranked = paths
        .iter()
        .filter_map(|path| {
            let file = index.file(path)?;
            let score = file_graph_degree(index, path) * 4
                + executable_symbol_count(file) * 3
                + file.symbols.len().min(24);
            Some((score, path.clone()))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    ranked
        .into_iter()
        .take(limit)
        .map(|(_, path)| path)
        .collect()
}

#[derive(Debug, Clone, Copy)]
struct ContextRange {
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
struct ContextCandidate {
    path: String,
    score: f32,
    reasons: BTreeSet<String>,
    graph_sources: BTreeSet<String>,
    ranges: Vec<ContextRange>,
    hit_lines: BTreeSet<usize>,
}

impl ContextCandidate {
    fn new(path: String) -> Self {
        Self {
            path,
            score: 0.0,
            reasons: BTreeSet::new(),
            graph_sources: BTreeSet::new(),
            ranges: Vec::new(),
            hit_lines: BTreeSet::new(),
        }
    }
}

#[derive(Clone)]
struct ContextGraphNeighbor {
    path: String,
    direction: &'static str,
    score: f32,
}

#[derive(Clone)]
struct GraphFlowSeed {
    rank: usize,
    priority: f32,
    reach: usize,
    role: &'static str,
    path: String,
    community: Option<usize>,
}

#[derive(Clone)]
struct ContextGraphTrail {
    path: String,
    via: String,
    direction: &'static str,
    distance: usize,
    score: f32,
}

#[derive(Clone)]
struct ContextFlowSymbol {
    rank: usize,
    order: usize,
    score: f32,
    target: SymbolTarget,
}

struct ContextFlowFileSymbol<'a> {
    symbol: &'a Symbol,
    score: f32,
    order: usize,
}

struct ContextFlowSymbolEdge {
    source: SymbolTarget,
    target: SymbolTarget,
    score: f32,
    reason: String,
}

struct ContextFlowDataTypeLead {
    source: SymbolTarget,
    target: SymbolTarget,
    order: usize,
}

#[derive(Clone)]
struct ContextFlowSpineCandidate {
    target: SymbolTarget,
    score: f32,
    direct: bool,
    linked_root: bool,
    split_family: bool,
}

struct ContextFlowFileEdge {
    source_path: String,
    target_path: String,
    direction: &'static str,
    score: f32,
    reason: String,
}

#[derive(Clone)]
struct ContextFlowTrace {
    steps: Vec<SymbolTarget>,
    score: f32,
    reason: String,
}

struct ContextFlowQuality {
    score: f32,
    high_confidence: bool,
    corroborated_candidate_count: usize,
    candidate_count: usize,
    symbol_count: usize,
    symbol_edge_count: usize,
    trace_count: usize,
    strong_trace_count: usize,
    data_type_lead_count: usize,
    file_edge_count: usize,
}

fn handle_context(index: &Codebase, args: &Value) -> Result<String> {
    let query = get_str(args, "task")
        .or_else(|| get_str(args, "query"))
        .ok_or_else(|| anyhow!("missing 'task'"))?;
    if query.trim().is_empty() {
        return Ok("error: empty task - pass a non-empty 'task' string".to_string());
    }
    let max_tokens = get_usize(args, "max_tokens");
    let requested_max_files = get_usize(args, "max_files")
        .or_else(|| get_usize(args, "max_results"))
        .unwrap_or_else(|| {
            max_tokens
                .map(context_max_files_for_token_budget)
                .unwrap_or(CONTEXT_DEFAULT_MAX_FILES)
        });
    let max_files = max_tokens
        .map(context_max_files_for_token_budget)
        .map(|token_files| requested_max_files.min(token_files))
        .unwrap_or(requested_max_files)
        .max(1);
    let path_glob = get_str(args, "path_glob");
    let include_inventory = get_bool_default(args, "include_inventory", false);
    let include_deps = get_bool_default(args, "include_deps", true);
    let include_snippets = get_bool_default(args, "include_snippets", false);
    let snippet_radius = get_usize(args, "snippet_radius")
        .unwrap_or(CONTEXT_DEFAULT_SNIPPET_RADIUS)
        .clamp(0, CONTEXT_MAX_SNIPPET_RADIUS);
    let requested_snippets_per_file = get_usize(args, "snippets_per_file")
        .unwrap_or(CONTEXT_DEFAULT_SNIPPETS_PER_FILE)
        .clamp(0, CONTEXT_MAX_SNIPPETS_PER_FILE);
    let snippets_per_file =
        context_snippets_per_file_for_budget(requested_snippets_per_file, max_files, max_tokens);
    let max_chars = get_usize(args, "max_chars")
        .or_else(|| max_tokens.map(|tokens| tokens.saturating_mul(3)))
        .unwrap_or(CONTEXT_DEFAULT_MAX_CHARS)
        .max(2_000);
    if path_glob.is_none() {
        let mut out = format!("codedb_context '{}'\n", query);
        out.push_str(
            "retrieval: graph atlas only; task text is an opaque label and is not analyzed.\n",
        );
        append_context_module_inventory(index, &mut out);
        out.push_str("next: choose one listed leaf child and call codedb_context with path_glob=\"<parent>/<child>/**\". Do not scope to the broad parent when a listed child covers the phase; use another leaf-scoped call for another phase.\n");
        finalize_context_output(&mut out, max_chars);
        return Ok(out);
    }
    let candidates = collect_graph_flow_candidates(index, path_glob.as_deref(), max_files)?;
    if candidates.is_empty() {
        return Ok(format!("no context candidates for: {query}"));
    }

    let mut out = format!("codedb_context '{}' files={}\n", query, candidates.len());
    let body_lead_query_terms = Vec::<String>::new();
    out.push_str(
        "retrieval: scoped structural roots -> graph-community boundaries -> weighted dependency bridges -> call expansion; task text is not analyzed.\n",
    );
    append_context_flow_pack(index, &candidates, &query, &[], 0, 0, &mut out)?;
    if include_inventory {
        append_context_module_inventory(index, &mut out);
    }
    append_context_candidate_overview(index, &candidates, "", &mut out);
    if include_deps {
        append_context_graph_trails(index, &candidates, "", &mut out, CONTEXT_GRAPH_TRAIL_LIMIT);
    }
    let mut seen_handoff_leads = BTreeSet::<(String, usize, String)>::new();
    let mut emitted_handoff_leads = 0usize;
    for (idx, candidate) in candidates.iter().enumerate() {
        if out.len() >= max_chars {
            out.push_str("\n[context budget reached; increase max_tokens for more files]\n");
            break;
        }
        let Some(file) = index.file(&candidate.path) else {
            continue;
        };
        out.push_str(&format!(
            "\n{}. {} ({} {}L {}sym) s={:.3}\n",
            idx + 1,
            file.path,
            file.language,
            file.line_count,
            file.symbols.len(),
            candidate.score
        ));
        out.push_str(&format!(
            "   why:{}\n",
            candidate
                .reasons
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let graph_sources = context_candidate_graph_sources(candidate);
        if !graph_sources.is_empty() {
            out.push_str(&format!(
                "   graph:{}\n",
                graph_sources
                    .iter()
                    .take(4)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !candidate.hit_lines.is_empty() {
            out.push_str(&format!(
                "   hits:{}\n",
                candidate
                    .hit_lines
                    .iter()
                    .take(8)
                    .map(|line| line.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let symbols = selected_candidate_symbols(file, candidate, "", 6);
        if !symbols.is_empty() {
            out.push_str("   symbols:\n");
            for symbol in &symbols {
                out.push_str(&format!(
                    "     L{}-L{} {} {} // {}\n",
                    symbol.line_start, symbol.line_end, symbol.kind, symbol.name, symbol.detail
                ));
            }
            append_context_symbol_handoff_leads(
                index,
                file,
                &symbols,
                &body_lead_query_terms,
                &mut seen_handoff_leads,
                &mut emitted_handoff_leads,
                &mut out,
            )?;
        }
        if include_deps {
            append_compact_deps(index, &candidate.path, "", &mut out, "   ");
        }
        if include_snippets && snippets_per_file > 0 {
            let remaining_chars = max_chars.saturating_sub(out.len());
            if remaining_chars > 200 {
                append_context_snippets(
                    index,
                    file,
                    candidate,
                    &mut out,
                    snippet_radius,
                    snippets_per_file,
                    remaining_chars,
                )?;
            }
        }
    }
    finalize_context_output(&mut out, max_chars);
    Ok(out)
}

fn handle_flow(index: &Codebase, args: &Value) -> Result<String> {
    let query = get_str(args, "task")
        .or_else(|| get_str(args, "query"))
        .ok_or_else(|| anyhow!("missing 'task'"))?;
    if query.trim().is_empty() {
        return Ok("error: empty task - pass a non-empty 'task' string".to_string());
    }
    let max_tokens = get_usize(args, "max_tokens");
    let max_files = get_usize(args, "max_files")
        .or_else(|| get_usize(args, "max_results"))
        .unwrap_or(CONTEXT_FLOW_CANDIDATE_LIMIT)
        .max(1);
    let max_chars = get_usize(args, "max_chars")
        .or_else(|| max_tokens.map(|tokens| tokens.saturating_mul(4) / 3))
        .unwrap_or(4_500)
        .max(2_000);
    let path_glob = get_str(args, "path_glob").filter(|glob| !glob.trim().is_empty());
    let include_inventory = get_bool_default(args, "include_inventory", false);
    if path_glob.is_none() {
        let mut out = format!("codedb_flow '{}'\n", query);
        out.push_str(
            "retrieval: graph atlas only; task text is an opaque label and is not analyzed.\n",
        );
        append_context_module_inventory(index, &mut out);
        out.push_str("next: choose one listed leaf child and call codedb_flow with path_glob=\"<parent>/<child>/**\". Do not scope to the broad parent when a listed child covers the phase. A broad path with no listed child group is itself a valid scope. Use another scoped call only for another requested lifecycle phase.\n");
        finalize_context_output(&mut out, max_chars);
        return Ok(out);
    }
    let candidates = collect_graph_flow_candidates(index, path_glob.as_deref(), max_files)?;
    if candidates.is_empty() {
        let mut out = format!("no flow candidates for: {query}\n");
        if include_inventory {
            append_context_module_inventory(index, &mut out);
        }
        finalize_context_output(&mut out, max_chars);
        return Ok(out);
    }

    let mut out = format!("codedb_flow '{}'\n", query);
    out.push_str(
        "retrieval: scoped structural roots -> graph-community boundaries -> weighted dependency bridges -> call expansion; task text is not analyzed.\n",
    );
    append_context_flow_pack(
        index,
        &candidates,
        &query,
        &[],
        0,
        max_chars
            .saturating_mul(45)
            .saturating_div(100)
            .min(CONTEXT_FLOW_SPINE_SOURCE_TOTAL_CHARS),
        &mut out,
    )?;
    if include_inventory {
        append_context_module_inventory(index, &mut out);
    }
    finalize_context_output(&mut out, max_chars);
    Ok(out)
}

fn append_context_flow_pack(
    index: &Codebase,
    candidates: &[ContextCandidate],
    _query: &str,
    _code_terms: &[String],
    _specific_anchor_count: usize,
    spine_source_budget: usize,
    out: &mut String,
) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }
    let query_terms = Vec::<String>::new();
    let flow_symbols = context_flow_symbols(index, candidates, "");
    let symbol_edges = context_flow_symbol_edges(index, &flow_symbols, &query_terms)?;
    let flow_traces =
        collect_context_flow_traces(index, &symbol_edges, &query_terms, &flow_symbols)?;
    let data_type_leads = context_flow_data_type_leads(index, &flow_symbols)?;
    let file_edges = context_flow_file_edges(index, candidates, "");
    let corroborated_candidate_count = candidates
        .iter()
        .filter(|candidate| !candidate.graph_sources.is_empty())
        .count();
    let root_keys = context_flow_root_keys(&flow_symbols);
    let strong_trace_count = flow_traces
        .iter()
        .filter(|trace| context_flow_trace_is_strong(trace, &root_keys, &query_terms))
        .count();
    let quality = context_flow_quality(
        corroborated_candidate_count,
        candidates.len(),
        flow_symbols.len(),
        symbol_edges.len(),
        flow_traces.len(),
        strong_trace_count,
        data_type_leads.len(),
        file_edges.len(),
    );
    out.push_str("pack:\n");
    append_context_flow_quality(&quality, out);
    out.push_str("completion rule: spine source bodies are already read. Answer once every explicitly requested phase or variant has an active body, adjacent phases have direct handoffs or graph paths, and answer-critical filter/fallback/retry/branch semantics have the exact callee body. Direct calls prove handoffs only; do not descend into unrelated generic loaders or re-prove links. Preprocessor guards apply only to enclosed source.\n");
    append_context_flow_candidate_table(index, candidates, "", out)?;
    append_context_flow_spine_source(
        index,
        &flow_symbols,
        &symbol_edges,
        &query_terms,
        true,
        spine_source_budget,
        out,
    );
    append_context_flow_symbol_edges(&symbol_edges, out);
    append_context_flow_traces(&flow_traces, out);
    append_context_flow_trace_previews(index, &flow_traces, out);
    append_context_flow_data_type_leads(&data_type_leads, out);
    append_context_flow_file_edges(&file_edges, out);
    append_context_flow_followups(index, candidates, &flow_symbols, &symbol_edges, out);
    Ok(())
}

fn context_flow_quality(
    corroborated_candidate_count: usize,
    candidate_count: usize,
    symbol_count: usize,
    symbol_edge_count: usize,
    trace_count: usize,
    strong_trace_count: usize,
    data_type_lead_count: usize,
    file_edge_count: usize,
) -> ContextFlowQuality {
    let relation_count = symbol_edge_count + trace_count + data_type_lead_count + file_edge_count;
    let high_confidence =
        strong_trace_count > 0 || (corroborated_candidate_count >= 2 && symbol_edge_count > 0);
    let mut score = 0.0;
    score += (candidate_count.min(6) as f32 / 6.0) * 0.18;
    score += (symbol_count.min(12) as f32 / 12.0) * 0.22;
    score += (symbol_edge_count.min(8) as f32 / 8.0) * 0.22;
    score += (trace_count.min(4) as f32 / 4.0) * 0.18;
    score += (data_type_lead_count.min(4) as f32 / 4.0) * 0.08;
    score += (file_edge_count.min(6) as f32 / 6.0) * 0.12;
    if relation_count == 0 {
        score *= 0.4;
    }
    if !high_confidence {
        score = score.min(0.45);
    }
    ContextFlowQuality {
        score: round2_local(score.min(1.0)),
        high_confidence,
        corroborated_candidate_count,
        candidate_count,
        symbol_count,
        symbol_edge_count,
        trace_count,
        strong_trace_count,
        data_type_lead_count,
        file_edge_count,
    }
}

fn append_context_flow_quality(quality: &ContextFlowQuality, out: &mut String) {
    let mode = if quality.high_confidence {
        "graph_corroborated"
    } else {
        "graph_navigation"
    };
    out.push_str(&format!(
        "confidence={} q={:.2} mode={} connected_files={} files={} sym={} edges={} traces={}/{} data={} file_edges={}\n",
        if quality.high_confidence { "high" } else { "low" },
        quality.score,
        mode,
        quality.corroborated_candidate_count,
        quality.candidate_count,
        quality.symbol_count,
        quality.symbol_edge_count,
        quality.strong_trace_count,
        quality.trace_count,
        quality.data_type_lead_count,
        quality.file_edge_count
    ));
    if !quality.high_confidence {
        out.push_str("caution: selection is structural; treat included active bodies as evidence and use an exact follow-up only for a requested adjacent handoff that is still absent.\n");
    }
}

fn append_context_flow_candidate_table(
    index: &Codebase,
    candidates: &[ContextCandidate],
    query: &str,
    out: &mut String,
) -> Result<()> {
    out.push_str("files:\n");
    out.push_str("#|file|s|rel|why|sym\n");
    for (idx, candidate) in candidates
        .iter()
        .take(CONTEXT_FLOW_CANDIDATE_LIMIT)
        .enumerate()
    {
        let Some(file) = index.file(&candidate.path) else {
            continue;
        };
        let outgoing = index.deps_for(&candidate.path).len();
        let incoming = index.reverse_deps_for(&candidate.path).len();
        let rels = format!("o{outgoing}/i{incoming}");
        let why = candidate
            .reasons
            .iter()
            .take(2)
            .map(|reason| context_flow_cell(reason, 70))
            .collect::<Vec<_>>()
            .join("; ");
        let symbols =
            context_flow_file_symbols(index, file, candidate, query, CONTEXT_FLOW_SYMBOLS_PER_FILE)
                .into_iter()
                .map(|ranked| {
                    format!(
                        "L{} {} {}",
                        ranked.symbol.line_start,
                        ranked.symbol.kind,
                        context_flow_cell(&ranked.symbol.name, 48)
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
        out.push_str(&format!(
            "{}|{}|{:.2}|{}|{}|{}\n",
            idx + 1,
            context_flow_cell(&candidate.path, 96),
            candidate.score,
            rels,
            why,
            symbols
        ));
    }
    let mut structural_rows = Vec::new();
    for candidate in candidates
        .iter()
        .take(CONTEXT_FLOW_STRUCTURAL_FOLLOWUP_FILE_LIMIT)
    {
        let Some(file) = index.file(&candidate.path) else {
            continue;
        };
        let followups = outline_body_followup_candidates(index, file, OUTLINE_BODY_FOLLOWUP_LIMIT)?;
        let executable_followups = followups
            .iter()
            .filter(|followup| {
                matches!(
                    followup.symbol.kind.as_str(),
                    "method" | "function" | "constructor" | "procedure" | "macro"
                )
            })
            .take(CONTEXT_FLOW_STRUCTURAL_FOLLOWUP_PER_FILE_LIMIT)
            .cloned()
            .collect::<Vec<_>>();
        let selected_followups = if executable_followups.is_empty() {
            followups
                .into_iter()
                .take(CONTEXT_FLOW_STRUCTURAL_FOLLOWUP_PER_FILE_LIMIT)
                .collect::<Vec<_>>()
        } else {
            executable_followups
        };
        for followup in selected_followups {
            structural_rows.push((file.path.clone(), followup));
        }
    }
    if !structural_rows.is_empty() {
        out.push_str("structural body followups (inspect before optional branches):\n");
        for (path, followup) in structural_rows {
            out.push_str(&format!(
                "  {}: L{} {} {} in={} out={} exec={} -> codedb_symbol name={} path={} body=true max_results=1\n",
                context_flow_cell(&path, 88),
                followup.symbol.line_start,
                followup.symbol.kind,
                context_flow_cell(&followup.symbol.name, 48),
                followup.incoming,
                followup.outgoing,
                followup.executable_outgoing,
                followup.symbol.name,
                path
            ));
        }
    }
    Ok(())
}

fn context_flow_symbols(
    index: &Codebase,
    candidates: &[ContextCandidate],
    query: &str,
) -> Vec<ContextFlowSymbol> {
    let mut symbols = Vec::new();
    let mut seen = BTreeSet::<(String, usize, String)>::new();
    for (rank, candidate) in candidates
        .iter()
        .take(CONTEXT_FLOW_CANDIDATE_LIMIT)
        .enumerate()
    {
        let Some(file) = index.file(&candidate.path) else {
            continue;
        };
        for ranked in
            context_flow_file_symbols(index, file, candidate, query, CONTEXT_FLOW_SYMBOLS_PER_FILE)
                .into_iter()
        {
            let target = target_from_symbol(file, ranked.symbol);
            if !seen.insert((target.path.clone(), target.line_start, target.name.clone())) {
                continue;
            }
            symbols.push(ContextFlowSymbol {
                rank: rank + 1,
                order: ranked.order,
                score: candidate.score
                    + ranked.score
                    + context_flow_symbol_role_score(ranked.symbol),
                target,
            });
        }
    }
    symbols.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.rank.cmp(&right.rank))
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    symbols
}

fn context_flow_file_symbols<'a>(
    index: &Codebase,
    file: &'a FileEntry,
    candidate: &ContextCandidate,
    query: &str,
    limit: usize,
) -> Vec<ContextFlowFileSymbol<'a>> {
    if limit == 0 {
        return Vec::new();
    }
    let identifiers = context_query_identifiers(query);
    let mut scored = Vec::<ContextFlowFileSymbol<'a>>::new();
    let mut seen = BTreeSet::<usize>::new();

    for symbol in &file.symbols {
        let mut score = context_symbol_score(symbol, &candidate.ranges, &identifiers);
        if context_symbol_relevant_to_candidate(symbol, candidate) {
            score += 10.0;
        }
        if let Some(direct_score) = context_candidate_direct_symbol_score(symbol, candidate) {
            score += direct_score as f32 * 0.05;
        }
        if score > 0.0 && seen.insert(symbol.line_start) {
            scored.push(ContextFlowFileSymbol {
                symbol,
                score,
                order: symbol.line_start,
            });
        }
    }

    if let Ok(graph_symbols) =
        outline_body_followup_candidates(index, file, limit.saturating_mul(3))
    {
        for item in graph_symbols {
            let graph_score = context_flow_local_graph_symbol_score(&item);
            if seen.insert(item.symbol.line_start) {
                scored.push(ContextFlowFileSymbol {
                    symbol: item.symbol,
                    score: graph_score,
                    order: item.symbol.line_start,
                });
                continue;
            }
            if let Some(existing) = scored
                .iter_mut()
                .find(|existing| existing.symbol.line_start == item.symbol.line_start)
            {
                existing.score += graph_score;
            }
        }
    }

    scored.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.order.cmp(&right.order))
            .then_with(|| left.symbol.name.cmp(&right.symbol.name))
    });
    scored = diversify_flow_file_symbols(scored, limit);

    if scored.is_empty() {
        return selected_candidate_symbols(file, candidate, query, limit)
            .into_iter()
            .enumerate()
            .map(|(order, symbol)| ContextFlowFileSymbol {
                symbol,
                score: context_flow_symbol_role_score(symbol),
                order,
            })
            .collect();
    }
    scored
}

fn diversify_flow_file_symbols<'a>(
    scored: Vec<ContextFlowFileSymbol<'a>>,
    limit: usize,
) -> Vec<ContextFlowFileSymbol<'a>> {
    let mut selected = Vec::new();
    let mut deferred = Vec::new();
    let mut names = BTreeSet::new();
    for item in scored {
        let name = item.symbol.name.to_ascii_lowercase();
        if selected.len() < limit && names.insert(name) {
            selected.push(item);
        } else {
            deferred.push(item);
        }
    }
    for item in deferred {
        if selected.len() >= limit {
            break;
        }
        selected.push(item);
    }
    selected
}

fn context_flow_local_graph_symbol_score(item: &OutlineBodyFollowupCandidate<'_>) -> f32 {
    (item.score as f32).sqrt()
        + item.incoming.min(16) as f32 * 2.5
        + item.outgoing.min(16) as f32 * 3.5
        + (item.span.min(80) as f32 / 20.0)
}

fn context_flow_symbol_role_score(symbol: &Symbol) -> f32 {
    match symbol.kind.as_str() {
        "function" | "method" | "constructor" => 6.0,
        "class" | "interface" | "struct" | "record" | "module" => 3.0,
        "property" | "field" | "variable" | "const" | "static" | "enum" => 1.5,
        _ => 1.0,
    }
}

fn context_flow_symbol_edges(
    index: &Codebase,
    symbols: &[ContextFlowSymbol],
    query_terms: &[String],
) -> Result<Vec<ContextFlowSymbolEdge>> {
    let mut edges = Vec::<ContextFlowSymbolEdge>::new();
    let mut seen = BTreeSet::<(String, usize, String, String, usize, String)>::new();
    for source in symbols
        .iter()
        .take(CONTEXT_FLOW_CANDIDATE_LIMIT * CONTEXT_FLOW_SYMBOLS_PER_FILE)
    {
        let Some(file) = index.file(&source.target.path) else {
            continue;
        };
        let Some(symbol) = symbol_for_target(file, &source.target) else {
            continue;
        };
        if !is_context_handoff_source_symbol(symbol) {
            continue;
        }
        let symbol_end = symbol.line_end.max(symbol.line_start);
        let span = symbol_end.saturating_sub(symbol.line_start) + 1;
        if span > CONTEXT_FLOW_BODY_SCAN_MAX_LINES {
            continue;
        }
        let content = index.file_content(file)?;
        let body = source_line_slice(&content, symbol.line_start, symbol_end);
        for lead in symbol_body_leads_with_terms(index, file, symbol, &body, 3, query_terms) {
            let key = (
                source.target.path.clone(),
                source.target.line_start,
                source.target.name.clone(),
                lead.target.path.clone(),
                lead.target.line_start,
                lead.target.name.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            let cross_file = if lead.target.path == source.target.path {
                0.0
            } else {
                6.0
            };
            let query_match = lead.query_matches.len() as f32 * 5.0;
            edges.push(ContextFlowSymbolEdge {
                source: source.target.clone(),
                target: lead.target,
                score: source.score + lead.score as f32 * 0.06 + cross_file + query_match,
                reason: if lead.query_matches.is_empty() {
                    "body reference".to_string()
                } else {
                    format!(
                        "body reference matches {}",
                        lead.query_matches
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>()
                            .join("/")
                    )
                },
            });
        }
    }
    edges.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.source.path.cmp(&right.source.path))
            .then_with(|| left.source.line_start.cmp(&right.source.line_start))
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    Ok(edges)
}

fn append_context_flow_symbol_edges(edges: &[ContextFlowSymbolEdge], out: &mut String) {
    if edges.is_empty() {
        return;
    }
    out.push_str("edges:\n");
    out.push_str("from|rel|to|why\n");
    for edge in edges.iter().take(CONTEXT_FLOW_SYMBOL_EDGE_LIMIT) {
        out.push_str(&format!(
            "{}|references|{}|{}\n",
            context_flow_target_cell(&edge.source),
            context_flow_target_cell(&edge.target),
            context_flow_cell(&edge.reason, 72)
        ));
    }
}

fn append_context_flow_traces(traces: &[ContextFlowTrace], out: &mut String) {
    if traces.is_empty() {
        return;
    }
    out.push_str("traces:\n");
    out.push_str("trace|why\n");
    for trace in traces {
        let steps = trace
            .steps
            .iter()
            .map(context_flow_target_cell)
            .collect::<Vec<_>>()
            .join(" -> ");
        out.push_str(&format!(
            "{}|{}\n",
            steps,
            context_flow_cell(&trace.reason, 96)
        ));
    }
}

fn append_context_flow_trace_previews(
    index: &Codebase,
    traces: &[ContextFlowTrace],
    out: &mut String,
) {
    if traces.is_empty() {
        return;
    }
    let mut seen = BTreeSet::<(String, usize, String)>::new();
    let mut section = String::new();
    let mut emitted = 0usize;
    for trace in traces {
        for target in trace.steps.iter().rev().take(2) {
            if emitted >= CONTEXT_FLOW_TRACE_PREVIEW_LIMIT {
                break;
            }
            if !is_previewable_symbol_kind(&target.kind) {
                continue;
            }
            let key = (target.path.clone(), target.line_start, target.name.clone());
            if !seen.insert(key) {
                continue;
            }
            let Some(snippet) = compact_symbol_target_snippet_limited(
                index,
                target,
                CONTEXT_FLOW_TRACE_PREVIEW_MAX_LINES,
                CONTEXT_FLOW_TRACE_PREVIEW_MAX_CHARS,
            ) else {
                continue;
            };
            if section.is_empty() {
                section.push_str("trace previews:\n");
                section.push_str("symbol|preview\n");
            }
            section.push_str(&format!(
                "{}|{}\n",
                context_flow_target_cell(target),
                context_flow_cell(&snippet, CONTEXT_FLOW_TRACE_PREVIEW_MAX_CHARS)
            ));
            emitted += 1;
        }
        if emitted >= CONTEXT_FLOW_TRACE_PREVIEW_LIMIT {
            break;
        }
    }
    out.push_str(&section);
}

fn append_context_flow_spine_source(
    index: &Codebase,
    flow_symbols: &[ContextFlowSymbol],
    symbol_edges: &[ContextFlowSymbolEdge],
    query_terms: &[String],
    _high_confidence: bool,
    source_budget: usize,
    out: &mut String,
) {
    if source_budget == 0 {
        return;
    }
    let mut candidates = BTreeMap::<String, ContextFlowSpineCandidate>::new();
    let root_keys = context_flow_root_keys(flow_symbols);
    let linked_root_keys = symbol_edges
        .iter()
        .filter(|edge| {
            root_keys.contains(&symbol_target_key(&edge.source))
                && root_keys.contains(&symbol_target_key(&edge.target))
        })
        .flat_map(|edge| {
            [
                symbol_target_key(&edge.source),
                symbol_target_key(&edge.target),
            ]
        })
        .collect::<BTreeSet<_>>();
    for symbol in flow_symbols {
        let linked_root = linked_root_keys.contains(&symbol_target_key(&symbol.target));
        let linked_root_bonus = if linked_root { 30.0 } else { 0.0 };
        push_context_flow_spine_candidate(
            &mut candidates,
            symbol.target.clone(),
            240.0 - symbol.rank.min(12) as f32 * 18.0 + linked_root_bonus + symbol.score * 0.01,
            true,
            linked_root,
            false,
            query_terms,
        );
    }
    let source_paths = flow_symbols
        .iter()
        .map(|symbol| symbol.target.path.as_str())
        .collect::<BTreeSet<_>>();
    for file in index.files.values() {
        if source_paths.contains(file.path.as_str())
            || !source_paths
                .iter()
                .any(|source_path| same_path_family(source_path, &file.path))
        {
            continue;
        }
        let mut family_symbols = file
            .symbols
            .iter()
            .map(|symbol| target_from_symbol(file, symbol))
            .filter(|target| is_previewable_symbol_kind(&target.kind))
            .filter_map(|target| {
                let coverage = context_flow_target_query_coverage(&target, query_terms);
                (coverage > 0).then_some((target, coverage))
            })
            .collect::<Vec<_>>();
        family_symbols.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| left.0.line_start.cmp(&right.0.line_start))
        });
        for (target, _) in family_symbols.into_iter().take(2) {
            push_context_flow_spine_candidate(
                &mut candidates,
                target,
                35.0,
                false,
                false,
                true,
                query_terms,
            );
        }
    }
    let mut candidates = candidates.into_values().collect::<Vec<_>>();
    apply_context_flow_spine_query_distinctiveness(&mut candidates, query_terms);
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
    let mut ordered = Vec::<ContextFlowSpineCandidate>::new();
    let mut ordered_keys = BTreeSet::<String>::new();
    if !query_terms.is_empty() {
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.linked_root)
            .take(2)
        {
            if ordered_keys.insert(symbol_target_key(&candidate.target)) {
                ordered.push(candidate.clone());
            }
        }
        for candidate in candidates
            .iter()
            .filter(|candidate| candidate.split_family)
            .take(2)
        {
            if ordered_keys.insert(symbol_target_key(&candidate.target)) {
                ordered.push(candidate.clone());
            }
        }
    }
    for candidate in candidates {
        if ordered_keys.insert(symbol_target_key(&candidate.target)) {
            ordered.push(candidate);
        }
    }

    let mut section = String::new();
    let mut emitted = 0usize;
    let mut file_counts = BTreeMap::<String, usize>::new();
    let mut non_direct_file_counts = BTreeMap::<String, usize>::new();
    for candidate in ordered {
        if emitted >= CONTEXT_FLOW_SPINE_SOURCE_LIMIT {
            break;
        }
        let target = candidate.target;
        if file_counts.get(&target.path).copied().unwrap_or(0) >= 2 {
            continue;
        }
        if !candidate.direct
            && non_direct_file_counts
                .get(&target.path)
                .copied()
                .unwrap_or(0)
                >= 1
        {
            continue;
        }
        let Some(body) = complete_active_symbol_body(
            index,
            &target,
            CONTEXT_FLOW_SPINE_SOURCE_MAX_LINES,
            CONTEXT_FLOW_SPINE_SOURCE_MAX_CHARS,
        ) else {
            continue;
        };
        let header = format!(
            "--- {}:L{} {} {}\n",
            target.path, target.line_start, target.kind, target.name
        );
        if section
            .len()
            .saturating_add(header.len())
            .saturating_add(body.len())
            > source_budget.min(CONTEXT_FLOW_SPINE_SOURCE_TOTAL_CHARS)
        {
            continue;
        }
        section.push_str(&header);
        section.push_str(&body);
        *file_counts.entry(target.path.clone()).or_default() += 1;
        if !candidate.direct {
            *non_direct_file_counts
                .entry(target.path.clone())
                .or_default() += 1;
        }
        emitted += 1;
    }
    if emitted > 0 {
        out.push_str(
            "spine source (complete active bodies; comments omitted, line numbers preserved):\n",
        );
        out.push_str(&section);
    }
}

fn apply_context_flow_spine_query_distinctiveness(
    candidates: &mut [ContextFlowSpineCandidate],
    query_terms: &[String],
) {
    if candidates.is_empty() || query_terms.is_empty() {
        return;
    }
    let matches = candidates
        .iter()
        .map(|candidate| {
            matched_identity_terms(
                query_terms,
                &identity_terms_from_text(&candidate.target.name),
            )
        })
        .collect::<Vec<_>>();
    let mut frequencies = BTreeMap::<String, usize>::new();
    for terms in &matches {
        for term in terms {
            *frequencies.entry(term.clone()).or_default() += 1;
        }
    }
    let total = candidates.len() as f32;
    for (candidate, terms) in candidates.iter_mut().zip(matches) {
        for term in terms {
            let frequency = frequencies.get(&term).copied().unwrap_or(1) as f32;
            candidate.score += ((total + 1.0) / (frequency + 1.0)).ln() * 32.0;
        }
    }
}

fn push_context_flow_spine_candidate(
    candidates: &mut BTreeMap<String, ContextFlowSpineCandidate>,
    target: SymbolTarget,
    base_score: f32,
    direct: bool,
    linked_root: bool,
    split_family: bool,
    query_terms: &[String],
) {
    if !is_previewable_symbol_kind(&target.kind) {
        return;
    }
    if query_terms.is_empty()
        && !matches!(
            target.kind.as_str(),
            "method" | "function" | "constructor" | "procedure" | "macro"
        )
    {
        return;
    }
    let name_coverage = context_flow_target_direct_query_coverage(&target, query_terms);
    if !query_terms.is_empty() && name_coverage == 0 {
        return;
    }
    let detail_coverage = context_flow_target_detail_query_coverage(&target, query_terms);
    let path_coverage = context_flow_target_path_query_coverage(&target, query_terms);
    let score = base_score
        + name_coverage as f32 * 60.0
        + detail_coverage as f32 * 8.0
        + path_coverage as f32 * 4.0
        + context_flow_spine_role_score(&target.kind);
    let key = symbol_target_key(&target);
    match candidates.get_mut(&key) {
        Some(existing) => {
            existing.score = existing.score.max(score);
            existing.direct |= direct;
            existing.linked_root |= linked_root;
            existing.split_family |= split_family;
        }
        None => {
            candidates.insert(
                key,
                ContextFlowSpineCandidate {
                    target,
                    score,
                    direct,
                    linked_root,
                    split_family,
                },
            );
        }
    }
}

fn context_flow_target_direct_query_coverage(
    target: &SymbolTarget,
    query_terms: &[String],
) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    matched_identity_terms(query_terms, &identity_terms_from_text(&target.name)).len()
}

fn context_flow_target_detail_query_coverage(
    target: &SymbolTarget,
    query_terms: &[String],
) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    matched_identity_terms(query_terms, &identity_terms_from_text(&target.detail)).len()
}

fn context_flow_target_path_query_coverage(target: &SymbolTarget, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    matched_identity_terms(query_terms, &identity_terms_from_text(&target.path)).len()
}

fn context_flow_target_query_coverage(target: &SymbolTarget, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    let mut identity_terms = identity_terms_from_text(&target.path);
    identity_terms.extend(identity_terms_from_text(&target.name));
    identity_terms.extend(identity_terms_from_text(&target.detail));
    matched_identity_terms(query_terms, &identity_terms).len()
}

fn context_flow_spine_role_score(kind: &str) -> f32 {
    match kind {
        "method" | "function" | "constructor" => 12.0,
        "property" => 10.0,
        "field" | "variable" | "const" | "static" => 5.0,
        _ => 1.0,
    }
}

fn complete_active_symbol_body(
    index: &Codebase,
    target: &SymbolTarget,
    max_lines: usize,
    max_chars: usize,
) -> Option<String> {
    let file = index.file(&target.path)?;
    let symbol = symbol_for_target(file, target)?;
    let start = symbol.line_start;
    let end = symbol.line_end.max(start).min(file.line_count.max(1));
    if end.saturating_sub(start) + 1 > max_lines {
        return None;
    }
    let content = index.file_content(file).ok()?;
    let active_content = mask_comments(file.language.as_str(), &content);
    let mut out = String::new();
    for (idx, line) in active_content.lines().enumerate() {
        let line_no = idx + 1;
        if line_no < start || line_no > end {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("{line_no}: {line}\n"));
        if out.len() > max_chars {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}

fn collect_context_flow_traces(
    index: &Codebase,
    edges: &[ContextFlowSymbolEdge],
    query_terms: &[String],
    flow_symbols: &[ContextFlowSymbol],
) -> Result<Vec<ContextFlowTrace>> {
    let mut traces = Vec::<ContextFlowTrace>::new();
    let mut seen = BTreeSet::<String>::new();
    for edge in edges.iter().take(CONTEXT_FLOW_TRACE_SOURCE_LIMIT) {
        let mut direct_steps = vec![edge.source.clone(), edge.target.clone()];
        push_context_flow_trace(
            &mut traces,
            &mut seen,
            direct_steps.clone(),
            edge.score,
            edge.reason.clone(),
        );
        let next_edges = context_flow_target_reference_edges(
            index,
            &edge.target,
            query_terms,
            CONTEXT_FLOW_TRACE_FANOUT,
        )?;
        for next in next_edges {
            direct_steps.push(next.target.clone());
            let cross_file = usize::from(next.target.path != edge.target.path) as f32 * 5.0;
            push_context_flow_trace(
                &mut traces,
                &mut seen,
                direct_steps.clone(),
                edge.score + next.score * 0.05 + cross_file,
                format!("{}; continuation {}", edge.reason, next.reason),
            );
            direct_steps.pop();
        }
    }
    let root_keys = context_flow_root_keys(flow_symbols);
    Ok(select_context_flow_traces(
        traces,
        &root_keys,
        query_terms,
        CONTEXT_FLOW_TRACE_LIMIT,
    ))
}

fn push_context_flow_trace(
    traces: &mut Vec<ContextFlowTrace>,
    seen: &mut BTreeSet<String>,
    steps: Vec<SymbolTarget>,
    score: f32,
    reason: String,
) {
    if steps.len() < 2 {
        return;
    }
    let key = context_flow_trace_key(&steps);
    if seen.insert(key) {
        traces.push(ContextFlowTrace {
            steps,
            score,
            reason,
        });
    }
}

fn context_flow_trace_key(steps: &[SymbolTarget]) -> String {
    steps
        .iter()
        .map(symbol_target_key)
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn select_context_flow_traces(
    mut traces: Vec<ContextFlowTrace>,
    root_keys: &BTreeSet<String>,
    query_terms: &[String],
    limit: usize,
) -> Vec<ContextFlowTrace> {
    if limit == 0 || traces.is_empty() {
        return Vec::new();
    }
    traces = remove_context_flow_trace_prefixes(traces);
    sort_context_flow_traces(&mut traces, root_keys, query_terms);

    let mut selected = Vec::<ContextFlowTrace>::new();
    let mut seen_full = BTreeSet::<String>::new();
    let mut seen_endpoints = BTreeSet::<String>::new();
    for trace in &traces {
        if selected.len() >= limit {
            break;
        }
        let endpoint_key = context_flow_trace_endpoint_key(&trace.steps);
        if endpoint_key.is_empty() || !seen_endpoints.insert(endpoint_key) {
            continue;
        }
        seen_full.insert(context_flow_trace_key(&trace.steps));
        selected.push(trace.clone());
    }
    for trace in traces {
        if selected.len() >= limit {
            break;
        }
        let full_key = context_flow_trace_key(&trace.steps);
        if seen_full.insert(full_key) {
            selected.push(trace);
        }
    }
    sort_context_flow_traces(&mut selected, root_keys, query_terms);
    selected
}

fn remove_context_flow_trace_prefixes(traces: Vec<ContextFlowTrace>) -> Vec<ContextFlowTrace> {
    let keys = traces
        .iter()
        .map(|trace| {
            trace
                .steps
                .iter()
                .map(symbol_target_key)
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    traces
        .into_iter()
        .enumerate()
        .filter_map(|(idx, trace)| {
            let key = keys.get(idx)?;
            let covered_by_longer = keys
                .iter()
                .enumerate()
                .any(|(other_idx, other)| other_idx != idx && trace_key_is_prefix(key, other));
            if covered_by_longer { None } else { Some(trace) }
        })
        .collect()
}

fn trace_key_is_prefix(prefix: &[String], candidate: &[String]) -> bool {
    prefix.len() < candidate.len() && candidate.starts_with(prefix)
}

fn context_flow_trace_endpoint_key(steps: &[SymbolTarget]) -> String {
    let Some(first) = steps.first() else {
        return String::new();
    };
    let Some(last) = steps.last() else {
        return String::new();
    };
    format!(
        "{} -> {}",
        symbol_target_key(first),
        symbol_target_key(last)
    )
}

fn sort_context_flow_traces(
    traces: &mut [ContextFlowTrace],
    root_keys: &BTreeSet<String>,
    query_terms: &[String],
) {
    traces.sort_by(|left, right| {
        context_flow_trace_is_strong(right, root_keys, query_terms)
            .cmp(&context_flow_trace_is_strong(left, root_keys, query_terms))
            .then_with(|| {
                context_flow_trace_root_count(right, root_keys)
                    .cmp(&context_flow_trace_root_count(left, root_keys))
            })
            .then_with(|| {
                context_flow_trace_query_coverage(right, query_terms)
                    .cmp(&context_flow_trace_query_coverage(left, query_terms))
            })
            .then_with(|| right.steps.len().cmp(&left.steps.len()))
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| {
                context_flow_trace_key(&left.steps).cmp(&context_flow_trace_key(&right.steps))
            })
    });
}

fn context_flow_root_keys(symbols: &[ContextFlowSymbol]) -> BTreeSet<String> {
    symbols
        .iter()
        .map(|symbol| symbol_target_key(&symbol.target))
        .collect()
}

fn context_flow_trace_root_count(trace: &ContextFlowTrace, root_keys: &BTreeSet<String>) -> usize {
    trace
        .steps
        .iter()
        .map(symbol_target_key)
        .filter(|key| root_keys.contains(key))
        .collect::<BTreeSet<_>>()
        .len()
}

fn context_flow_trace_query_coverage(trace: &ContextFlowTrace, query_terms: &[String]) -> usize {
    if query_terms.is_empty() {
        return 0;
    }
    let mut identity_terms = BTreeSet::new();
    for target in &trace.steps {
        identity_terms.extend(identity_terms_from_text(&target.path));
        identity_terms.extend(identity_terms_from_text(&target.name));
        identity_terms.extend(identity_terms_from_text(&target.detail));
    }
    matched_identity_terms(query_terms, &identity_terms).len()
}

fn context_flow_trace_is_strong(
    trace: &ContextFlowTrace,
    root_keys: &BTreeSet<String>,
    query_terms: &[String],
) -> bool {
    context_flow_trace_root_count(trace, root_keys) >= 2
        || context_flow_trace_query_coverage(trace, query_terms) >= 2
}

fn context_flow_target_reference_edges(
    index: &Codebase,
    target: &SymbolTarget,
    query_terms: &[String],
    limit: usize,
) -> Result<Vec<ContextFlowSymbolEdge>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(file) = index.file(&target.path) else {
        return Ok(Vec::new());
    };
    let Some(symbol) = symbol_for_target(file, target) else {
        return Ok(Vec::new());
    };
    if !is_context_handoff_source_symbol(symbol) {
        return Ok(Vec::new());
    }
    let symbol_end = symbol.line_end.max(symbol.line_start);
    let span = symbol_end.saturating_sub(symbol.line_start) + 1;
    if span > CONTEXT_FLOW_BODY_SCAN_MAX_LINES {
        return Ok(Vec::new());
    }
    let content = index.file_content(file)?;
    let body = source_line_slice(&content, symbol.line_start, symbol_end);
    let leads = symbol_body_leads_with_terms(index, file, symbol, &body, limit, query_terms);
    Ok(leads
        .into_iter()
        .map(|lead| ContextFlowSymbolEdge {
            source: target.clone(),
            target: lead.target,
            score: lead.score as f32,
            reason: if lead.query_matches.is_empty() {
                "body reference".to_string()
            } else {
                format!(
                    "body reference matches {}",
                    lead.query_matches
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("/")
                )
            },
        })
        .collect())
}

fn context_flow_data_type_leads(
    index: &Codebase,
    symbols: &[ContextFlowSymbol],
) -> Result<Vec<ContextFlowDataTypeLead>> {
    let mut leads = Vec::<ContextFlowDataTypeLead>::new();
    let mut seen = BTreeSet::<(String, usize, String, String, usize, String)>::new();
    for source in symbols.iter().take(CONTEXT_FLOW_DATA_TYPE_SOURCE_SYMBOLS) {
        let Some(file) = index.file(&source.target.path) else {
            continue;
        };
        let Some(symbol) = symbol_for_target(file, &source.target) else {
            continue;
        };
        if !is_context_handoff_source_symbol(symbol) {
            continue;
        }
        let symbol_end = symbol.line_end.max(symbol.line_start);
        let span = symbol_end.saturating_sub(symbol.line_start) + 1;
        if span > CONTEXT_FLOW_BODY_SCAN_MAX_LINES {
            continue;
        }
        let content = index.file_content(file)?;
        let body = source_line_slice(&content, symbol.line_start, symbol_end);
        for lead in symbol_body_data_type_leads(
            index,
            file,
            symbol,
            &body,
            CONTEXT_FLOW_DATA_TYPE_PER_SYMBOL,
        ) {
            let key = (
                source.target.path.clone(),
                source.target.line_start,
                source.target.name.clone(),
                lead.target.path.clone(),
                lead.target.line_start,
                lead.target.name.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            leads.push(ContextFlowDataTypeLead {
                source: source.target.clone(),
                target: lead.target,
                order: lead.order,
            });
            if leads.len() >= CONTEXT_FLOW_DATA_TYPE_TOTAL_LIMIT {
                break;
            }
        }
        if leads.len() >= CONTEXT_FLOW_DATA_TYPE_TOTAL_LIMIT {
            break;
        }
    }
    Ok(leads)
}

fn append_context_flow_data_type_leads(leads: &[ContextFlowDataTypeLead], out: &mut String) {
    if leads.is_empty() {
        return;
    }
    out.push_str("data/type links:\n");
    out.push_str("from|to|why\n");
    for lead in leads {
        out.push_str(&format!(
            "{}|{}|body identifier definition order={}\n",
            context_flow_target_cell(&lead.source),
            context_flow_target_cell(&lead.target),
            lead.order
        ));
    }
}

fn context_flow_file_edges(
    index: &Codebase,
    candidates: &[ContextCandidate],
    query: &str,
) -> Vec<ContextFlowFileEdge> {
    let mut edges = Vec::<ContextFlowFileEdge>::new();
    let mut seen = BTreeSet::<(String, String, &'static str)>::new();
    for candidate in candidates.iter().take(CONTEXT_FLOW_CANDIDATE_LIMIT) {
        for neighbor in ranked_direct_graph_neighbors(index, &candidate.path, query, 4) {
            if !seen.insert((
                candidate.path.clone(),
                neighbor.path.clone(),
                neighbor.direction,
            )) {
                continue;
            }
            let reason = candidate
                .reasons
                .iter()
                .next()
                .cloned()
                .unwrap_or_else(|| "ranked candidate".to_string());
            edges.push(ContextFlowFileEdge {
                source_path: candidate.path.clone(),
                target_path: neighbor.path,
                direction: neighbor.direction,
                score: candidate.score * 0.15 + neighbor.score,
                reason,
            });
        }
    }
    if edges.is_empty() {
        return edges;
    }
    edges.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.source_path.cmp(&right.source_path))
            .then_with(|| left.target_path.cmp(&right.target_path))
    });
    edges
}

fn append_context_flow_file_edges(edges: &[ContextFlowFileEdge], out: &mut String) {
    if edges.is_empty() {
        return;
    }
    out.push_str("file edges:\n");
    out.push_str("from|rel|to|why\n");
    for edge in edges.iter().take(CONTEXT_FLOW_FILE_EDGE_LIMIT) {
        out.push_str(&format!(
            "{}|{}|{}|{}\n",
            context_flow_cell(&edge.source_path, 96),
            edge.direction,
            context_flow_cell(&edge.target_path, 96),
            context_flow_cell(&edge.reason, 72)
        ));
    }
}

fn append_context_flow_followups(
    index: &Codebase,
    candidates: &[ContextCandidate],
    symbols: &[ContextFlowSymbol],
    symbol_edges: &[ContextFlowSymbolEdge],
    out: &mut String,
) {
    let mut graph_section = String::new();
    let mut seen_paths = BTreeSet::<String>::new();
    for candidate in candidates.iter().take(CONTEXT_FLOW_FOLLOWUP_LIMIT) {
        if graph_section.is_empty() {
            graph_section.push_str("followup deps:\n");
        }
        if seen_paths.insert(candidate.path.clone()) {
            graph_section.push_str(&format!("  codedb_deps path={}\n", candidate.path));
        }
    }
    out.push_str(&graph_section);

    let mut callpath_section = String::new();
    let mut seen_edges = BTreeSet::<(String, usize, String, String, usize, String)>::new();
    for edge in symbol_edges.iter().take(CONTEXT_FLOW_FOLLOWUP_LIMIT) {
        let key = (
            edge.source.path.clone(),
            edge.source.line_start,
            edge.source.name.clone(),
            edge.target.path.clone(),
            edge.target.line_start,
            edge.target.name.clone(),
        );
        if !seen_edges.insert(key) {
            continue;
        }
        if callpath_section.is_empty() {
            callpath_section.push_str("followup callpath:\n");
        }
        let args = json!({
            "from": edge.source.name,
            "from_path": edge.source.path,
            "from_line": edge.source.line_start,
            "to": edge.target.name,
            "to_path": edge.target.path,
            "to_line": edge.target.line_start,
            "max_hops": 6
        });
        callpath_section.push_str(&format!("  codedb_callpath {args}\n"));
    }
    out.push_str(&callpath_section);

    if symbols.is_empty() {
        let mut fallback_section = String::new();
        let mut seen_family = BTreeSet::<String>::new();
        for candidate in candidates.iter().take(4) {
            if seen_family.len() >= 2 {
                break;
            }
            let Some(pattern) = context_flow_family_glob(index, &candidate.path) else {
                continue;
            };
            if !seen_family.insert(pattern.clone()) {
                continue;
            }
            if fallback_section.is_empty() {
                fallback_section.push_str("followup glob fallback:\n");
            }
            fallback_section.push_str(&format!(
                "  codedb_glob pattern={} summary_limit=6\n",
                pattern
            ));
        }
        out.push_str(&fallback_section);
        return;
    }

    let mut emitted = 0usize;
    let mut seen = BTreeSet::<(String, usize, String)>::new();
    let mut section = String::new();
    for symbol in symbols {
        if emitted >= CONTEXT_FLOW_FOLLOWUP_LIMIT {
            break;
        }
        let key = (
            symbol.target.path.clone(),
            symbol.target.line_start,
            symbol.target.name.clone(),
        );
        if !seen.insert(key) {
            continue;
        }
        if section.is_empty() {
            section.push_str("followup symbol:\n");
        }
        section.push_str(&format!(
            "  codedb_symbol name={} path={} body=true max_results=1\n",
            symbol.target.name, symbol.target.path
        ));
        emitted += 1;
    }
    out.push_str(&section);
}

fn context_flow_family_glob(index: &Codebase, path: &str) -> Option<String> {
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() <= 1 {
        return None;
    }
    for depth in (1..parts.len()).rev() {
        let prefix = parts[..depth].join("/");
        let prefix_slash = format!("{prefix}/");
        let file_count = index
            .files
            .keys()
            .filter(|candidate| candidate.starts_with(&prefix_slash))
            .take(CONTEXT_MODULE_INVENTORY_MAX_FILES + 1)
            .count();
        if (3..=CONTEXT_MODULE_INVENTORY_MAX_FILES).contains(&file_count) {
            return Some(format!("{prefix}/**"));
        }
    }
    Some(format!("{}/**", parts[..parts.len() - 1].join("/")))
}

fn context_flow_target_cell(target: &SymbolTarget) -> String {
    context_flow_cell(
        &format!(
            "{}:L{} {} {}",
            target.path, target.line_start, target.kind, target.name
        ),
        120,
    )
}

fn context_flow_cell(value: &str, max_chars: usize) -> String {
    truncate_inline(
        &value
            .replace('|', "/")
            .replace('\r', " ")
            .replace('\n', " ")
            .trim()
            .to_string(),
        max_chars,
    )
}

fn finalize_context_output(out: &mut String, max_chars: usize) {
    if out.len() <= max_chars {
        return;
    }
    let marker = format!(
        "\n[truncated at {max_chars} chars; continue from exact returned paths/symbols/callers/deps/outlines]\n"
    );
    let budget = max_chars.saturating_sub(marker.len());
    truncate_string(out, budget);
    out.push_str(&marker);
}

fn append_context_candidate_overview(
    index: &Codebase,
    candidates: &[ContextCandidate],
    query: &str,
    out: &mut String,
) {
    if candidates.is_empty() {
        return;
    }
    out.push_str("ranked files:\n");
    for (idx, candidate) in candidates.iter().enumerate() {
        let Some(file) = index.file(&candidate.path) else {
            continue;
        };
        let reasons = candidate
            .reasons
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "  {}. {} score={:.2} why={}",
            idx + 1,
            candidate.path,
            candidate.score,
            reasons
        ));
        if !candidate.hit_lines.is_empty() {
            let hit_lines = candidate
                .hit_lines
                .iter()
                .take(4)
                .map(|line| line.to_string())
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(&format!(" hit_lines={hit_lines}"));
        }
        let symbols = selected_candidate_symbols(file, candidate, query, 3);
        if !symbols.is_empty() {
            let summary = symbols
                .into_iter()
                .map(|symbol| format!("L{} {} {}", symbol.line_start, symbol.kind, symbol.name))
                .collect::<Vec<_>>()
                .join("; ");
            out.push_str(&format!(" symbols: {summary}"));
        }
        out.push('\n');
    }
}

#[derive(Clone)]
struct ContextModuleInventoryRow {
    prefix: String,
    file_count: usize,
    degree: usize,
    outgoing: usize,
    incoming: usize,
    representatives: Vec<String>,
    depth: usize,
    score: f32,
}

#[derive(Debug, Clone)]
struct ContextModuleInventoryLeafGroup {
    parent: String,
    children: Vec<ContextModuleInventoryLeafChild>,
    score: f32,
}

#[derive(Debug, Clone)]
struct ContextModuleInventoryLeafChild {
    name: String,
    file_count: usize,
    outgoing: usize,
    incoming: usize,
    representatives: Vec<String>,
}

#[derive(Clone)]
struct ContextModuleInventoryLink {
    source: String,
    target: String,
    direction: &'static str,
    count: usize,
}

fn append_context_module_inventory(index: &Codebase, out: &mut String) {
    let rows = context_module_inventory_rows(index);
    if rows.is_empty() {
        return;
    }
    let broad_rows = select_context_module_inventory_broad_rows(&rows);
    let focused_rows = select_context_module_inventory_focused_rows(&rows, &broad_rows);
    let leaf_groups = select_context_module_inventory_leaf_groups(&rows, &broad_rows);
    out.push_str("module inventory (selection only; follow exact prefixes/paths):\n");
    if !leaf_groups.is_empty() {
        out.push_str("  compact source prefix index (scope to parent/child/**, not the broad parent/**; use separate leaves for separate phases):\n");
    }
    for group in leaf_groups {
        let show_representatives = broad_rows.iter().any(|broad| {
            group.parent == broad.prefix || group.parent.starts_with(&(broad.prefix.clone() + "/"))
        });
        let children = group
            .children
            .iter()
            .map(|child| {
                let mut summary = format!(
                    "{}({},o={},i={})",
                    child.name, child.file_count, child.outgoing, child.incoming
                );
                if show_representatives && !child.representatives.is_empty() {
                    summary.push_str(&format!(",r={}", child.representatives.join("|")));
                }
                summary
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("    - {}: {}\n", group.parent, children));
    }
    append_context_module_inventory_links(index, &focused_rows, out);
    if !focused_rows.is_empty() {
        out.push_str("  focused path groups:\n");
    }
    for row in focused_rows {
        out.push_str(&format!(
            "    - {} ({} files, graph_degree={})\n",
            row.prefix, row.file_count, row.degree
        ));
    }
    if !broad_rows.is_empty() {
        out.push_str("  broad path groups:\n");
    }
    for row in broad_rows {
        out.push_str(&format!(
            "    - {} ({} files, graph_degree={})\n",
            row.prefix, row.file_count, row.degree
        ));
    }
}

fn context_module_inventory_rows(index: &Codebase) -> Vec<ContextModuleInventoryRow> {
    let mut grouped_paths = BTreeMap::<String, BTreeSet<String>>::new();
    for file in index.files.values() {
        for prefix in context_module_prefixes(&file.path) {
            grouped_paths
                .entry(prefix)
                .or_default()
                .insert(file.path.clone());
        }
    }

    let mut rows = Vec::new();
    for (prefix, paths) in grouped_paths {
        let file_count = paths.len();
        if !(CONTEXT_MODULE_INVENTORY_MIN_FILES..=CONTEXT_MODULE_INVENTORY_MAX_FILES)
            .contains(&file_count)
        {
            continue;
        }
        let depth = path_component_count(&prefix);
        let outgoing = paths
            .iter()
            .map(|path| index.deps_for(path).len())
            .sum::<usize>();
        let incoming = paths
            .iter()
            .map(|path| index.reverse_deps_for(path).len())
            .sum::<usize>();
        let degree = outgoing + incoming;
        let representatives = context_module_inventory_representative_paths(index, &paths);
        let score = ((file_count + 1) as f32).ln() * 8.0
            + ((degree + 1) as f32).ln() * 5.0
            + depth as f32 * 1.25
            + configured_scan_root_order_score(index, &prefix)
            + generic_source_path_score(&prefix) * 2.0;
        rows.push(ContextModuleInventoryRow {
            prefix,
            file_count,
            degree,
            outgoing,
            incoming,
            representatives,
            depth,
            score,
        });
    }

    rows.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.file_count.cmp(&left.file_count))
            .then_with(|| left.prefix.cmp(&right.prefix))
    });
    rows
}

fn append_context_module_inventory_links(
    index: &Codebase,
    focused_rows: &[ContextModuleInventoryRow],
    out: &mut String,
) {
    let links = context_module_inventory_links(index, focused_rows);
    if links.is_empty() {
        return;
    }
    out.push_str("  focused path graph links:\n");
    for link in links {
        out.push_str(&format!(
            "    - {} --{}:{}--> {}\n",
            link.source, link.direction, link.count, link.target
        ));
    }
}

fn context_module_inventory_links(
    index: &Codebase,
    focused_rows: &[ContextModuleInventoryRow],
) -> Vec<ContextModuleInventoryLink> {
    if focused_rows.is_empty() {
        return Vec::new();
    }
    let prefixes = focused_rows
        .iter()
        .map(|row| row.prefix.clone())
        .collect::<Vec<_>>();
    let mut prefixes_by_depth = prefixes.clone();
    prefixes_by_depth.sort_by(|left, right| {
        path_component_count(right)
            .cmp(&path_component_count(left))
            .then_with(|| left.cmp(right))
    });
    let mut file_prefix = BTreeMap::<String, String>::new();
    for file in index.files.values() {
        if let Some(prefix) =
            longest_context_module_inventory_prefix(&file.path, &prefixes_by_depth)
        {
            file_prefix.insert(file.path.clone(), prefix);
        }
    }
    let mut counts = BTreeMap::<(String, String, &'static str), usize>::new();
    for (path, source_prefix) in &file_prefix {
        for dep in index.deps_for(path) {
            let Some(target_prefix) = file_prefix.get(&dep) else {
                continue;
            };
            if target_prefix == source_prefix {
                continue;
            }
            *counts
                .entry((source_prefix.clone(), target_prefix.clone(), "depends_on"))
                .or_default() += 1;
            *counts
                .entry((target_prefix.clone(), source_prefix.clone(), "imported_by"))
                .or_default() += 1;
        }
    }
    let row_by_prefix = focused_rows
        .iter()
        .map(|row| (row.prefix.clone(), row.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut by_source = BTreeMap::<String, Vec<ContextModuleInventoryLink>>::new();
    for ((source, target, direction), count) in counts {
        by_source
            .entry(source.clone())
            .or_default()
            .push(ContextModuleInventoryLink {
                source,
                target,
                direction,
                count,
            });
    }
    let mut selected = Vec::new();
    for row in focused_rows {
        let Some(links) = by_source.remove(&row.prefix) else {
            continue;
        };
        for link in select_context_module_inventory_source_links(links, &row_by_prefix) {
            selected.push(link);
        }
    }
    selected.sort_by(|left, right| {
        context_module_inventory_link_output_score(right, &row_by_prefix)
            .total_cmp(&context_module_inventory_link_output_score(
                left,
                &row_by_prefix,
            ))
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.target.cmp(&right.target))
    });
    selected.truncate(CONTEXT_MODULE_INVENTORY_LINK_LIMIT);
    selected
}

fn select_context_module_inventory_source_links(
    links: Vec<ContextModuleInventoryLink>,
    row_by_prefix: &BTreeMap<String, ContextModuleInventoryRow>,
) -> Vec<ContextModuleInventoryLink> {
    if links.len() <= CONTEXT_MODULE_INVENTORY_LINK_PER_PREFIX {
        return links;
    }
    let mut selected = Vec::new();
    let mut seen = BTreeSet::<(String, &'static str)>::new();
    let mut by_count = links.clone();
    by_count.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.direction.cmp(right.direction))
            .then_with(|| left.target.cmp(&right.target))
    });
    for link in by_count
        .into_iter()
        .take(CONTEXT_MODULE_INVENTORY_LINK_COUNT_PER_PREFIX)
    {
        seen.insert((link.target.clone(), link.direction));
        selected.push(link);
    }

    let mut by_specificity = links;
    by_specificity.sort_by(|left, right| {
        context_module_inventory_link_specificity_score(right, row_by_prefix)
            .total_cmp(&context_module_inventory_link_specificity_score(
                left,
                row_by_prefix,
            ))
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.direction.cmp(right.direction))
            .then_with(|| left.target.cmp(&right.target))
    });
    for link in by_specificity {
        if selected.len() >= CONTEXT_MODULE_INVENTORY_LINK_PER_PREFIX {
            break;
        }
        if seen.insert((link.target.clone(), link.direction)) {
            selected.push(link);
        }
    }
    selected
}

fn context_module_inventory_link_specificity_score(
    link: &ContextModuleInventoryLink,
    row_by_prefix: &BTreeMap<String, ContextModuleInventoryRow>,
) -> f32 {
    let source_components = path_components(&link.source);
    let target_components = path_components(&link.target);
    let locality = common_prefix_len(&source_components, &target_components) as f32;
    let count_score = ((link.count + 1) as f32).ln() * 2.0;
    let Some(target) = row_by_prefix.get(&link.target) else {
        return locality + count_score;
    };
    let depth_score = target.depth as f32 * 0.4;
    let size_score = context_module_inventory_size_score(target.file_count) * 0.2;
    let degree_penalty = ((target.degree + 1) as f32).ln() * 0.7;
    let file_penalty = ((target.file_count + 1) as f32).ln() * 0.8;
    locality * 2.0 + count_score + depth_score + size_score - degree_penalty - file_penalty
}

fn context_module_inventory_link_output_score(
    link: &ContextModuleInventoryLink,
    row_by_prefix: &BTreeMap<String, ContextModuleInventoryRow>,
) -> f32 {
    let source_score = row_by_prefix
        .get(&link.source)
        .map(context_module_inventory_focus_score)
        .unwrap_or_default();
    let target_score = row_by_prefix
        .get(&link.target)
        .map(context_module_inventory_focus_score)
        .unwrap_or_default();
    let source_quality = generic_source_path_score(&link.source);
    let target_quality = generic_source_path_score(&link.target);
    context_module_inventory_link_specificity_score(link, row_by_prefix)
        + source_score.min(80.0) * 0.08
        + target_score.min(80.0) * 0.08
        + source_quality * 6.0
        + target_quality * 6.0
}

fn longest_context_module_inventory_prefix(
    path: &str,
    prefixes_by_depth: &[String],
) -> Option<String> {
    prefixes_by_depth
        .iter()
        .find(|prefix| path_matches_prefix(path, prefix))
        .cloned()
}

fn context_module_prefixes(path: &str) -> Vec<String> {
    let mut parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.last().is_some_and(|part| part.contains('.')) {
        parts.pop();
    }
    let max_depth = parts.len().min(CONTEXT_MODULE_INVENTORY_MAX_DEPTH);
    let mut prefixes = Vec::new();
    for depth in 2..=max_depth {
        prefixes.push(parts[..depth].join("/"));
    }
    prefixes
}

fn select_context_module_inventory_broad_rows(
    rows: &[ContextModuleInventoryRow],
) -> Vec<ContextModuleInventoryRow> {
    let mut selected = Vec::new();
    push_context_module_inventory_broad_rows(rows, &mut selected, true);
    push_context_module_inventory_broad_rows(rows, &mut selected, false);
    selected
}

fn push_context_module_inventory_broad_rows(
    rows: &[ContextModuleInventoryRow],
    selected: &mut Vec<ContextModuleInventoryRow>,
    prefer_source_paths: bool,
) {
    for row in rows
        .iter()
        .filter(|row| row.depth <= CONTEXT_MODULE_INVENTORY_BROAD_MAX_DEPTH)
    {
        if selected.len() >= CONTEXT_MODULE_INVENTORY_BROAD_LIMIT {
            break;
        }
        if prefer_source_paths && generic_source_path_score(&row.prefix) < 0.0 {
            continue;
        }
        if selected
            .iter()
            .any(|prior: &ContextModuleInventoryRow| prior.prefix == row.prefix)
        {
            continue;
        }
        if selected.iter().any(|prior: &ContextModuleInventoryRow| {
            row.prefix.starts_with(&(prior.prefix.clone() + "/"))
                || prior.prefix.starts_with(&(row.prefix.clone() + "/"))
        }) {
            continue;
        }
        selected.push(row.clone());
    }
}

fn select_context_module_inventory_focused_rows(
    rows: &[ContextModuleInventoryRow],
    broad_rows: &[ContextModuleInventoryRow],
) -> Vec<ContextModuleInventoryRow> {
    let broad_prefixes = broad_rows
        .iter()
        .map(|row| row.prefix.clone())
        .collect::<BTreeSet<_>>();
    let mut candidates = rows
        .iter()
        .filter(|row| row.depth >= CONTEXT_MODULE_INVENTORY_FOCUSED_MIN_DEPTH)
        .filter(|row| !broad_prefixes.contains(&row.prefix))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        context_module_inventory_focus_score(right)
            .total_cmp(&context_module_inventory_focus_score(left))
            .then_with(|| right.file_count.cmp(&left.file_count))
            .then_with(|| left.prefix.cmp(&right.prefix))
    });

    let mut selected = Vec::new();
    let mut parent_counts = BTreeMap::<String, usize>::new();
    let source_candidates = candidates
        .iter()
        .filter(|row| generic_source_path_score(&row.prefix) >= 0.0)
        .cloned()
        .collect::<Vec<_>>();
    push_context_module_inventory_focused_rows(
        &source_candidates,
        &mut selected,
        &mut parent_counts,
        true,
    );
    push_context_module_inventory_focused_rows(
        &candidates,
        &mut selected,
        &mut parent_counts,
        true,
    );
    push_context_module_inventory_focused_rows(
        &source_candidates,
        &mut selected,
        &mut parent_counts,
        false,
    );
    push_context_module_inventory_focused_rows(
        &candidates,
        &mut selected,
        &mut parent_counts,
        false,
    );
    selected.sort_by(|left, right| {
        context_module_inventory_focus_score(right)
            .total_cmp(&context_module_inventory_focus_score(left))
            .then_with(|| right.file_count.cmp(&left.file_count))
            .then_with(|| left.prefix.cmp(&right.prefix))
    });
    selected
}

fn push_context_module_inventory_focused_rows(
    rows: &[ContextModuleInventoryRow],
    selected: &mut Vec<ContextModuleInventoryRow>,
    parent_counts: &mut BTreeMap<String, usize>,
    enforce_parent_limit: bool,
) {
    for row in rows {
        if selected.len() >= CONTEXT_MODULE_INVENTORY_FOCUSED_LIMIT {
            break;
        }
        if selected
            .iter()
            .any(|prior: &ContextModuleInventoryRow| prior.prefix == row.prefix)
        {
            continue;
        }
        if selected.iter().any(|prior: &ContextModuleInventoryRow| {
            row.prefix.starts_with(&(prior.prefix.clone() + "/"))
                || prior.prefix.starts_with(&(row.prefix.clone() + "/"))
        }) {
            continue;
        }
        let parent = context_module_inventory_parent_bucket(&row.prefix);
        let count = parent_counts.get(&parent).copied().unwrap_or(0);
        if enforce_parent_limit && count >= CONTEXT_MODULE_INVENTORY_FOCUSED_PER_PARENT {
            continue;
        }
        parent_counts.insert(parent, count + 1);
        selected.push(row.clone());
    }
}

fn select_context_module_inventory_leaf_groups(
    rows: &[ContextModuleInventoryRow],
    broad_rows: &[ContextModuleInventoryRow],
) -> Vec<ContextModuleInventoryLeafGroup> {
    let mut by_parent = BTreeMap::<String, Vec<ContextModuleInventoryRow>>::new();
    for row in rows
        .iter()
        .filter(|row| row.depth >= CONTEXT_MODULE_INVENTORY_FOCUSED_MIN_DEPTH)
        .filter(|row| generic_source_path_score(&row.prefix) >= 0.0)
    {
        let Some((parent, child)) = split_context_module_inventory_leaf(&row.prefix) else {
            continue;
        };
        if child.is_empty() {
            continue;
        }
        by_parent.entry(parent).or_default().push(row.clone());
    }

    let mut groups = Vec::new();
    for (parent, mut children) in by_parent {
        dedupe_context_module_inventory_children(&mut children);
        if children.len() < 2
            && !is_context_module_inventory_shallow_single_child(&parent, &children)
        {
            continue;
        }
        let score = context_module_inventory_leaf_group_score(&parent, &children);
        groups.push(ContextModuleInventoryLeafGroup {
            parent,
            children: select_context_module_inventory_leaf_children(
                &children,
                CONTEXT_MODULE_INVENTORY_LEAF_GROUP_CHILD_LIMIT,
            ),
            score,
        });
    }
    groups.retain(|group| !group.children.is_empty());
    groups.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.parent.cmp(&right.parent))
    });

    let mut selected = Vec::new();
    let mut selected_parents = BTreeSet::new();
    let mut total_children = 0usize;
    for broad in broad_rows {
        let best_group = groups
            .iter()
            .filter(|group| {
                group.parent == broad.prefix
                    || group.parent.starts_with(&(broad.prefix.clone() + "/"))
            })
            .min_by(|left, right| {
                let left_exact = left.parent == broad.prefix;
                let right_exact = right.parent == broad.prefix;
                right_exact
                    .cmp(&left_exact)
                    .then_with(|| {
                        path_component_count(&left.parent).cmp(&path_component_count(&right.parent))
                    })
                    .then_with(|| right.score.total_cmp(&left.score))
                    .then_with(|| left.parent.cmp(&right.parent))
            })
            .cloned();
        if let Some(group) = best_group {
            push_context_module_inventory_leaf_group(
                group,
                &mut selected,
                &mut selected_parents,
                &mut total_children,
            );
        }
    }
    let mut rootish_groups = groups
        .iter()
        .filter(|group| path_component_count(&group.parent) <= 3)
        .cloned()
        .collect::<Vec<_>>();
    rootish_groups.sort_by(|left, right| {
        path_component_count(&left.parent)
            .cmp(&path_component_count(&right.parent))
            .then_with(|| {
                context_module_inventory_leaf_group_file_count(right)
                    .cmp(&context_module_inventory_leaf_group_file_count(left))
            })
            .then_with(|| left.parent.cmp(&right.parent))
    });
    for group in rootish_groups.into_iter().take(4) {
        push_context_module_inventory_leaf_group(
            group,
            &mut selected,
            &mut selected_parents,
            &mut total_children,
        );
    }
    let ranked_limit = CONTEXT_MODULE_INVENTORY_LEAF_GROUP_LIMIT
        .saturating_sub(4)
        .max(CONTEXT_MODULE_INVENTORY_LEAF_GROUP_LIMIT / 2);
    for group in groups.iter().take(ranked_limit).cloned() {
        push_context_module_inventory_leaf_group(
            group,
            &mut selected,
            &mut selected_parents,
            &mut total_children,
        );
    }

    let mut alpha_groups = groups.clone();
    alpha_groups.sort_by(|left, right| left.parent.cmp(&right.parent));
    let diversity_slots = CONTEXT_MODULE_INVENTORY_LEAF_GROUP_LIMIT.saturating_sub(selected.len());
    for index in evenly_spaced_indices(alpha_groups.len(), diversity_slots) {
        if let Some(group) = alpha_groups.get(index).cloned() {
            push_context_module_inventory_leaf_group(
                group,
                &mut selected,
                &mut selected_parents,
                &mut total_children,
            );
        }
    }

    for group in groups {
        if selected.len() >= CONTEXT_MODULE_INVENTORY_LEAF_GROUP_LIMIT
            || total_children >= CONTEXT_MODULE_INVENTORY_LEAF_GROUP_TOTAL_CHILD_LIMIT
        {
            break;
        }
        push_context_module_inventory_leaf_group(
            group,
            &mut selected,
            &mut selected_parents,
            &mut total_children,
        );
    }
    selected
}

fn is_context_module_inventory_shallow_single_child(
    parent: &str,
    children: &[ContextModuleInventoryRow],
) -> bool {
    children.len() == 1
        && path_component_count(parent) <= 3
        && children
            .first()
            .is_some_and(|child| child.file_count >= CONTEXT_MODULE_INVENTORY_MIN_FILES)
}

fn context_module_inventory_leaf_group_file_count(
    group: &ContextModuleInventoryLeafGroup,
) -> usize {
    group.children.iter().map(|child| child.file_count).sum()
}

fn push_context_module_inventory_leaf_group(
    mut group: ContextModuleInventoryLeafGroup,
    selected: &mut Vec<ContextModuleInventoryLeafGroup>,
    selected_parents: &mut BTreeSet<String>,
    total_children: &mut usize,
) {
    if selected.len() >= CONTEXT_MODULE_INVENTORY_LEAF_GROUP_LIMIT
        || *total_children >= CONTEXT_MODULE_INVENTORY_LEAF_GROUP_TOTAL_CHILD_LIMIT
        || !selected_parents.insert(group.parent.clone())
    {
        return;
    }
    let remaining = CONTEXT_MODULE_INVENTORY_LEAF_GROUP_TOTAL_CHILD_LIMIT - *total_children;
    if group.children.len() > remaining {
        group.children.truncate(remaining);
    }
    if group.children.is_empty() {
        return;
    }
    *total_children += group.children.len();
    selected.push(group);
}

fn split_context_module_inventory_leaf(prefix: &str) -> Option<(String, String)> {
    let mut parts = prefix
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let child = parts.pop()?.to_string();
    if parts.is_empty() {
        return None;
    }
    Some((parts.join("/"), child))
}

fn dedupe_context_module_inventory_children(children: &mut Vec<ContextModuleInventoryRow>) {
    children.sort_by(|left, right| {
        split_context_module_inventory_leaf(&left.prefix)
            .map(|(_, child)| child)
            .cmp(&split_context_module_inventory_leaf(&right.prefix).map(|(_, child)| child))
            .then_with(|| {
                context_module_inventory_focus_score(right)
                    .total_cmp(&context_module_inventory_focus_score(left))
            })
            .then_with(|| right.file_count.cmp(&left.file_count))
    });
    children.dedup_by(|left, right| {
        split_context_module_inventory_leaf(&left.prefix).map(|(_, child)| child)
            == split_context_module_inventory_leaf(&right.prefix).map(|(_, child)| child)
    });
}

fn context_module_inventory_leaf_group_score(
    parent: &str,
    children: &[ContextModuleInventoryRow],
) -> f32 {
    let child_count = children.len();
    let file_count = children.iter().map(|row| row.file_count).sum::<usize>();
    let degree = children.iter().map(|row| row.degree).sum::<usize>();
    let best_child_score = children
        .iter()
        .map(context_module_inventory_focus_score)
        .fold(0.0f32, f32::max);
    best_child_score
        + ((child_count + 1) as f32).ln() * 10.0
        + ((file_count + 1) as f32).ln() * 4.0
        + ((degree + 1) as f32).ln() * 2.0
        + path_component_count(parent) as f32
        + generic_source_path_score(parent) * 2.0
}

fn select_context_module_inventory_leaf_children(
    children: &[ContextModuleInventoryRow],
    limit: usize,
) -> Vec<ContextModuleInventoryLeafChild> {
    if children.len() <= limit {
        return context_module_inventory_leaf_children_from_rows(children);
    }

    let mut selected = BTreeMap::<String, ContextModuleInventoryRow>::new();
    let ranked_limit = (limit / 2).max(1);
    let mut ranked = children.to_vec();
    ranked.sort_by(|left, right| {
        context_module_inventory_focus_score(right)
            .total_cmp(&context_module_inventory_focus_score(left))
            .then_with(|| right.file_count.cmp(&left.file_count))
            .then_with(|| left.prefix.cmp(&right.prefix))
    });
    for row in ranked.into_iter().take(ranked_limit) {
        if let Some((_, child)) = split_context_module_inventory_leaf(&row.prefix) {
            selected.entry(child).or_insert(row);
        }
    }

    let mut alpha = children.to_vec();
    alpha.sort_by(|left, right| left.prefix.cmp(&right.prefix));
    let remaining = limit.saturating_sub(selected.len());
    for index in evenly_spaced_indices(alpha.len(), remaining) {
        if let Some(row) = alpha.get(index).cloned()
            && let Some((_, child)) = split_context_module_inventory_leaf(&row.prefix)
        {
            selected.entry(child).or_insert(row);
        }
    }

    let mut rows = selected.into_values().collect::<Vec<_>>();
    rows.sort_by(|left, right| left.prefix.cmp(&right.prefix));
    context_module_inventory_leaf_children_from_rows(&rows)
}

fn context_module_inventory_leaf_children_from_rows(
    rows: &[ContextModuleInventoryRow],
) -> Vec<ContextModuleInventoryLeafChild> {
    let mut rows = rows.to_vec();
    rows.sort_by(|left, right| {
        context_module_inventory_entry_score(right)
            .total_cmp(&context_module_inventory_entry_score(left))
            .then_with(|| {
                context_module_inventory_focus_score(right)
                    .total_cmp(&context_module_inventory_focus_score(left))
            })
            .then_with(|| left.prefix.cmp(&right.prefix))
    });
    rows.iter()
        .filter_map(|row| {
            split_context_module_inventory_leaf(&row.prefix).map(|(_, name)| {
                ContextModuleInventoryLeafChild {
                    name,
                    file_count: row.file_count,
                    outgoing: row.outgoing,
                    incoming: row.incoming,
                    representatives: row.representatives.clone(),
                }
            })
        })
        .collect()
}

fn context_module_inventory_entry_score(row: &ContextModuleInventoryRow) -> f32 {
    let outgoing = (row.outgoing + 1) as f32;
    let incoming = (row.incoming + 1) as f32;
    outgoing.ln() * 7.0 - incoming.ln() * 4.0 + (outgoing / incoming).ln() * 3.0
}

fn context_module_inventory_representative_paths(
    index: &Codebase,
    paths: &BTreeSet<String>,
) -> Vec<String> {
    let mut ranked = paths
        .iter()
        .map(|path| {
            let outgoing = index.deps_for(path).len();
            let incoming = index.reverse_deps_for(path).len();
            let entry_score = ((outgoing + 1) as f32).ln() * 7.0
                - ((incoming + 1) as f32).ln() * 4.0
                + (((outgoing + 1) as f32) / ((incoming + 1) as f32)).ln() * 3.0;
            let central_score = ((outgoing + incoming + 1) as f32).ln() * 6.0;
            (path, entry_score, central_score, outgoing)
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| right.2.total_cmp(&left.2))
            .then_with(|| left.0.cmp(right.0))
    });
    let mut selected = Vec::<String>::new();
    if let Some((path, _, _, _)) = ranked.first() {
        selected.push(context_module_inventory_representative_name(path));
    }
    ranked.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| right.1.total_cmp(&left.1))
            .then_with(|| left.0.cmp(right.0))
    });
    for (path, _, _, _) in &ranked {
        let name = context_module_inventory_representative_name(path);
        if selected.contains(&name) {
            continue;
        }
        selected.push(name);
        break;
    }
    ranked.sort_by(|left, right| {
        right
            .3
            .cmp(&left.3)
            .then_with(|| right.2.total_cmp(&left.2))
            .then_with(|| left.0.cmp(right.0))
    });
    for (path, _, _, _) in ranked {
        let name = context_module_inventory_representative_name(path);
        if selected.contains(&name) {
            continue;
        }
        selected.push(name);
        break;
    }
    selected
}

fn context_module_inventory_representative_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

fn evenly_spaced_indices(len: usize, limit: usize) -> Vec<usize> {
    if len == 0 || limit == 0 {
        return Vec::new();
    }
    if limit >= len {
        return (0..len).collect();
    }
    let mut indices = BTreeSet::new();
    for slot in 0..limit {
        let index = if limit == 1 {
            len / 2
        } else {
            slot * (len - 1) / (limit - 1)
        };
        indices.insert(index);
    }
    indices.into_iter().collect()
}

fn context_module_inventory_focus_score(row: &ContextModuleInventoryRow) -> f32 {
    row.score + row.depth as f32 * 4.0 + context_module_inventory_size_score(row.file_count)
}

fn context_module_inventory_size_score(file_count: usize) -> f32 {
    match file_count {
        0..=2 => -20.0,
        3..=8 => 6.0,
        9..=40 => 14.0,
        41..=100 => 12.0,
        101..=160 => 7.0,
        _ => 2.0,
    }
}

fn context_module_inventory_parent_bucket(prefix: &str) -> String {
    let parts = prefix
        .split('/')
        .filter(|part| !part.is_empty())
        .take(
            prefix
                .split('/')
                .filter(|part| !part.is_empty())
                .count()
                .saturating_sub(1)
                .min(5),
        )
        .collect::<Vec<_>>();
    parts.join("/")
}

fn path_component_count(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

fn truncate_inline(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut out = text
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

fn context_max_files_for_token_budget(tokens: usize) -> usize {
    (tokens / 1_000).max(1)
}

fn context_snippets_per_file_for_budget(
    requested: usize,
    max_files: usize,
    max_tokens: Option<usize>,
) -> usize {
    let mut cap = CONTEXT_MAX_SNIPPETS_PER_FILE;
    if max_files >= 16 {
        cap = cap.min(2);
    } else if max_files >= 10 {
        cap = cap.min(3);
    }
    if let Some(tokens) = max_tokens {
        if tokens <= 8_000 {
            cap = cap.min(2);
        } else if tokens <= 14_000 {
            cap = cap.min(3);
        }
    }
    requested.min(cap)
}

fn collect_graph_flow_candidates(
    index: &Codebase,
    path_glob: Option<&str>,
    max_files: usize,
) -> Result<Vec<ContextCandidate>> {
    if max_files == 0 {
        return Ok(Vec::new());
    }
    let globset = path_glob.map(build_globset).transpose()?;
    let graph = index.graph();
    let communities = graph_file_communities_from_graph(graph.as_ref());
    let node_count = graph.file_graph.paths.len();
    let mut allowed = vec![false; node_count];
    for (id, path) in graph.file_graph.paths.iter().enumerate() {
        allowed[id] = globset
            .as_ref()
            .is_none_or(|glob| glob.is_match(path.as_str()));
    }
    if !allowed.iter().any(|allowed| *allowed) {
        return Ok(Vec::new());
    }

    let mut seeds_by_path = BTreeMap::<String, GraphFlowSeed>::new();
    let mut community_representatives = BTreeMap::<usize, GraphFlowSeed>::new();
    for (graph_id, path) in graph.file_graph.paths.iter().enumerate() {
        if !allowed[graph_id] {
            continue;
        }
        let outgoing = index
            .deps_for(path)
            .into_iter()
            .filter(|target| {
                graph
                    .file_graph
                    .id(target)
                    .is_some_and(|target_id| allowed[target_id])
            })
            .count();
        let incoming = index
            .reverse_deps_for(path)
            .into_iter()
            .filter(|source| {
                graph
                    .file_graph
                    .id(source)
                    .is_some_and(|source_id| allowed[source_id])
            })
            .count();
        let degree = graph.file_graph.degree(graph_id);
        let community = communities.get(path).copied();
        let cross_community =
            file_graph_cross_community_neighbors(graph.as_ref(), graph_id, &allowed);
        let (symbol_count, line_count) = index
            .file(path)
            .map(|file| (file.symbols.len(), file.line_count))
            .unwrap_or_default();
        let root_priority =
            graph_flow_root_priority(outgoing, incoming, degree, symbol_count, line_count);
        if outgoing > 0 {
            insert_graph_flow_seed(
                &mut seeds_by_path,
                GraphFlowSeed {
                    rank: usize::MAX,
                    priority: (root_priority + 12.0).max(0.1),
                    reach: 0,
                    role: "structural-root",
                    path: path.clone(),
                    community,
                },
            );
        }
        if cross_community > 0 {
            let boundary_priority =
                graph_flow_boundary_priority(root_priority, cross_community, outgoing, degree);
            insert_graph_flow_seed(
                &mut seeds_by_path,
                GraphFlowSeed {
                    rank: usize::MAX,
                    priority: boundary_priority.max(0.1),
                    reach: 0,
                    role: "community-boundary",
                    path: path.clone(),
                    community,
                },
            );
        }
        if let Some(community) = community {
            let representative = GraphFlowSeed {
                rank: usize::MAX,
                priority: (root_priority + ((outgoing > 0) as u8 as f32) * 4.0).max(0.1),
                reach: 0,
                role: "community-root",
                path: path.clone(),
                community: Some(community),
            };
            match community_representatives.get(&community) {
                Some(current) if current.priority >= representative.priority => {}
                _ => {
                    community_representatives.insert(community, representative);
                }
            }
        }
    }
    for representative in community_representatives.into_values() {
        insert_graph_flow_seed(&mut seeds_by_path, representative);
    }
    let allowed_count = allowed.iter().filter(|allowed| **allowed).count();
    let mut preliminary = seeds_by_path
        .values()
        .map(|seed| (seed.path.clone(), seed.priority))
        .collect::<Vec<_>>();
    preliminary.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    let refinement_count = if allowed_count <= 512 {
        preliminary.len()
    } else {
        max_files.saturating_mul(12).max(32).min(preliminary.len())
    };
    for (path, _) in preliminary.into_iter().take(refinement_count) {
        let reach = graph_flow_dependency_reach(index, graph.as_ref(), &allowed, &path, 2);
        if let Some(seed) = seeds_by_path.get_mut(&path) {
            seed.reach = reach;
            seed.priority += graph_flow_reach_bonus(reach);
        }
    }
    let mut teleport = vec![0.0f32; node_count];
    for seed in seeds_by_path.values() {
        if let Some(id) = graph.file_graph.id(&seed.path) {
            teleport[id] = seed.priority.max(0.1);
        }
    }
    let teleport_sum = teleport.iter().sum::<f32>().max(1e-9);
    for value in &mut teleport {
        *value /= teleport_sum;
    }
    let page_rank = graph
        .file_graph
        .personalized_page_rank(&teleport, &allowed, 0.85, 1e-7);
    let max_page_rank = page_rank.iter().copied().fold(0.0f32, f32::max).max(1e-9);
    for seed in seeds_by_path.values_mut() {
        let Some(id) = graph.file_graph.id(&seed.path) else {
            continue;
        };
        let hub_cost = ((graph.file_graph.degree(id) + 1) as f32).powf(0.25);
        seed.priority += page_rank[id] / max_page_rank * 12.0 / hub_cost;
    }
    let mut ranked_seeds = seeds_by_path.into_values().collect::<Vec<_>>();
    ranked_seeds.sort_by(|left, right| {
        right
            .priority
            .total_cmp(&left.priority)
            .then_with(|| left.path.cmp(&right.path))
    });
    ranked_seeds.truncate(max_files.saturating_mul(6).max(12));
    for (rank, seed) in ranked_seeds.iter_mut().enumerate() {
        seed.rank = rank;
    }

    let mut candidates = BTreeMap::<String, ContextCandidate>::new();
    for seed in &ranked_seeds {
        add_candidate_score(
            &mut candidates,
            seed.path.clone(),
            seed.priority,
            format!(
                "{} rank={} community={} reach2={}",
                seed.role,
                seed.rank + 1,
                seed.community
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                seed.reach
            ),
        );
    }

    let bridge_anchors = select_graph_flow_seeds(&ranked_seeds, max_files.clamp(4, 6));
    add_weighted_graph_bridges(graph.as_ref(), &allowed, &bridge_anchors, &mut candidates);

    let items = candidates.into_values().collect::<Vec<_>>();
    let mut items = select_graph_flow_candidates(items, &communities, max_files);
    for item in &mut items {
        if item.ranges.is_empty()
            && let Some(file) = index.file(&item.path)
        {
            let range = structural_candidate_range(index, file);
            item.ranges.push(range);
            item.hit_lines.insert(range.start);
        }
        item.ranges = merged_ranges(item.ranges.clone(), 0, usize::MAX);
    }
    Ok(items)
}

fn graph_flow_root_priority(
    outgoing: usize,
    incoming: usize,
    degree: usize,
    symbol_count: usize,
    line_count: usize,
) -> f32 {
    let outgoing_signal = ((outgoing + 1) as f32).ln() * 18.0;
    let incoming_penalty = ((incoming + 1) as f32).ln() * 11.0;
    let degree_penalty = ((degree + 1) as f32).ln() * 4.0;
    let symbol_complexity = ((symbol_count + 1) as f32).ln() * 3.0;
    let line_complexity = ((line_count + 1) as f32).ln() * 1.5;
    outgoing_signal - incoming_penalty - degree_penalty - symbol_complexity - line_complexity
}

fn graph_flow_boundary_priority(
    root_priority: f32,
    cross_community: usize,
    outgoing: usize,
    degree: usize,
) -> f32 {
    root_priority * 0.55
        + ((cross_community + 1) as f32).ln() * 8.0
        + ((outgoing + 1) as f32).ln() * 2.0
        - ((degree + 1) as f32).ln() * 6.0
}

fn graph_flow_reach_bonus(reach: usize) -> f32 {
    ((reach + 1) as f32).ln() * 9.0
}

fn graph_flow_dependency_reach(
    index: &Codebase,
    graph: &crate::graph::CodeGraph,
    allowed: &[bool],
    start_path: &str,
    max_depth: usize,
) -> usize {
    let Some(start) = graph.file_graph.id(start_path) else {
        return 0;
    };
    let mut seen = BTreeSet::<usize>::from([start]);
    let mut frontier = vec![start];
    for _ in 0..max_depth {
        let mut next = Vec::new();
        for node in frontier {
            let Some(path) = graph.file_graph.paths.get(node) else {
                continue;
            };
            for dependency in index.deps_for(path) {
                let Some(target) = graph.file_graph.id(&dependency) else {
                    continue;
                };
                if !allowed.get(target).copied().unwrap_or(false) || !seen.insert(target) {
                    continue;
                }
                next.push(target);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    seen.len().saturating_sub(1)
}

fn insert_graph_flow_seed(seeds: &mut BTreeMap<String, GraphFlowSeed>, seed: GraphFlowSeed) {
    match seeds.get(&seed.path) {
        Some(current) if current.priority >= seed.priority => {}
        _ => {
            seeds.insert(seed.path.clone(), seed);
        }
    }
}

fn file_graph_cross_community_neighbors(
    graph: &crate::graph::CodeGraph,
    graph_id: usize,
    allowed: &[bool],
) -> usize {
    let Some(community) = graph.file_graph.community(graph_id) else {
        return 0;
    };
    let start = graph.file_graph.offsets.get(graph_id).copied().unwrap_or(0) as usize;
    let end = graph
        .file_graph
        .offsets
        .get(graph_id + 1)
        .copied()
        .unwrap_or(start as u32) as usize;
    graph.file_graph.neighbors[start..end]
        .iter()
        .map(|neighbor| *neighbor as usize)
        .filter(|neighbor| {
            allowed.get(*neighbor).copied().unwrap_or(false)
                && graph.file_graph.community(*neighbor) != Some(community)
        })
        .count()
}

fn graph_file_communities_from_graph(graph: &crate::graph::CodeGraph) -> BTreeMap<String, usize> {
    graph
        .file_graph
        .paths
        .iter()
        .enumerate()
        .filter_map(|(id, path)| Some((path.clone(), graph.file_graph.community(id)?)))
        .collect()
}

fn select_graph_flow_seeds(ranked: &[GraphFlowSeed], limit: usize) -> Vec<GraphFlowSeed> {
    if limit == 0 {
        return Vec::new();
    }
    let mut selected = Vec::new();
    let mut per_community = BTreeMap::<usize, usize>::new();
    for seed in ranked {
        if selected.len() >= limit {
            break;
        }
        if let Some(community) = seed.community {
            let count = per_community.get(&community).copied().unwrap_or(0);
            if count >= 1 {
                continue;
            }
            per_community.insert(community, count + 1);
        }
        selected.push(seed.clone());
    }
    if selected.len() < limit {
        let selected_paths = selected
            .iter()
            .map(|seed| seed.path.clone())
            .collect::<BTreeSet<_>>();
        let remaining = limit - selected.len();
        selected.extend(
            ranked
                .iter()
                .filter(|seed| !selected_paths.contains(&seed.path))
                .take(remaining)
                .cloned(),
        );
    }
    selected
}

fn add_weighted_graph_bridges(
    graph: &crate::graph::CodeGraph,
    allowed: &[bool],
    anchors: &[GraphFlowSeed],
    candidates: &mut BTreeMap<String, ContextCandidate>,
) {
    for left_idx in 0..anchors.len() {
        for right_idx in (left_idx + 1)..anchors.len() {
            let left = &anchors[left_idx];
            let right = &anchors[right_idx];
            let Some(source) = graph.file_graph.id(&left.path) else {
                continue;
            };
            let Some(target) = graph.file_graph.id(&right.path) else {
                continue;
            };
            let path = graph
                .file_graph
                .weighted_shortest_path(source, target, allowed);
            if path.len() < 2 {
                continue;
            }
            let bridge_score = (left.priority + right.priority)
                / 2.0
                / (path.len().saturating_sub(1) as f32).sqrt()
                * 0.22;
            for node_id in path {
                let Some(path) = graph.file_graph.paths.get(node_id).cloned() else {
                    continue;
                };
                add_candidate_score(
                    candidates,
                    path.clone(),
                    bridge_score,
                    format!("weighted graph bridge {} -> {}", left.path, right.path),
                );
                if let Some(candidate) = candidates.get_mut(&path) {
                    candidate.graph_sources.insert(left.path.clone());
                    candidate.graph_sources.insert(right.path.clone());
                }
            }
        }
    }
}

fn structural_candidate_range(index: &Codebase, file: &FileEntry) -> ContextRange {
    if let Ok(rows) = outline_body_followup_candidates(index, file, 1)
        && let Some(row) = rows.first()
    {
        return ContextRange {
            start: row.symbol.line_start,
            end: row.symbol.line_end.max(row.symbol.line_start),
        };
    }
    if let Some(symbol) = file
        .symbols
        .iter()
        .find(|symbol| is_context_handoff_source_symbol(symbol))
        .or_else(|| file.symbols.first())
    {
        return ContextRange {
            start: symbol.line_start,
            end: symbol.line_end.max(symbol.line_start),
        };
    }
    ContextRange { start: 1, end: 1 }
}

fn select_graph_flow_candidates(
    mut candidates: Vec<ContextCandidate>,
    communities: &BTreeMap<String, usize>,
    limit: usize,
) -> Vec<ContextCandidate> {
    candidates.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.graph_sources.len().cmp(&left.graph_sources.len()))
            .then_with(|| left.path.cmp(&right.path))
    });
    let mut selected = Vec::new();
    let mut used_paths = BTreeSet::new();
    let mut per_community = BTreeMap::<usize, usize>::new();
    let diverse_limit = ((limit * 2) / 3).max(1);
    for candidate in &candidates {
        if selected.len() >= diverse_limit {
            break;
        }
        if let Some(community) = communities.get(&candidate.path) {
            let count = per_community.get(community).copied().unwrap_or(0);
            if count >= 2 {
                continue;
            }
            per_community.insert(*community, count + 1);
        }
        if used_paths.insert(candidate.path.clone()) {
            selected.push(candidate.clone());
        }
    }
    for candidate in candidates {
        if selected.len() >= limit {
            break;
        }
        if used_paths.insert(candidate.path.clone()) {
            selected.push(candidate);
        }
    }
    selected
}

fn context_candidate_graph_sources(candidate: &ContextCandidate) -> BTreeSet<String> {
    let mut sources = candidate.graph_sources.clone();
    for reason in &candidate.reasons {
        let Some(rest) = reason.strip_prefix("graph ") else {
            continue;
        };
        let Some((_, source)) = rest.rsplit_once(" of ") else {
            continue;
        };
        if !source.trim().is_empty() {
            sources.insert(source.trim().to_string());
        }
    }
    sources
}

fn ranked_line_hits(
    index: &Codebase,
    query: &str,
    max_results: usize,
    path_glob: Option<&str>,
    compact: bool,
    include_scope: bool,
) -> Result<Vec<SearchHit>> {
    Ok(
        ranked_line_hits_with_scores(index, query, max_results, path_glob, compact, include_scope)?
            .into_iter()
            .map(|(hit, _)| hit)
            .collect(),
    )
}

fn ranked_line_hits_with_scores(
    index: &Codebase,
    query: &str,
    max_results: usize,
    path_glob: Option<&str>,
    compact: bool,
    include_scope: bool,
) -> Result<Vec<(SearchHit, f32)>> {
    ranked_line_hits_with_scores_mode(
        index,
        query,
        max_results,
        path_glob,
        compact,
        include_scope,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn ranked_line_hits_with_scores_mode(
    index: &Codebase,
    query: &str,
    max_results: usize,
    path_glob: Option<&str>,
    compact: bool,
    include_scope: bool,
    allow_semantic: bool,
) -> Result<Vec<(SearchHit, f32)>> {
    let selector = path_glob.map(|glob| index.path_selector(Some(glob)));
    let fetch = max_results.saturating_mul(4).max(max_results);
    let ranked = if allow_semantic {
        hybrid_ranked_chunks(index, query, fetch, selector.as_deref())?
    } else {
        lexical_ranked_chunks(index, query, fetch, selector.as_deref())?
    };
    let definition_names = search_definition_names(index, query);
    let mut hits = Vec::new();
    let mut seen = BTreeSet::<(String, usize)>::new();
    for (chunk_idx, score) in ranked {
        let Some(hit) = best_line_hit_for_chunk(
            index,
            query,
            &definition_names,
            chunk_idx,
            compact,
            include_scope,
        )?
        else {
            continue;
        };
        if seen.insert((hit.path.clone(), hit.line)) {
            let adjusted = score * search_path_penalty(&hit.path, query)
                + search_definition_boost(index, &hit, &definition_names);
            hits.push((hit, adjusted));
        }
    }
    if !query_targets_generated_web_assets(query) {
        hits.retain(|(hit, _)| {
            let normalized = hit.path.replace('\\', "/").to_ascii_lowercase();
            !is_generated_web_asset_path(&normalized)
        });
    }
    hits.sort_by(|(left_hit, left_score), (right_hit, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left_hit.path.cmp(&right_hit.path))
            .then_with(|| left_hit.line.cmp(&right_hit.line))
    });
    apply_ranked_hit_file_saturation(&mut hits);
    hits.truncate(max_results.min(hits.len()));
    Ok(hits)
}

fn query_targets_generated_web_assets(query: &str) -> bool {
    let normalized = query.replace('\\', "/").to_ascii_lowercase();
    normalized.contains("public/assets/")
        || normalized.contains("static/assets/")
        || normalized.contains("public/build/")
        || normalized.contains(".min.js")
        || normalized.contains(".min.css")
        || normalized.contains("minified asset")
        || normalized.contains("generated web asset")
}

fn best_line_hit_for_chunk(
    index: &Codebase,
    query: &str,
    definition_names: &[String],
    chunk_idx: usize,
    compact: bool,
    include_scope: bool,
) -> Result<Option<SearchHit>> {
    let Some(chunk) = index.chunks.get(chunk_idx) else {
        return Ok(None);
    };
    let path = index.chunk_file_path(chunk).to_string();
    let Some(file) = index.file(&path) else {
        return Ok(None);
    };
    let content = index.file_content(file)?;
    let terms = tokenize(query)
        .into_iter()
        .filter(|term| {
            term.chars().count() >= 2 && !CONTEXT_FALLBACK_STOPWORDS.contains(&term.as_str())
        })
        .collect::<BTreeSet<_>>();
    let query_lower = query.to_lowercase();
    let mut best = None::<(usize, String, usize)>;
    let mut fallback = None::<(usize, String)>;
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if line_no < chunk.start_line || line_no > chunk.end_line {
            continue;
        }
        if compact && is_comment_or_blank(line) {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if fallback.is_none() {
            fallback = Some((line_no, trimmed.to_string()));
        }
        let line_terms = tokenize(trimmed).into_iter().collect::<BTreeSet<_>>();
        let mut line_score = terms
            .iter()
            .filter(|term| line_terms.contains(*term))
            .count();
        if file.symbols.iter().any(|symbol| {
            symbol.line_start == line_no
                && definition_names
                    .iter()
                    .any(|name| symbol.name.eq_ignore_ascii_case(name))
        }) {
            line_score += 100;
        }
        let lower = trimmed.to_lowercase();
        if !query_lower.is_empty() && lower.contains(&query_lower) {
            line_score += terms.len().max(1) + 1;
        }
        if line_score == 0 {
            continue;
        }
        let replace = best.as_ref().is_none_or(|(best_score, _, best_line)| {
            line_score > *best_score || (line_score == *best_score && line_no < *best_line)
        });
        if replace {
            best = Some((line_score, trimmed.to_string(), line_no));
        }
    }
    let (line, text) = if let Some((_, text, line)) = best {
        (line, text)
    } else if let Some((line, text)) = fallback {
        (line, text)
    } else {
        return Ok(None);
    };
    let scope = include_scope
        .then(|| scope_for_line(&file.symbols, line))
        .flatten();
    Ok(Some(SearchHit {
        path,
        line,
        text,
        scope,
    }))
}

fn is_ranked_text_query(query: &str) -> bool {
    !query.is_ascii() || !raw_identifiers(query).is_empty()
}

fn search_definition_names(index: &Codebase, query: &str) -> Vec<String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if is_symbol_query(trimmed) {
        let leaf = trimmed
            .rsplit(['.', ':', '#', '/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(trimmed);
        return vec![leaf.to_string()];
    }
    let identifiers = raw_identifiers(trimmed);
    if identifiers.len() == 1
        && identifiers[0] == trimmed
        && !index.symbols_named(trimmed).is_empty()
    {
        return vec![trimmed.to_string()];
    }
    let mut names = identifiers
        .into_iter()
        .filter(|name| {
            name.len() >= 4
                && name.chars().any(|ch| ch.is_ascii_uppercase())
                && name.chars().any(|ch| ch.is_ascii_lowercase())
        })
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn search_definition_boost(index: &Codebase, hit: &SearchHit, definition_names: &[String]) -> f32 {
    if definition_names.is_empty() {
        return 0.0;
    }
    let Some(file) = index.file(&hit.path) else {
        return 0.0;
    };
    let matching = file
        .symbols
        .iter()
        .filter(|symbol| {
            definition_names
                .iter()
                .any(|name| symbol.name.eq_ignore_ascii_case(name))
        })
        .collect::<Vec<_>>();
    if matching.is_empty() {
        return 0.0;
    }
    let mut boost = 6.0;
    if matching.iter().any(|symbol| symbol.line_start == hit.line) {
        boost += 12.0;
    }
    let stem = Path::new(&file.path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .replace('_', "")
        .to_ascii_lowercase();
    if definition_names.iter().any(|name| {
        let name = name.replace('_', "").to_ascii_lowercase();
        stem == name || stem.trim_end_matches('s') == name
    }) {
        boost += 3.0;
    }
    boost
}

fn search_path_penalty(path: &str, query: &str) -> f32 {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    let mut penalty = 1.0;
    let query_targets_tests = query_lower.contains("test") || query_lower.contains("spec");
    if !query_targets_tests
        && (path_has_dir(&normalized, "test")
            || path_has_dir(&normalized, "tests")
            || path_has_dir(&normalized, "__tests__")
            || path_has_dir(&normalized, "spec")
            || normalized.ends_with("_test.rs")
            || normalized.ends_with("_test.py")
            || normalized.contains(".test.")
            || normalized.contains(".spec."))
    {
        penalty *= 0.6;
    }
    if path_has_dir(&normalized, "examples") || path_has_dir(&normalized, "samples") {
        penalty *= 0.6;
    }
    if path_has_dir(&normalized, "editor") || path_has_dir(&normalized, "editorrun") {
        penalty *= 0.25;
    }
    if !query_lower.contains("debug") && path_has_debug_component(&normalized) {
        penalty *= 0.35;
    }
    if normalized.starts_with("library/packagecache/")
        || normalized.contains("/library/packagecache/")
    {
        penalty *= 0.35;
    }
    if normalized.starts_with("assets/plugins/")
        || normalized.contains("/assets/plugins/")
        || path_has_dir(&normalized, "3rdplugins")
    {
        penalty *= 0.3;
    }
    if normalized.starts_with("assets/packages/") || normalized.contains("/assets/packages/") {
        penalty *= 0.55;
    }
    if path_has_dir(&normalized, "compat") || path_has_dir(&normalized, "legacy") {
        penalty *= 0.6;
    }
    if path_has_dir(&normalized, "vendor")
        || path_has_dir(&normalized, "node_modules")
        || path_has_dir(&normalized, "third_party")
    {
        penalty *= 0.4;
    }
    if normalized.ends_with(".d.ts") {
        penalty *= 0.7;
    }
    if is_generated_web_asset_path(&normalized) {
        penalty *= 0.15;
    }
    penalty
}

fn is_generated_web_asset_path(normalized_path: &str) -> bool {
    normalized_path.starts_with("public/assets/")
        || normalized_path.contains("/public/assets/")
        || normalized_path.starts_with("static/assets/")
        || normalized_path.contains("/static/assets/")
        || normalized_path.starts_with("public/build/")
        || normalized_path.contains("/public/build/")
        || normalized_path.ends_with(".min.js")
        || normalized_path.ends_with(".min.css")
}

fn path_has_dir(normalized_path: &str, directory: &str) -> bool {
    normalized_path
        .split('/')
        .any(|component| component == directory)
}

fn path_has_debug_component(normalized_path: &str) -> bool {
    normalized_path.split('/').any(|component| {
        matches!(component, "debug" | "debugger")
            || component.ends_with("debugger")
            || component.ends_with("debugtools")
    })
}

fn apply_ranked_hit_file_saturation(hits: &mut [(SearchHit, f32)]) {
    let mut file_counts = BTreeMap::<String, usize>::new();
    for (hit, score) in hits.iter_mut() {
        let count = file_counts.entry(hit.path.clone()).or_default();
        if *count > 0 {
            *score *= 0.5f32.powi(*count as i32);
        }
        *count += 1;
    }
    hits.sort_by(|(left_hit, left_score), (right_hit, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left_hit.path.cmp(&right_hit.path))
            .then_with(|| left_hit.line.cmp(&right_hit.line))
    });
}

fn add_candidate_score(
    candidates: &mut BTreeMap<String, ContextCandidate>,
    path: String,
    score: f32,
    reason: String,
) {
    let candidate = candidates
        .entry(path.clone())
        .or_insert_with(|| ContextCandidate::new(path));
    candidate.score += score;
    candidate.reasons.insert(reason);
}

fn context_query_identifiers(query: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    push_context_quoted_query_terms(query, &mut seen, &mut out, 3);
    out
}

fn context_graph_query_terms(query: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for term in context_query_identifiers(query) {
        push_context_query_code_term(term, &mut seen, &mut out, 3);
    }
    out.into_iter()
        .map(|term| term.to_ascii_lowercase())
        .collect()
}

fn push_context_quoted_query_terms(
    query: &str,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<String>,
    min_len: usize,
) {
    for quoted in quoted_context_terms(query) {
        for value in std::iter::once(quoted.clone()).chain(split_identifier(&quoted)) {
            push_context_query_code_term(value, seen, out, min_len);
        }
    }
}

fn push_context_query_code_term(
    value: String,
    seen: &mut BTreeSet<String>,
    out: &mut Vec<String>,
    min_len: usize,
) {
    if value.len() < min_len {
        return;
    }
    let key = value.to_ascii_lowercase();
    if seen.insert(key) {
        out.push(value);
    }
}

fn context_symbol_relevant_to_candidate(symbol: &Symbol, candidate: &ContextCandidate) -> bool {
    candidate
        .hit_lines
        .iter()
        .any(|line| symbol.line_start <= *line && symbol.line_end.max(symbol.line_start) >= *line)
        || candidate.ranges.iter().any(|range| {
            let symbol_end = symbol.line_end.max(symbol.line_start);
            symbol.line_start <= range.end && symbol_end >= range.start
        })
}

fn identity_terms_from_text(text: &str) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    for raw in raw_identifiers(text) {
        let raw_lower = raw.to_ascii_lowercase();
        if raw_lower.len() >= 3 && !raw_lower.chars().all(|ch| ch.is_ascii_digit()) {
            terms.insert(raw_lower);
        }
        for part in split_identifier(&raw) {
            if part.len() >= 3 && !part.chars().all(|ch| ch.is_ascii_digit()) {
                terms.insert(part);
            }
        }
        if raw.contains('_') {
            for segment in raw.split('_').filter(|segment| !segment.is_empty()) {
                for part in split_identifier(segment) {
                    if part.len() >= 3 && !part.chars().all(|ch| ch.is_ascii_digit()) {
                        terms.insert(part);
                    }
                }
            }
        }
    }
    terms
}

fn matched_identity_terms(keywords: &[String], identity_terms: &BTreeSet<String>) -> Vec<String> {
    let mut matches = Vec::new();
    for keyword in keywords {
        let keyword = keyword.to_ascii_lowercase();
        if identity_terms
            .iter()
            .any(|term| context_identity_terms_match(&keyword, term))
        {
            matches.push(keyword);
        }
    }
    matches
}

fn context_identity_terms_match(query_term: &str, identity_term: &str) -> bool {
    query_term.eq_ignore_ascii_case(identity_term)
}

fn selected_candidate_symbols<'a>(
    file: &'a FileEntry,
    candidate: &ContextCandidate,
    query: &str,
    limit: usize,
) -> Vec<&'a Symbol> {
    if limit == 0 {
        return Vec::new();
    }
    let mut selected = Vec::new();
    let mut seen = BTreeSet::<(usize, String)>::new();
    if context_candidate_has_direct_line_evidence(candidate) {
        for symbol in context_candidate_direct_symbols(file, candidate, limit) {
            if seen.insert((symbol.line_start, symbol.name.clone())) {
                selected.push(symbol);
                if selected.len() >= limit {
                    return selected;
                }
            }
        }
    }
    for symbol in ranked_context_symbols(file, &candidate.ranges, query, limit.saturating_mul(3)) {
        if seen.insert((symbol.line_start, symbol.name.clone())) {
            selected.push(symbol);
            if selected.len() >= limit {
                return selected;
            }
        }
    }
    selected
}

fn context_candidate_direct_symbols<'a>(
    file: &'a FileEntry,
    candidate: &ContextCandidate,
    limit: usize,
) -> Vec<&'a Symbol> {
    let mut scored = file
        .symbols
        .iter()
        .filter_map(|symbol| {
            let score = context_candidate_direct_symbol_score(symbol, candidate)?;
            let span = symbol_span_lines(symbol);
            Some((score, span, symbol))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.line_start.cmp(&right.2.line_start))
            .then_with(|| left.2.name.cmp(&right.2.name))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, _, symbol)| symbol)
        .collect()
}

fn context_candidate_direct_symbol_score(
    symbol: &Symbol,
    candidate: &ContextCandidate,
) -> Option<usize> {
    if !context_candidate_has_direct_line_evidence(candidate) {
        return None;
    }
    let symbol_end = symbol.line_end.max(symbol.line_start);
    let hit_count = candidate
        .hit_lines
        .iter()
        .filter(|line| symbol.line_start <= **line && **line <= symbol_end)
        .count();
    let range_count = candidate
        .ranges
        .iter()
        .filter(|range| ranges_overlap(symbol.line_start, symbol_end, range.start, range.end))
        .count();
    if hit_count == 0 && range_count == 0 {
        return None;
    }
    let span = symbol_span_lines(symbol);
    let narrow_bonus = 200usize.saturating_sub(span.min(200));
    Some(
        hit_count * 1_000 + range_count * 100 + narrow_bonus + context_symbol_kind_priority(symbol),
    )
}

fn context_candidate_has_direct_line_evidence(candidate: &ContextCandidate) -> bool {
    !candidate.hit_lines.is_empty()
}

fn context_symbol_kind_priority(symbol: &Symbol) -> usize {
    match symbol.kind.as_str() {
        "function" | "method" | "constructor" | "procedure" | "macro" => 30,
        "class" | "interface" | "struct" | "enum" | "record" | "module" | "trait" | "impl" => 22,
        "property" | "field" | "constant" | "const" | "static" | "variable" | "type_alias" => 12,
        _ => 4,
    }
}

fn symbol_span_lines(symbol: &Symbol) -> usize {
    symbol
        .line_end
        .max(symbol.line_start)
        .saturating_sub(symbol.line_start)
        + 1
}

fn symbol_for_target<'a>(file: &'a FileEntry, target: &SymbolTarget) -> Option<&'a Symbol> {
    file.symbols
        .iter()
        .find(|symbol| symbol.line_start == target.line_start && symbol.name == target.name)
}

fn generic_source_path_score(path: &str) -> f32 {
    let normalized = path.replace('\\', "/").to_ascii_lowercase();
    if is_generated_web_asset_path(&normalized) {
        return -120.0;
    }
    if path_has_dir(&normalized, "generate")
        || path_has_dir(&normalized, "generated")
        || path_has_dir(&normalized, "autogen")
        || path_has_dir(&normalized, "generated_code")
    {
        return -60.0;
    }
    if path_has_dir(&normalized, "vendor")
        || path_has_dir(&normalized, "node_modules")
        || path_has_dir(&normalized, "third_party")
        || path_has_dir(&normalized, "3rdplugins")
    {
        return -80.0;
    }
    if path_has_dir(&normalized, "editor") || path_has_dir(&normalized, "editorrun") {
        return -70.0;
    }
    if path_has_debug_component(&normalized) {
        return -50.0;
    }
    if normalized.starts_with("library/packagecache/")
        || normalized.contains("/library/packagecache/")
    {
        return -45.0;
    }
    if normalized.starts_with("assets/plugins/") || normalized.contains("/assets/plugins/") {
        return -40.0;
    }
    if normalized.starts_with("assets/packages/") || normalized.contains("/assets/packages/") {
        return -30.0;
    }
    if path_has_dir(&normalized, "test")
        || path_has_dir(&normalized, "tests")
        || path_has_dir(&normalized, "__tests__")
        || path_has_dir(&normalized, "spec")
        || normalized.ends_with("_test.rs")
        || normalized.ends_with("_test.py")
        || normalized.contains(".test.")
        || normalized.contains(".spec.")
    {
        return -35.0;
    }
    if path_has_dir(&normalized, "examples")
        || path_has_dir(&normalized, "samples")
        || path_has_dir(&normalized, "compat")
        || path_has_dir(&normalized, "legacy")
        || path_has_dir(&normalized, "docs")
    {
        return -25.0;
    }
    if normalized.ends_with(".d.ts") {
        return -20.0;
    }
    0.0
}

fn configured_scan_root_order_score(index: &Codebase, path: &str) -> f32 {
    if index.options.root_paths.is_empty() {
        return 0.0;
    }
    let normalized_path = normalize_rel_path(path);
    let root_count = index.options.root_paths.len();
    for (idx, root) in index.options.root_paths.iter().enumerate() {
        let normalized_root = normalize_rel_path(root);
        if normalized_root.is_empty() {
            continue;
        }
        if normalized_path == normalized_root
            || normalized_path.starts_with(&(normalized_root.clone() + "/"))
        {
            return (root_count - idx) as f32 * 12.0;
        }
    }
    0.0
}

fn append_context_symbol_handoff_leads(
    index: &Codebase,
    file: &FileEntry,
    symbols: &[&Symbol],
    query_terms: &[String],
    seen: &mut BTreeSet<(String, usize, String)>,
    emitted_total: &mut usize,
    out: &mut String,
) -> Result<()> {
    if symbols.is_empty() || *emitted_total >= CONTEXT_SYMBOL_HANDOFF_GLOBAL_LIMIT {
        return Ok(());
    }
    let content = index.file_content(file)?;
    let mut section = String::new();
    let mut emitted_file = 0usize;
    for symbol in symbols {
        if emitted_file >= CONTEXT_SYMBOL_HANDOFF_PER_FILE_LIMIT
            || *emitted_total >= CONTEXT_SYMBOL_HANDOFF_GLOBAL_LIMIT
        {
            break;
        }
        if !is_context_handoff_source_symbol(symbol) {
            continue;
        }
        let symbol_end = symbol.line_end.max(symbol.line_start);
        let span = symbol_end.saturating_sub(symbol.line_start) + 1;
        if span > CONTEXT_SYMBOL_HANDOFF_MAX_SOURCE_LINES {
            continue;
        }
        let body = source_line_slice(&content, symbol.line_start, symbol_end);
        let leads = symbol_body_leads_with_terms(
            index,
            file,
            symbol,
            &body,
            CONTEXT_SYMBOL_HANDOFF_PER_SYMBOL_LIMIT,
            query_terms,
        );
        if leads.is_empty() {
            continue;
        }
        for lead in leads {
            let key = (
                lead.target.path.clone(),
                lead.target.line_start,
                lead.target.name.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            if section.is_empty() {
                section.push_str("   symbol handoff leads:\n");
            }
            section.push_str(&format!(
                "     {} L{} -> {} {}:L{} ({}) // {}\n",
                symbol.name,
                symbol.line_start,
                lead.target.name,
                lead.target.path,
                lead.target.line_start,
                lead.target.kind,
                lead.target.detail
            ));
            emitted_file += 1;
            *emitted_total += 1;
            if emitted_file >= CONTEXT_SYMBOL_HANDOFF_PER_FILE_LIMIT
                || *emitted_total >= CONTEXT_SYMBOL_HANDOFF_GLOBAL_LIMIT
            {
                break;
            }
        }
    }
    out.push_str(&section);
    Ok(())
}

fn is_context_handoff_source_symbol(symbol: &Symbol) -> bool {
    matches!(
        symbol.kind.as_str(),
        "function" | "method" | "constructor" | "property" | "macro" | "impl" | "module" | "symbol"
    )
}

fn ranked_context_symbols<'a>(
    file: &'a FileEntry,
    ranges: &[ContextRange],
    query: &str,
    limit: usize,
) -> Vec<&'a Symbol> {
    let identifiers = context_query_identifiers(query);
    let mut scored = Vec::new();
    for symbol in &file.symbols {
        let score = context_symbol_score(symbol, ranges, &identifiers);
        if score > 0.0 {
            scored.push((score, symbol));
        }
    }
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left.line_start.cmp(&right.line_start))
            .then_with(|| left.name.cmp(&right.name))
    });
    let mut selected = Vec::new();
    let mut selected_lines = BTreeSet::new();
    let mut covered_terms = BTreeSet::new();
    if identifiers.is_empty() && ranges.is_empty() {
        let state_quota = state_symbol_summary_quota(limit);
        for symbol in file
            .symbols
            .iter()
            .filter(|symbol| is_state_summary_symbol(symbol))
        {
            if selected.len() >= state_quota {
                break;
            }
            if selected_lines.insert(symbol.line_start) {
                selected.push(symbol);
            }
        }
    }
    for (_, symbol) in &scored {
        if selected.len() >= limit {
            break;
        }
        let terms = matched_symbol_query_terms(symbol, &identifiers);
        if terms.is_empty() || terms.iter().all(|term| covered_terms.contains(term)) {
            continue;
        }
        if selected_lines.insert(symbol.line_start) {
            covered_terms.extend(terms);
            selected.push(*symbol);
        }
    }
    for (_, symbol) in scored {
        if selected.len() >= limit {
            break;
        }
        if selected_lines.insert(symbol.line_start) {
            selected.push(symbol);
        }
    }
    if selected.is_empty() {
        selected.extend(file.symbols.iter().take(limit));
    }
    selected
}

fn state_symbol_summary_quota(limit: usize) -> usize {
    if limit >= 8 {
        3
    } else if limit >= 4 {
        2
    } else {
        0
    }
}

fn is_state_summary_symbol(symbol: &Symbol) -> bool {
    matches!(
        symbol.kind.as_str(),
        "property" | "field" | "variable" | "const" | "static" | "enum"
    )
}

fn matched_symbol_query_terms(symbol: &Symbol, identifiers: &[String]) -> BTreeSet<String> {
    let mut matches =
        matched_query_terms_for_identity(&identity_terms_from_text(&symbol.name), identifiers);
    if matches.is_empty() {
        matches = matched_query_terms_for_identity(
            &identity_terms_from_text(&symbol.detail),
            identifiers,
        );
    }
    matches
}

fn matched_query_terms_for_identity(
    identity_terms: &BTreeSet<String>,
    identifiers: &[String],
) -> BTreeSet<String> {
    let mut matches = BTreeSet::new();
    for token in identifiers {
        let token = token.to_ascii_lowercase();
        if identity_terms
            .iter()
            .any(|term| context_identity_terms_match(&token, term))
        {
            matches.insert(token);
        }
    }
    matches
}

fn context_symbol_score(symbol: &Symbol, ranges: &[ContextRange], identifiers: &[String]) -> f32 {
    let mut score = 0.0;
    let symbol_end = symbol.line_end.max(symbol.line_start);
    let range_overlaps = ranges
        .iter()
        .filter(|range| ranges_overlap(symbol.line_start, symbol_end, range.start, range.end))
        .count();
    if range_overlaps > 0 {
        score += 4.0 + range_overlaps.min(4) as f32;
    }

    let name_terms = identity_terms_from_text(&symbol.name);
    let detail_terms = identity_terms_from_text(&symbol.detail);
    for token in identifiers {
        let token = token.to_ascii_lowercase();
        if name_terms
            .iter()
            .any(|term| context_identity_terms_match(&token, term))
        {
            score += 5.0;
        } else if detail_terms
            .iter()
            .any(|term| context_identity_terms_match(&token, term))
        {
            score += 2.0;
        } else if has_whole_word(&symbol.detail, &token) {
            score += 1.0;
        }
    }

    score += match symbol.kind.as_str() {
        "function" | "method" | "constructor" => 1.5,
        "class" | "interface" | "struct" | "enum" | "record" => 0.75,
        _ => 0.0,
    };

    let span = symbol_end.saturating_sub(symbol.line_start) + 1;
    if span > 1000 {
        score *= 0.25;
    } else if span > 400 {
        score *= 0.5;
    }
    score
}

fn ranges_overlap(a_start: usize, a_end: usize, b_start: usize, b_end: usize) -> bool {
    a_start <= b_end && b_start <= a_end
}

fn append_compact_deps(index: &Codebase, path: &str, query: &str, out: &mut String, indent: &str) {
    let deps = ranked_related_paths(index, path, index.deps_for(path), query, 4);
    let incoming = ranked_related_paths(index, path, index.reverse_deps_for(path), query, 4);
    out.push_str(&format!(
        "{indent}deps: depends_on={} imported_by={}\n",
        index.deps_for(path).len(),
        index.reverse_deps_for(path).len()
    ));
    if !deps.is_empty() {
        out.push_str(&format!("{indent}  depends_on: {}\n", deps.join(", ")));
    }
    if !incoming.is_empty() {
        out.push_str(&format!("{indent}  imported_by: {}\n", incoming.join(", ")));
    }
}

fn append_context_graph_trails(
    index: &Codebase,
    candidates: &[ContextCandidate],
    query: &str,
    out: &mut String,
    limit: usize,
) {
    let trails = collect_context_graph_trails(index, candidates, query, limit);
    if trails.is_empty() {
        return;
    }
    out.push_str("graph leads:\n");
    for trail in trails {
        out.push_str(&format!(
            "  {} via {} {} d{} score={:.2}\n",
            trail.path, trail.direction, trail.via, trail.distance, trail.score
        ));
    }
}

fn collect_context_graph_trails(
    index: &Codebase,
    candidates: &[ContextCandidate],
    query: &str,
    limit: usize,
) -> Vec<ContextGraphTrail> {
    if limit == 0 {
        return Vec::new();
    }
    let selected = candidates
        .iter()
        .map(|candidate| candidate.path.clone())
        .collect::<BTreeSet<_>>();
    let mut best = BTreeMap::<String, ContextGraphTrail>::new();
    for seed in candidates.iter().take(CONTEXT_GRAPH_TRAIL_SEEDS) {
        let direct =
            ranked_direct_graph_neighbors(index, &seed.path, query, CONTEXT_GRAPH_TRAIL_FANOUT);
        for neighbor in &direct {
            if selected.contains(&neighbor.path) {
                continue;
            }
            insert_context_graph_trail(
                &mut best,
                ContextGraphTrail {
                    path: neighbor.path.clone(),
                    via: seed.path.clone(),
                    direction: neighbor.direction,
                    distance: 1,
                    score: seed.score * 0.08 + neighbor.score,
                },
            );
        }
        for neighbor in direct.iter().take(CONTEXT_GRAPH_TRAIL_FANOUT / 2) {
            let second = ranked_direct_graph_neighbors(
                index,
                &neighbor.path,
                query,
                CONTEXT_GRAPH_TRAIL_FANOUT,
            );
            for next in second {
                if next.path == seed.path || selected.contains(&next.path) {
                    continue;
                }
                insert_context_graph_trail(
                    &mut best,
                    ContextGraphTrail {
                        path: next.path,
                        via: neighbor.path.clone(),
                        direction: next.direction,
                        distance: 2,
                        score: seed.score * 0.04 + neighbor.score * 0.35 + next.score,
                    },
                );
            }
        }
    }
    let mut trails = best.into_values().collect::<Vec<_>>();
    trails.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.distance.cmp(&right.distance))
            .then_with(|| left.path.cmp(&right.path))
    });
    select_diverse_context_graph_trails(trails, limit)
}

fn insert_context_graph_trail(
    best: &mut BTreeMap<String, ContextGraphTrail>,
    trail: ContextGraphTrail,
) {
    match best.get(&trail.path) {
        Some(current) if current.score >= trail.score => {}
        _ => {
            best.insert(trail.path.clone(), trail);
        }
    }
}

fn select_diverse_context_graph_trails(
    trails: Vec<ContextGraphTrail>,
    limit: usize,
) -> Vec<ContextGraphTrail> {
    let mut selected = Vec::new();
    let mut used_paths = BTreeSet::new();
    let mut per_via = BTreeMap::<String, usize>::new();
    for trail in &trails {
        if selected.len() >= limit {
            return selected;
        }
        let count = per_via.get(&trail.via).copied().unwrap_or(0);
        if count >= CONTEXT_GRAPH_TRAIL_PER_VIA_LIMIT {
            continue;
        }
        if used_paths.insert(trail.path.clone()) {
            per_via.insert(trail.via.clone(), count + 1);
            selected.push(trail.clone());
        }
    }
    for trail in trails {
        if selected.len() >= limit {
            break;
        }
        if used_paths.insert(trail.path.clone()) {
            selected.push(trail);
        }
    }
    selected
}

fn related_path_structure_evidence(source_path: &str, related_path: &str) -> f32 {
    let source_components = path_components(source_path);
    let related_components = path_components(related_path);
    let mut score = common_prefix_len(&source_components, &related_components) as f32;
    let source_stem_terms = file_stem_identity_terms(source_path);
    let related_stem_terms = file_stem_identity_terms(related_path);
    let shared_stem_terms = source_stem_terms
        .iter()
        .filter(|term| {
            term.len() >= 5
                && related_stem_terms
                    .iter()
                    .any(|related| context_identity_terms_match(term, related))
        })
        .count();
    score += shared_stem_terms.min(4) as f32 * 0.75;
    score
}

fn file_stem_identity_terms(path: &str) -> BTreeSet<String> {
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(path);
    identity_terms_from_text(stem)
}

fn ranked_direct_graph_neighbors(
    index: &Codebase,
    source_path: &str,
    query: &str,
    limit: usize,
) -> Vec<ContextGraphNeighbor> {
    let query_terms = context_graph_query_terms(query);
    let source_components = path_components(source_path);
    let mut best = BTreeMap::<String, ContextGraphNeighbor>::new();
    for (direction, paths) in [
        ("depends_on", index.deps_for(source_path)),
        ("imported_by", index.reverse_deps_for(source_path)),
    ] {
        for path in paths {
            if path == source_path {
                continue;
            }
            let score = related_path_score(index, &source_components, &path, &query_terms)
                + graph_connectivity_prior(index, &path);
            let neighbor = ContextGraphNeighbor {
                path: path.clone(),
                direction,
                score,
            };
            match best.get(&path) {
                Some(current) if current.score >= neighbor.score => {}
                _ => {
                    best.insert(path, neighbor);
                }
            }
        }
    }
    let mut neighbors = best.into_values().collect::<Vec<_>>();
    neighbors.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    neighbors.truncate(limit);
    neighbors
}

fn graph_connectivity_prior(index: &Codebase, path: &str) -> f32 {
    let outgoing = index.deps_for(path).len();
    let incoming = index.reverse_deps_for(path).len();
    context_graph_focus_prior(outgoing, incoming)
}

fn context_graph_focus_prior(outgoing: usize, incoming: usize) -> f32 {
    let total = outgoing + incoming;
    if total == 0 {
        return 0.0;
    }
    let mut score =
        ((outgoing.min(48) + 1) as f32).ln() * 0.45 + ((incoming.min(64) + 1) as f32).ln() * 0.25;
    if outgoing > 0 && incoming > 0 {
        score += 0.5;
    }
    if incoming > 600 && outgoing.saturating_mul(20) < incoming {
        score -= 80.0;
    } else if incoming > 300 && outgoing.saturating_mul(12) < incoming {
        score -= 40.0;
    } else if incoming > 180 && outgoing.saturating_mul(8) < incoming {
        score -= 18.0;
    } else if total > 700 {
        score -= 12.0;
    }
    score
}

fn ranked_related_paths(
    index: &Codebase,
    source_path: &str,
    paths: Vec<String>,
    query: &str,
    limit: usize,
) -> Vec<String> {
    let query_terms = context_graph_query_terms(query);
    let source_components = path_components(source_path);
    let mut scored = paths
        .into_iter()
        .map(|path| {
            let score = related_path_score(index, &source_components, &path, &query_terms);
            (score, path)
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(left_score, left_path), (right_score, right_path)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left_path.cmp(right_path))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, path)| path)
        .collect()
}

fn related_path_score(
    index: &Codebase,
    source_components: &[String],
    path: &str,
    query_terms: &[String],
) -> f32 {
    let mut score = common_prefix_len(source_components, &path_components(path)) as f32;
    let path_terms = identity_terms_from_text(path);
    for term in query_terms {
        if path_terms
            .iter()
            .any(|path_term| context_identity_terms_match(term, path_term))
        {
            score += 3.0;
        }
    }
    if let Some(file) = index.file(path) {
        for symbol in file.symbols.iter().take(16) {
            let symbol_terms = identity_terms_from_text(&symbol.name);
            if query_terms.iter().any(|term| {
                symbol_terms
                    .iter()
                    .any(|symbol_term| context_identity_terms_match(term, symbol_term))
            }) {
                score += 1.5;
                break;
            }
        }
    }
    score
}

fn path_components(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_ascii_lowercase())
        .collect()
}

fn common_prefix_len(left: &[String], right: &[String]) -> usize {
    left.iter()
        .zip(right.iter())
        .take_while(|(left, right)| left == right)
        .count()
}

fn append_context_snippets(
    index: &Codebase,
    file: &FileEntry,
    candidate: &ContextCandidate,
    out: &mut String,
    radius: usize,
    limit: usize,
    max_chars: usize,
) -> Result<()> {
    let ranges = context_snippet_ranges(file, candidate, radius, limit);
    if ranges.is_empty() {
        return Ok(());
    }
    let content = index.file_content(file)?;
    let mut emitted = 0usize;
    let mut used_chars = 0usize;
    let mut section = String::new();
    for range in ranges {
        if emitted >= limit {
            break;
        }
        let lines = extract_lines(&content, range.start, range.end, true);
        if lines.trim().is_empty() {
            continue;
        }
        let block = format!(
            "   snippet L{}-L{}:\n{}",
            range.start,
            range.end,
            indent_block(&lines, "     ")
        );
        if used_chars + block.len() > max_chars {
            break;
        }
        section.push_str(&block);
        used_chars += block.len();
        emitted += 1;
    }
    if !section.is_empty() {
        out.push_str("   evidence:\n");
        out.push_str(&section);
    }
    Ok(())
}

fn context_snippet_ranges(
    file: &FileEntry,
    candidate: &ContextCandidate,
    radius: usize,
    limit: usize,
) -> Vec<ContextRange> {
    if file.line_count <= READ_COMPACT_MAX_LINES {
        return vec![ContextRange {
            start: 1,
            end: file.line_count.max(1),
        }];
    }
    let mut ranges = Vec::new();
    for line in &candidate.hit_lines {
        ranges.push(centered_context_range(*line, radius, file.line_count));
    }
    for range in &candidate.ranges {
        let anchor = range.start.min(file.line_count.max(1));
        ranges.push(centered_context_range(anchor, radius, file.line_count));
    }
    merged_ranges(ranges, radius, file.line_count.max(1))
        .into_iter()
        .take(limit)
        .collect()
}

fn centered_context_range(line: usize, radius: usize, max_line: usize) -> ContextRange {
    let max_line = max_line.max(1);
    let line = line.clamp(1, max_line);
    ContextRange {
        start: line.saturating_sub(radius).max(1),
        end: line.saturating_add(radius).min(max_line),
    }
}

fn indent_block(text: &str, indent: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        out.push_str(indent);
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn merged_ranges(mut ranges: Vec<ContextRange>, gap: usize, max_line: usize) -> Vec<ContextRange> {
    ranges.retain(|range| range.start > 0 && range.end > 0);
    ranges.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.end.cmp(&b.end)));
    let mut merged = Vec::<ContextRange>::new();
    for mut range in ranges {
        range.start = range.start.min(max_line);
        range.end = range.end.min(max_line);
        if range.start > range.end {
            continue;
        }
        if let Some(last) = merged.last_mut()
            && range.start <= last.end.saturating_add(gap.saturating_add(1))
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }
    merged
}

#[derive(Clone)]
struct GlobActionableLead {
    source_path: String,
    source_line: usize,
    source_name: String,
    relation: &'static str,
    source_text: Option<String>,
    target: SymbolTarget,
    score: usize,
}

fn handle_glob(index: &Codebase, args: &Value) -> Result<String> {
    let pattern = required_str(args, "pattern")?;
    let max_results = get_usize(args, "max_results").unwrap_or(200).clamp(1, 5000);
    let include_symbols = get_bool_default(args, "include_symbols", true);
    let summary_limit = get_usize(args, "summary_limit")
        .unwrap_or(GLOB_CENTER_SUMMARY_RESULT_LIMIT)
        .clamp(0, GLOB_CENTER_SUMMARY_RESULT_LIMIT);
    let include_paths = get_bool(args, "include_paths");
    let requested_actionable_leads = get_bool_default(args, "include_actionable_leads", false);
    let glob = build_globset(&pattern)?;
    let mut all_matches = index
        .files
        .keys()
        .filter(|path| glob.is_match(path.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    all_matches.sort();
    let matches = all_matches
        .iter()
        .take(max_results)
        .cloned()
        .collect::<Vec<_>>();
    if all_matches.is_empty() {
        return Ok("no matches".to_string());
    }
    if !include_symbols || summary_limit == 0 {
        return Ok(matches.join("\n") + "\n");
    }

    let central_paths = graph_ranked_paths(index, &all_matches, summary_limit);
    let action_scope = if requested_actionable_leads {
        if all_matches.len() <= GLOB_ACTIONABLE_MAX_MATCHES {
            all_matches.clone()
        } else {
            graph_ranked_paths(
                index,
                &all_matches,
                GLOB_ACTIONABLE_FILE_SCAN_LIMIT.min(all_matches.len()),
            )
        }
    } else {
        Vec::new()
    };
    let action_leads = if requested_actionable_leads {
        collect_glob_actionable_leads(index, &action_scope, GLOB_ACTIONABLE_LEAD_LIMIT)?
    } else {
        Vec::new()
    };
    let mut out = format!("{} matches for {pattern}\n", all_matches.len());
    out.push_str("glob page\n");
    if !include_paths {
        out.push_str("paths omitted; set include_paths=true if needed.\n");
    }
    if requested_actionable_leads && all_matches.len() > GLOB_ACTIONABLE_MAX_MATCHES {
        out.push_str(&format!(
            "action leads scoped to top {} graph-ranked matches.\n",
            action_scope.len()
        ));
    } else if !requested_actionable_leads {
        out.push_str("action leads off.\n");
    }
    if include_paths && matches.len() < all_matches.len() {
        out.push_str(&format!("(paths truncated max_results={max_results})\n"));
    }
    out.push_str("central samples:\n");
    for path in &central_paths {
        let Some(file) = index.file(path) else {
            continue;
        };
        let graph_degree = file_graph_degree(index, path);
        let symbols = compact_file_symbol_summary(file, GLOB_SYMBOL_SUMMARY_PER_FILE);
        out.push_str(&format!(
            "  - {} ({}L {}sym deg={})",
            path,
            file.line_count,
            file.symbols.len(),
            graph_degree
        ));
        if !symbols.is_empty() {
            out.push_str(&format!(" symbols: {}", symbols.join("; ")));
        }
        out.push('\n');
    }
    if !action_leads.is_empty() {
        out.push_str("action leads:\n");
        for lead in &action_leads {
            let source_text = lead
                .source_text
                .as_deref()
                .map(|text| format!(" // {}", compact_inline_text(text, 120)))
                .unwrap_or_default();
            out.push_str(&format!(
                "  - {}:{} {} -{}-> {} {}:{} ({}) score={}{}\n",
                lead.source_path,
                lead.source_line,
                lead.source_name,
                lead.relation,
                lead.target.name,
                lead.target.path,
                lead.target.line_start,
                lead.target.kind,
                lead.score,
                source_text
            ));
        }
        let assignment_followups = action_leads
            .iter()
            .filter(|lead| lead.relation == "assign")
            .take(2)
            .collect::<Vec<_>>();
        if !assignment_followups.is_empty() {
            out.push_str("assign followups:\n");
            for lead in assignment_followups {
                out.push_str(&format!(
                    "  - codedb_symbol name={} path={} body=true max_results=1\n",
                    lead.target.name, lead.target.path
                ));
            }
        }
    }
    if include_paths {
        out.push_str("paths:\n");
        for path in matches {
            out.push_str(&path);
            out.push('\n');
        }
    }
    Ok(out)
}

fn collect_glob_actionable_leads(
    index: &Codebase,
    paths: &[String],
    limit: usize,
) -> Result<Vec<GlobActionableLead>> {
    if limit == 0 || paths.is_empty() {
        return Ok(Vec::new());
    }
    let ranked_paths = graph_ranked_paths(
        index,
        paths,
        GLOB_ACTIONABLE_FILE_SCAN_LIMIT.min(paths.len()),
    );
    let mut leads = Vec::new();
    let mut seen = BTreeSet::<(String, usize, String, String, usize, String)>::new();
    for path in &ranked_paths {
        let Some(file) = index.file(&path) else {
            continue;
        };
        let content = index.file_content(file)?;
        for symbol in glob_actionable_source_symbols(file) {
            let symbol_end = symbol.line_end.max(symbol.line_start);
            let span = symbol_end.saturating_sub(symbol.line_start) + 1;
            if span > GLOB_ACTIONABLE_MAX_SYMBOL_LINES {
                continue;
            }
            let body = source_line_slice(&content, symbol.line_start, symbol_end);
            for assignment_lead in symbol_body_assignment_target_leads(
                index,
                file,
                symbol,
                &body,
                GLOB_ACTIONABLE_LEADS_PER_SYMBOL,
            ) {
                let key = (
                    file.path.clone(),
                    symbol.line_start,
                    symbol.name.clone(),
                    assignment_lead.target.path.clone(),
                    assignment_lead.target.line_start,
                    assignment_lead.target.name.clone(),
                );
                if !seen.insert(key) {
                    continue;
                }
                let score =
                    glob_assignment_target_lead_score(index, file, symbol, &assignment_lead.target);
                leads.push(GlobActionableLead {
                    source_path: file.path.clone(),
                    source_line: assignment_lead.line,
                    source_name: symbol.name.clone(),
                    relation: "assign",
                    source_text: Some(assignment_lead.text),
                    target: assignment_lead.target,
                    score,
                });
            }
        }
    }
    let mut assignment_leads = leads;
    sort_glob_actionable_leads(&mut assignment_leads);
    let assignment_limit = if limit <= 2 { limit } else { limit.div_ceil(2) };
    let mut selected =
        select_diverse_glob_actionable_leads(assignment_leads.clone(), assignment_limit.min(limit));

    let mut ref_leads = Vec::new();
    for path in &ranked_paths {
        let Some(file) = index.file(&path) else {
            continue;
        };
        let content = index.file_content(file)?;
        for symbol in glob_actionable_source_symbols(file) {
            let symbol_end = symbol.line_end.max(symbol.line_start);
            let span = symbol_end.saturating_sub(symbol.line_start) + 1;
            if span > GLOB_ACTIONABLE_MAX_SYMBOL_LINES {
                continue;
            }
            let body = source_line_slice(&content, symbol.line_start, symbol_end);
            for body_lead in
                symbol_body_leads(index, file, symbol, &body, GLOB_ACTIONABLE_LEADS_PER_SYMBOL)
            {
                let key = (
                    file.path.clone(),
                    symbol.line_start,
                    symbol.name.clone(),
                    body_lead.target.path.clone(),
                    body_lead.target.line_start,
                    body_lead.target.name.clone(),
                );
                if !seen.insert(key) {
                    continue;
                }
                let score = glob_actionable_lead_score(index, file, symbol, &body, &body_lead);
                ref_leads.push(GlobActionableLead {
                    source_path: file.path.clone(),
                    source_line: symbol.line_start,
                    source_name: symbol.name.clone(),
                    relation: "ref",
                    source_text: None,
                    target: body_lead.target,
                    score,
                });
            }
        }
    }
    sort_glob_actionable_leads(&mut ref_leads);
    let ref_limit = limit.saturating_sub(selected.len());
    extend_unique_glob_actionable_leads(
        &mut selected,
        select_diverse_glob_actionable_leads(ref_leads.clone(), ref_limit),
        limit,
    );
    if selected.len() < limit {
        let mut combined = assignment_leads;
        combined.extend(ref_leads);
        sort_glob_actionable_leads(&mut combined);
        extend_unique_glob_actionable_leads(
            &mut selected,
            select_diverse_glob_actionable_leads(combined, limit),
            limit,
        );
    }
    sort_glob_actionable_leads(&mut selected);
    Ok(selected)
}

fn glob_actionable_source_symbols(file: &FileEntry) -> Vec<&Symbol> {
    let symbols = file
        .symbols
        .iter()
        .filter(|symbol| is_context_handoff_source_symbol(symbol))
        .collect::<Vec<_>>();
    let limit = GLOB_ACTIONABLE_SYMBOL_SCAN_LIMIT.min(symbols.len());
    if symbols.len() <= limit {
        return symbols;
    }
    let mut indices = BTreeSet::<usize>::new();
    let head = (limit / 4).max(4).min(limit);
    let tail = (limit / 4).max(4).min(limit.saturating_sub(head));
    for idx in 0..head.min(symbols.len()) {
        indices.insert(idx);
    }
    for idx in symbols.len().saturating_sub(tail)..symbols.len() {
        indices.insert(idx);
    }
    if limit > 1 {
        for slot in 0..limit {
            let idx = slot * (symbols.len() - 1) / (limit - 1);
            indices.insert(idx);
            if indices.len() >= limit {
                break;
            }
        }
    }
    for idx in 0..symbols.len() {
        if indices.len() >= limit {
            break;
        }
        indices.insert(idx);
    }
    indices
        .into_iter()
        .filter_map(|idx| symbols.get(idx).copied())
        .collect()
}

fn extend_unique_glob_actionable_leads(
    selected: &mut Vec<GlobActionableLead>,
    candidates: Vec<GlobActionableLead>,
    limit: usize,
) {
    let mut used = selected
        .iter()
        .map(glob_actionable_lead_identity)
        .collect::<BTreeSet<_>>();
    for lead in candidates {
        if selected.len() >= limit {
            break;
        }
        if used.insert(glob_actionable_lead_identity(&lead)) {
            selected.push(lead);
        }
    }
}

fn glob_actionable_lead_identity(
    lead: &GlobActionableLead,
) -> (String, usize, String, String, usize, String) {
    (
        lead.source_path.clone(),
        lead.source_line,
        lead.source_name.clone(),
        lead.target.path.clone(),
        lead.target.line_start,
        lead.target.name.clone(),
    )
}

fn sort_glob_actionable_leads(leads: &mut [GlobActionableLead]) {
    leads.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.source_path.cmp(&right.source_path))
            .then_with(|| left.source_line.cmp(&right.source_line))
            .then_with(|| left.target.path.cmp(&right.target.path))
            .then_with(|| left.target.line_start.cmp(&right.target.line_start))
    });
}

fn glob_actionable_lead_score(
    index: &Codebase,
    source_file: &FileEntry,
    source_symbol: &Symbol,
    body: &str,
    lead: &BodySymbolLead,
) -> usize {
    let target = &lead.target;
    let mut score = lead.score as isize;
    if target.path != source_file.path {
        score += 80;
    } else {
        score -= 30;
    }
    score += glob_path_separation_score(&source_file.path, &target.path);
    score += match target.kind.as_str() {
        "method" | "function" => 40,
        "constructor" => -70,
        _ => 0,
    };
    if generic_source_path_score(&target.path) < 0.0 {
        score -= 80;
    }
    score += glob_target_graph_focus_score(index, &target.path);
    if source_symbol.name.starts_with("__") || target.name.starts_with("__") {
        score -= 60;
    }
    score += body_exact_reference_terms(body, 1).len() as isize * 12;
    score += symbol_name_specificity_weight(source_symbol) as isize;
    score.max(0) as usize
}

fn glob_assignment_target_lead_score(
    index: &Codebase,
    source_file: &FileEntry,
    source_symbol: &Symbol,
    target: &SymbolTarget,
) -> usize {
    let mut score = 320isize;
    if target.path != source_file.path {
        score += 90;
        if same_path_family(&source_file.path, &target.path) {
            score += 90;
        }
    } else {
        score += 20;
    }
    score += match target.kind.as_str() {
        "property" | "field" | "variable" | "const" | "static" => 70,
        "method" | "function" => 12,
        _ => 0,
    };
    if generic_source_path_score(&target.path) < 0.0 {
        score -= 80;
    }
    score += glob_target_graph_focus_score(index, &target.path);
    if source_symbol.name.starts_with("__") || target.name.starts_with("__") {
        score -= 50;
    }
    score += symbol_name_specificity_weight(source_symbol) as isize;
    score.max(0) as usize
}

fn compact_symbol_target_snippet_limited(
    index: &Codebase,
    target: &SymbolTarget,
    max_lines: usize,
    max_chars: usize,
) -> Option<String> {
    let file = index.file(&target.path)?;
    let symbol = symbol_for_target(file, target)?;
    let start = symbol.line_start;
    let mut full_end = symbol.line_end.max(symbol.line_start);
    if full_end <= start {
        let next_symbol_start = file
            .symbols
            .iter()
            .filter(|candidate| candidate.line_start > start)
            .map(|candidate| candidate.line_start)
            .min();
        full_end = next_symbol_start
            .and_then(|line| line.checked_sub(1))
            .unwrap_or(file.line_count)
            .max(start);
    }
    let full_end = full_end.min(file.line_count.max(1));
    let content = index.file_content(file).ok()?;
    let snippet = if max_lines > 3 && full_end.saturating_sub(start) + 1 > max_lines {
        let head_lines = (max_lines + 1) / 2;
        let tail_lines = max_lines / 2;
        let head_end = start
            .saturating_add(head_lines.saturating_sub(1))
            .min(full_end);
        let tail_start = full_end
            .saturating_sub(tail_lines.saturating_sub(1))
            .max(head_end.saturating_add(1));
        if tail_start <= full_end {
            format!(
                "{}\n...\n{}",
                source_line_slice(&content, start, head_end),
                source_line_slice(&content, tail_start, full_end)
            )
        } else {
            source_line_slice(&content, start, head_end)
        }
    } else {
        source_line_slice(&content, start, full_end)
    };
    let compact = compact_inline_text_with_tail(&snippet, max_chars);
    if compact.is_empty() {
        None
    } else {
        Some(compact)
    }
}

fn glob_target_graph_focus_score(index: &Codebase, target_path: &str) -> isize {
    let outgoing = index.deps_for(target_path).len();
    let incoming = index.reverse_deps_for(target_path).len();
    let degree = outgoing + incoming;
    let degree_score = match degree {
        0..=40 => 70,
        41..=120 => 45,
        121..=300 => 10,
        301..=700 => -60,
        _ => -120,
    };
    let incoming_penalty = match incoming {
        0..=80 => 0,
        81..=240 => -25,
        241..=600 => -70,
        _ => -120,
    };
    degree_score + incoming_penalty
}

fn glob_path_separation_score(source_path: &str, target_path: &str) -> isize {
    if source_path == target_path {
        return -40;
    }
    let source_parent = source_path.rsplit_once('/').map(|(parent, _)| parent);
    let target_parent = target_path.rsplit_once('/').map(|(parent, _)| parent);
    if source_parent.is_some() && source_parent == target_parent {
        return -45;
    }
    let source_parts = path_components(source_path);
    let target_parts = path_components(target_path);
    let shared = source_parts
        .iter()
        .zip(target_parts.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let shallow_shared = shared.min(4) as isize * 10;
    let deep_penalty = shared.saturating_sub(4) as isize * 8;
    shallow_shared - deep_penalty
}

fn select_diverse_glob_actionable_leads(
    leads: Vec<GlobActionableLead>,
    limit: usize,
) -> Vec<GlobActionableLead> {
    let mut selected = Vec::new();
    let mut used_sources = BTreeSet::<(String, usize, String)>::new();
    let mut used_targets = BTreeSet::<(String, usize, String)>::new();
    for lead in &leads {
        if selected.len() >= limit {
            return selected;
        }
        let source = (
            lead.source_path.clone(),
            lead.source_line,
            lead.source_name.clone(),
        );
        let target = (
            lead.target.path.clone(),
            lead.target.line_start,
            lead.target.name.clone(),
        );
        if used_sources.contains(&source) || used_targets.contains(&target) {
            continue;
        }
        used_sources.insert(source);
        used_targets.insert(target);
        selected.push(GlobActionableLead {
            source_path: lead.source_path.clone(),
            source_line: lead.source_line,
            source_name: lead.source_name.clone(),
            relation: lead.relation,
            source_text: lead.source_text.clone(),
            target: lead.target.clone(),
            score: lead.score,
        });
    }
    for lead in leads {
        if selected.len() >= limit {
            break;
        }
        let target = (
            lead.target.path.clone(),
            lead.target.line_start,
            lead.target.name.clone(),
        );
        if used_targets.insert(target) {
            selected.push(lead);
        }
    }
    selected
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
                let regex = get_bool(step, "regex");
                let compact = get_bool_default(step, "compact", true);
                let current = paths.clone();
                let candidate_cap = if current.is_some() {
                    max.saturating_mul(20).clamp(max, 10_000)
                } else {
                    max
                };
                let mut hits =
                    index.text_line_hits(&query, candidate_cap, regex, None, compact, true)?;
                if let Some(current_paths) = current.as_ref() {
                    hits.retain(|hit| current_paths.contains(&hit.path));
                    hits.truncate(max);
                }
                paths = Some(hits.iter().map(|hit| hit.path.clone()).collect());
                out = format_line_hits(&query, hits, compact);
            }
            "filter" => {
                let pattern = get_str(step, "path_glob")
                    .or_else(|| get_str(step, "glob"))
                    .or_else(|| get_str(step, "pattern"));
                let current = paths
                    .take()
                    .unwrap_or_else(|| index.files.keys().cloned().collect());
                if let Some(ext) = get_str(step, "ext") {
                    paths = Some(
                        current
                            .into_iter()
                            .filter(|path| path.ends_with(&ext))
                            .collect(),
                    );
                } else if let Some(pattern) = pattern {
                    let glob = build_globset(&pattern)?;
                    paths = Some(
                        current
                            .into_iter()
                            .filter(|path| glob.is_match(path))
                            .collect(),
                    );
                } else {
                    return Ok(
                        "error: filter needs 'path_glob', 'glob', 'pattern', or 'ext'".to_string(),
                    );
                }
            }
            "deps" => {
                let direction =
                    get_str(step, "direction").unwrap_or_else(|| "imported_by".to_string());
                let mut next = BTreeSet::new();
                let current = if let Some(path) = get_str(step, "path") {
                    [path].into_iter().collect::<BTreeSet<_>>()
                } else {
                    paths.take().unwrap_or_default()
                };
                for path in current {
                    let deps = if direction == "depends_on" {
                        index.deps_for(&path)
                    } else {
                        index.reverse_deps_for(&path)
                    };
                    next.extend(deps);
                }
                paths = Some(next);
            }
            "sort" => {
                let current = paths.take().unwrap_or_default();
                paths = Some(current.into_iter().collect());
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
            "read" => {
                let mut text = String::new();
                let line_start = get_usize(step, "line_start");
                let line_end = get_usize(step, "line_end");
                let compact = get_bool(step, "compact");
                let current = if let Some(path) = get_str(step, "path") {
                    [path].into_iter().collect::<BTreeSet<_>>()
                } else {
                    paths.clone().unwrap_or_default()
                };
                for path in current.iter().take(20) {
                    let args = json!({
                        "path": path,
                        "line_start": line_start,
                        "line_end": line_end,
                        "compact": compact
                    });
                    text.push_str(&handle_read(index, &args)?);
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
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

fn handle_callpath(index: &Codebase, args: &Value) -> Result<String> {
    let from = required_str(args, "from")?;
    let to = required_str(args, "to")?;
    let max_depth = get_usize(args, "max_hops")
        .or_else(|| get_usize(args, "max_depth"))
        .unwrap_or(12)
        .max(1);
    let source_path = get_str(args, "from_path")
        .or_else(|| get_str(args, "source_path"))
        .map(|path| normalize_rel_path(&path));
    let target_path = get_str(args, "to_path")
        .or_else(|| get_str(args, "target_path"))
        .map(|path| normalize_rel_path(&path));
    let source_line = get_usize(args, "from_line").or_else(|| get_usize(args, "source_line"));
    let target_line = get_usize(args, "to_line").or_else(|| get_usize(args, "target_line"));

    if let Some(result) = lazy_symbol_callpath(
        index,
        &from,
        &to,
        source_path.as_deref(),
        source_line,
        target_path.as_deref(),
        target_line,
        max_depth,
    )? {
        return serde_json::to_string_pretty(&result).map_err(Into::into);
    }

    let graph = index.graph();
    let mut result = serde_json::to_value(graph.shortest_path(&from, &to, max_depth))?;
    if let Some(object) = result.as_object_mut() {
        object.insert("mode".to_string(), json!("global_graph_fallback"));
        object.insert(
            "note".to_string(),
            json!("no exact symbol endpoint candidates were available for lazy symbol-reference callpath"),
        );
    }
    serde_json::to_string_pretty(&result).map_err(Into::into)
}

fn handle_graph_query(index: &Codebase, args: &Value) -> Result<String> {
    let statement = required_str(args, "query")?;
    let query = graph_query::parse(&statement)?;
    let mut provider = CodeGraphQueryProvider::new(index);
    let result = graph_query::execute(&mut provider, &query)?;
    serde_json::to_string_pretty(&json!({
        "ok": true,
        "language": "codedb-cypher-subset-v2",
        "query": statement,
        "result": result,
    }))
    .map_err(Into::into)
}

struct CodeGraphQueryProvider<'a> {
    index: &'a Codebase,
    graph: Option<Arc<crate::graph::CodeGraph>>,
    community_dependencies: Option<BTreeMap<(usize, usize), usize>>,
    community_nodes: Option<BTreeMap<usize, PropertyNode>>,
    file_nodes: HashMap<String, PropertyNode>,
    active_content_cache: HashMap<String, Arc<String>>,
    outgoing_cache: HashMap<String, Vec<CallpathEdge>>,
    incoming_cache: HashMap<String, Vec<CallpathEdge>>,
    control_cache: HashMap<String, ControlGraphFacts>,
}

impl<'a> CodeGraphQueryProvider<'a> {
    fn new(index: &'a Codebase) -> Self {
        Self {
            index,
            graph: None,
            community_dependencies: None,
            community_nodes: None,
            file_nodes: HashMap::new(),
            active_content_cache: HashMap::new(),
            outgoing_cache: HashMap::new(),
            incoming_cache: HashMap::new(),
            control_cache: HashMap::new(),
        }
    }

    fn graph(&mut self) -> Arc<crate::graph::CodeGraph> {
        if let Some(graph) = &self.graph {
            return graph.clone();
        }
        let graph = self.index.graph();
        self.graph = Some(graph.clone());
        graph
    }

    fn community_dependencies(&mut self) -> BTreeMap<(usize, usize), usize> {
        if let Some(dependencies) = &self.community_dependencies {
            return dependencies.clone();
        }
        let graph = self.graph();
        let mut dependencies = BTreeMap::<(usize, usize), usize>::new();
        for source in self.index.files.keys() {
            let Some(source_community) = graph_file_community(graph.as_ref(), source) else {
                continue;
            };
            for target in self.index.deps_for(source) {
                let Some(target_community) = graph_file_community(graph.as_ref(), &target) else {
                    continue;
                };
                if source_community != target_community {
                    *dependencies
                        .entry((source_community, target_community))
                        .or_default() += 1;
                }
            }
        }
        self.community_dependencies = Some(dependencies.clone());
        dependencies
    }

    fn file_node(&mut self, path: &str) -> Option<PropertyNode> {
        if let Some(node) = self.file_nodes.get(path) {
            return Some(node.clone());
        }
        let graph = self.graph();
        let file = self.index.file(path)?;
        let node = property_file_node(self.index, graph.as_ref(), file);
        self.file_nodes.insert(path.to_string(), node.clone());
        Some(node)
    }

    fn community_nodes(&mut self) -> BTreeMap<usize, PropertyNode> {
        if let Some(nodes) = &self.community_nodes {
            return nodes.clone();
        }
        let graph = self.graph();
        let nodes = property_community_nodes(graph.as_ref());
        self.community_nodes = Some(nodes.clone());
        nodes
    }

    fn community_node(&mut self, community: usize) -> Option<PropertyNode> {
        self.community_nodes().get(&community).cloned()
    }

    fn seed_value_nodes(&mut self, expression: &str) -> Result<Vec<PropertyNode>> {
        let Some(anchor) = raw_identifiers(expression).into_iter().next_back() else {
            return Ok(Vec::new());
        };
        let mut values = Vec::new();
        for hit in reference_candidates(self.index, &anchor)? {
            let Some(scope) = hit.scope else {
                continue;
            };
            let Some(file) = self.index.file(&hit.path) else {
                continue;
            };
            let Some(symbol) = file.symbols.iter().find(|symbol| {
                symbol.line_start == scope.start
                    && symbol.line_end == scope.end
                    && symbol.name == scope.name
                    && is_context_handoff_source_symbol(symbol)
            }) else {
                continue;
            };
            let owner = target_from_symbol(file, symbol);
            let key = format!("calls:{}", symbol_target_key(&owner));
            let mut resolved = BTreeSet::<(usize, String)>::new();
            for call in cached_callpath_edges(
                self.index,
                &key,
                &owner,
                None,
                false,
                &mut self.active_content_cache,
                &mut self.outgoing_cache,
            )? {
                if call.line != Some(hit.line) {
                    continue;
                }
                let callsite = property_resolved_callsite_node(self.index, &owner, &call);
                if let (Some(line), Some(name)) = (
                    graph_integer_property(&callsite, "line"),
                    graph_string_property(&callsite, "name"),
                ) {
                    resolved.insert((line as usize, name.to_string()));
                }
                values.extend(
                    property_callsite_values(&callsite)
                        .into_iter()
                        .filter(|value| {
                            graph_string_property(value, "expression") == Some(expression)
                        }),
                );
            }
            for callsite in unresolved_qualified_callsite_nodes_on_line(
                self.index, &owner, hit.line, &hit.text, &resolved,
            ) {
                values.extend(
                    property_callsite_values(&callsite)
                        .into_iter()
                        .filter(|value| {
                            graph_string_property(value, "expression") == Some(expression)
                        }),
                );
            }
        }
        values.sort_by(|left, right| left.id.cmp(&right.id));
        values.dedup_by(|left, right| left.id == right.id);
        Ok(values)
    }

    fn symbol_from_node(&self, node: &PropertyNode) -> Option<SymbolTarget> {
        let path = graph_string_property(node, "path")?;
        let name = graph_string_property(node, "name")?;
        let line_start = graph_integer_property(node, "line_start")? as usize;
        self.index.file(path)?;
        Some(SymbolTarget {
            name: name.to_string(),
            kind: graph_string_property(node, "kind")
                .unwrap_or("symbol")
                .to_string(),
            path: path.to_string(),
            line_start,
            detail: graph_string_property(node, "detail")
                .unwrap_or_default()
                .to_string(),
        })
    }

    fn shared_state_from_node(&self, node: &PropertyNode) -> Option<SymbolTarget> {
        if !node.labels.iter().any(|label| label == "SharedState") {
            return None;
        }
        self.symbol_from_node(node)
    }

    fn expand_symbol(
        &mut self,
        node: &PropertyNode,
        relationship: &GraphRelationshipPattern,
    ) -> Result<Vec<(PropertyEdge, PropertyNode)>> {
        let Some(source) = self.symbol_from_node(node) else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        let outgoing = matches!(
            relationship.direction,
            GraphDirection::Outgoing | GraphDirection::Either
        );
        let incoming = matches!(
            relationship.direction,
            GraphDirection::Incoming | GraphDirection::Either
        );

        if graph_relation_requested(relationship, "CALLS") {
            if outgoing {
                let key = format!("calls:{}", symbol_target_key(&source));
                for edge in cached_callpath_edges(
                    self.index,
                    &key,
                    &source,
                    None,
                    false,
                    &mut self.active_content_cache,
                    &mut self.outgoing_cache,
                )? {
                    if !matches!(edge.relation.as_str(), "qualified_call" | "direct_call") {
                        continue;
                    }
                    let target_node = property_symbol_node(self.index, &edge.target);
                    result.push((
                        property_call_edge(self.index, &source, &edge.target, &edge),
                        target_node,
                    ));
                }
            }
            if incoming {
                let key = format!("calls:{}", symbol_target_key(&source));
                for edge in cached_callpath_incoming_edges(
                    self.index,
                    &key,
                    &source,
                    None,
                    false,
                    &mut self.active_content_cache,
                    &mut self.incoming_cache,
                    &mut self.outgoing_cache,
                )? {
                    if !matches!(edge.relation.as_str(), "qualified_call" | "direct_call") {
                        continue;
                    }
                    let caller = edge.target.clone();
                    let caller_node = property_symbol_node(self.index, &caller);
                    result.push((
                        property_call_edge(self.index, &caller, &source, &edge),
                        caller_node,
                    ));
                }
            }
        }

        if graph_relation_requested(relationship, "REFERENCES") {
            if outgoing {
                let key = format!("refs:{}", symbol_target_key(&source));
                for edge in cached_callpath_edges(
                    self.index,
                    &key,
                    &source,
                    None,
                    true,
                    &mut self.active_content_cache,
                    &mut self.outgoing_cache,
                )? {
                    let target_node = property_symbol_node(self.index, &edge.target);
                    result.push((
                        property_reference_edge(self.index, &source, &edge.target, &edge),
                        target_node,
                    ));
                }
            }
            if incoming {
                for edge in incoming_reference_edges(self.index, &source)? {
                    let caller = edge.target.clone();
                    result.push((
                        property_reference_edge(self.index, &caller, &source, &edge),
                        property_symbol_node(self.index, &caller),
                    ));
                }
            }
        }

        if graph_relation_requested(relationship, "DISPATCHES_TO") {
            if outgoing {
                for target in symbol_target_dispatch_candidates(self.index, &source) {
                    result.push((
                        property_edge(&source, &target, "DISPATCHES_TO", BTreeMap::new()),
                        property_symbol_node(self.index, &target),
                    ));
                }
            }
            if incoming {
                for (file, symbol) in self.index.symbols_named(&source.name) {
                    if !matches!(symbol.kind.as_str(), "method" | "function" | "property") {
                        continue;
                    }
                    let interface = target_from_symbol(file, symbol);
                    if symbol_target_dispatch_candidates(self.index, &interface)
                        .into_iter()
                        .any(|candidate| {
                            symbol_target_key(&candidate) == symbol_target_key(&source)
                        })
                    {
                        result.push((
                            property_edge(&interface, &source, "DISPATCHES_TO", BTreeMap::new()),
                            property_symbol_node(self.index, &interface),
                        ));
                    }
                }
            }
        }

        if outgoing
            && (graph_relation_requested(relationship, "READS")
                || graph_relation_requested(relationship, "WRITES"))
        {
            for access in symbol_shared_state_accesses(self.index, &source)? {
                if !graph_relation_requested(relationship, access.relation) {
                    continue;
                }
                result.push((
                    property_state_access_edge(&source, &access),
                    property_shared_state_node(self.index, &access.state),
                ));
            }
        }

        if incoming && graph_relation_requested(relationship, "CONTAINS") {
            if let Some(file_node) = self.file_node(&source.path) {
                result.push((
                    property_file_contains_edge(&source.path, &source),
                    file_node,
                ));
            }
        }

        if outgoing && graph_relation_requested(relationship, "HAS_PARAMETER") {
            let facts = self.control_facts(&source)?;
            for parameter in &facts.parameters {
                result.push((
                    virtual_property_edge(
                        &symbol_target_key(&source),
                        &parameter.node.id,
                        "HAS_PARAMETER",
                        BTreeMap::new(),
                    ),
                    parameter.node.clone(),
                ));
            }
        }

        if outgoing && graph_relation_requested(relationship, "HAS_CALLSITE") {
            let key = format!("calls:{}", symbol_target_key(&source));
            let mut resolved = BTreeSet::<(usize, String)>::new();
            for call in cached_callpath_edges(
                self.index,
                &key,
                &source,
                None,
                false,
                &mut self.active_content_cache,
                &mut self.outgoing_cache,
            )? {
                if !matches!(call.relation.as_str(), "qualified_call" | "direct_call") {
                    continue;
                }
                let callsite = property_resolved_callsite_node(self.index, &source, &call);
                if let (Some(line), Some(name)) = (
                    graph_integer_property(&callsite, "line"),
                    graph_string_property(&callsite, "name"),
                ) {
                    resolved.insert((line as usize, name.to_string()));
                }
                result.push((
                    virtual_property_edge(
                        &symbol_target_key(&source),
                        &callsite.id,
                        "HAS_CALLSITE",
                        BTreeMap::new(),
                    ),
                    callsite,
                ));
            }
            for callsite in unresolved_qualified_callsite_nodes(self.index, &source, &resolved)? {
                result.push((
                    virtual_property_edge(
                        &symbol_target_key(&source),
                        &callsite.id,
                        "HAS_CALLSITE",
                        BTreeMap::new(),
                    ),
                    callsite,
                ));
            }
        }

        graph_sort_and_dedup(&mut result);
        Ok(result)
    }

    fn control_facts(&mut self, owner: &SymbolTarget) -> Result<ControlGraphFacts> {
        let key = symbol_target_key(owner);
        if let Some(facts) = self.control_cache.get(&key) {
            return Ok(facts.clone());
        }
        let facts = build_control_graph_facts(self.index, owner)?;
        self.control_cache.insert(key, facts.clone());
        Ok(facts)
    }

    fn expand_control_node(
        &mut self,
        node: &PropertyNode,
        relationship: &GraphRelationshipPattern,
    ) -> Result<Vec<(PropertyEdge, PropertyNode)>> {
        let Some(owner) = graph_owner_target(node) else {
            return Ok(Vec::new());
        };
        let facts = self.control_facts(&owner)?;
        let mut result = Vec::new();
        let outgoing = matches!(
            relationship.direction,
            GraphDirection::Outgoing | GraphDirection::Either
        );
        let incoming = matches!(
            relationship.direction,
            GraphDirection::Incoming | GraphDirection::Either
        );

        if node.labels.iter().any(|label| label == "Parameter") {
            if outgoing && graph_relation_requested(relationship, "USED_IN") {
                for condition in &facts.conditions {
                    for use_edge in &condition.parameter_uses {
                        if use_edge.parameter_id == node.id {
                            result.push((
                                virtual_property_edge(
                                    &node.id,
                                    &condition.node.id,
                                    "USED_IN",
                                    use_edge.properties.clone(),
                                ),
                                condition.node.clone(),
                            ));
                        }
                    }
                }
            }
            if incoming && graph_relation_requested(relationship, "HAS_PARAMETER") {
                result.push((
                    virtual_property_edge(
                        &symbol_target_key(&owner),
                        &node.id,
                        "HAS_PARAMETER",
                        BTreeMap::new(),
                    ),
                    property_symbol_node(self.index, &owner),
                ));
            }
        } else if node.labels.iter().any(|label| label == "Condition") {
            if outgoing {
                if let Some(condition) = facts
                    .conditions
                    .iter()
                    .find(|condition| condition.node.id == node.id)
                {
                    for branch in &condition.branches {
                        if graph_relation_requested(relationship, branch.relation) {
                            result.push((
                                virtual_property_edge(
                                    &node.id,
                                    &branch.action.id,
                                    branch.relation,
                                    BTreeMap::new(),
                                ),
                                branch.action.clone(),
                            ));
                        }
                    }
                }
            }
            if incoming && graph_relation_requested(relationship, "USED_IN") {
                if let Some(condition) = facts
                    .conditions
                    .iter()
                    .find(|condition| condition.node.id == node.id)
                {
                    for use_edge in &condition.parameter_uses {
                        if let Some(parameter) = facts
                            .parameters
                            .iter()
                            .find(|parameter| parameter.node.id == use_edge.parameter_id)
                        {
                            result.push((
                                virtual_property_edge(
                                    &parameter.node.id,
                                    &node.id,
                                    "USED_IN",
                                    use_edge.properties.clone(),
                                ),
                                parameter.node.clone(),
                            ));
                        }
                    }
                }
            }
        } else if node.labels.iter().any(|label| label == "ControlAction") {
            if outgoing {
                for condition in &facts.conditions {
                    for branch in &condition.branches {
                        if branch.action.id != node.id {
                            continue;
                        }
                        for effect in &branch.effects {
                            if graph_relation_requested(relationship, effect.relation) {
                                result.push((
                                    virtual_property_edge(
                                        &node.id,
                                        &effect.callsite.id,
                                        effect.relation,
                                        BTreeMap::new(),
                                    ),
                                    effect.callsite.clone(),
                                ));
                            }
                        }
                    }
                }
            }
            if incoming {
                for condition in &facts.conditions {
                    for branch in &condition.branches {
                        if branch.action.id == node.id
                            && graph_relation_requested(relationship, branch.relation)
                        {
                            result.push((
                                virtual_property_edge(
                                    &condition.node.id,
                                    &node.id,
                                    branch.relation,
                                    BTreeMap::new(),
                                ),
                                condition.node.clone(),
                            ));
                        }
                    }
                }
            }
        } else if node.labels.iter().any(|label| label == "CallSite") {
            if outgoing && graph_relation_requested(relationship, "TARGET") {
                if let Some(target) = graph_callsite_target(node) {
                    result.push((
                        virtual_property_edge(
                            &node.id,
                            &symbol_target_key(&target),
                            "TARGET",
                            BTreeMap::new(),
                        ),
                        property_symbol_node(self.index, &target),
                    ));
                }
            }
            if outgoing && graph_relation_requested(relationship, "ARGUMENT") {
                for value in property_callsite_values(node) {
                    let index = graph_integer_property(&value, "index").unwrap_or_default();
                    result.push((
                        virtual_property_edge(
                            &node.id,
                            &value.id,
                            "ARGUMENT",
                            BTreeMap::from([("index".to_string(), GraphScalar::Integer(index))]),
                        ),
                        value,
                    ));
                }
            }
            if incoming {
                if graph_relation_requested(relationship, "HAS_CALLSITE") {
                    result.push((
                        virtual_property_edge(
                            &symbol_target_key(&owner),
                            &node.id,
                            "HAS_CALLSITE",
                            BTreeMap::new(),
                        ),
                        property_symbol_node(self.index, &owner),
                    ));
                }
                for condition in &facts.conditions {
                    for branch in &condition.branches {
                        for effect in &branch.effects {
                            if effect.callsite.id == node.id
                                && graph_relation_requested(relationship, effect.relation)
                            {
                                result.push((
                                    virtual_property_edge(
                                        &branch.action.id,
                                        &node.id,
                                        effect.relation,
                                        BTreeMap::new(),
                                    ),
                                    branch.action.clone(),
                                ));
                            }
                        }
                    }
                }
            }
        } else if node.labels.iter().any(|label| label == "Value") {
            if outgoing && graph_relation_requested(relationship, "BINDS_TO") {
                if let Some(target) = graph_value_target(node) {
                    let facts = self.control_facts(&target)?;
                    let index = graph_integer_property(node, "index").unwrap_or_default();
                    if let Some(parameter) = facts.parameters.iter().find(|parameter| {
                        graph_integer_property(&parameter.node, "index") == Some(index)
                    }) {
                        result.push((
                            virtual_property_edge(
                                &node.id,
                                &parameter.node.id,
                                "BINDS_TO",
                                BTreeMap::from([(
                                    "index".to_string(),
                                    GraphScalar::Integer(index),
                                )]),
                            ),
                            parameter.node.clone(),
                        ));
                    }
                }
            }
            if incoming && graph_relation_requested(relationship, "ARGUMENT") {
                if let Some(callsite) = property_value_callsite(node) {
                    let index = graph_integer_property(node, "index").unwrap_or_default();
                    result.push((
                        virtual_property_edge(
                            &callsite.id,
                            &node.id,
                            "ARGUMENT",
                            BTreeMap::from([("index".to_string(), GraphScalar::Integer(index))]),
                        ),
                        callsite,
                    ));
                }
            }
        }
        graph_sort_and_dedup(&mut result);
        Ok(result)
    }

    fn expand_shared_state(
        &mut self,
        node: &PropertyNode,
        relationship: &GraphRelationshipPattern,
    ) -> Result<Vec<(PropertyEdge, PropertyNode)>> {
        let Some(state) = self.shared_state_from_node(node) else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        if matches!(
            relationship.direction,
            GraphDirection::Incoming | GraphDirection::Either
        ) {
            for access in incoming_shared_state_accesses(self.index, &state)? {
                if !graph_relation_requested(relationship, access.relation) {
                    continue;
                }
                result.push((
                    property_state_access_edge(&access.owner, &access),
                    property_symbol_node(self.index, &access.owner),
                ));
            }
        }
        graph_sort_and_dedup(&mut result);
        Ok(result)
    }

    fn expand_file(
        &mut self,
        node: &PropertyNode,
        relationship: &GraphRelationshipPattern,
    ) -> Result<Vec<(PropertyEdge, PropertyNode)>> {
        let Some(path) = graph_string_property(node, "path") else {
            return Ok(Vec::new());
        };
        let graph = self.graph();
        let Some(file) = self.index.file(path) else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        let outgoing = matches!(
            relationship.direction,
            GraphDirection::Outgoing | GraphDirection::Either
        );
        let incoming = matches!(
            relationship.direction,
            GraphDirection::Incoming | GraphDirection::Either
        );
        if outgoing && graph_relation_requested(relationship, "CONTAINS") {
            for symbol in &file.symbols {
                let target = target_from_symbol(file, symbol);
                result.push((
                    property_file_contains_edge(path, &target),
                    property_symbol_node(self.index, &target),
                ));
            }
        }
        if incoming
            && graph_relation_requested(relationship, "CONTAINS")
            && let Some(community) = graph_file_community(graph.as_ref(), path)
        {
            if let Some(community_node) = self.community_node(community) {
                result.push((
                    virtual_property_edge(
                        &community_node.id,
                        &format!("file:{path}"),
                        "CONTAINS",
                        BTreeMap::new(),
                    ),
                    community_node,
                ));
            }
        }
        if graph_relation_requested(relationship, "DEPENDS_ON") {
            if outgoing {
                for target_path in self.index.deps_for(path) {
                    if let Some(target_file) = self.file_node(&target_path) {
                        result.push((
                            property_file_dependency_edge(path, &target_path),
                            target_file,
                        ));
                    }
                }
            }
            if incoming {
                for source_path in self.index.reverse_deps_for(path) {
                    if let Some(source_file) = self.file_node(&source_path) {
                        result.push((
                            property_file_dependency_edge(&source_path, path),
                            source_file,
                        ));
                    }
                }
            }
        }
        graph_sort_and_dedup(&mut result);
        Ok(result)
    }

    fn expand_community(
        &mut self,
        node: &PropertyNode,
        relationship: &GraphRelationshipPattern,
    ) -> Result<Vec<(PropertyEdge, PropertyNode)>> {
        let Some(community) = graph_integer_property(node, "id").map(|value| value as usize) else {
            return Ok(Vec::new());
        };
        let graph = self.graph();
        let outgoing = matches!(
            relationship.direction,
            GraphDirection::Outgoing | GraphDirection::Either
        );
        let incoming = matches!(
            relationship.direction,
            GraphDirection::Incoming | GraphDirection::Either
        );
        let mut result = Vec::new();

        if outgoing && graph_relation_requested(relationship, "CONTAINS") {
            for (id, path) in graph.file_graph.paths.iter().enumerate() {
                if graph.file_graph.community(id) != Some(community) {
                    continue;
                }
                let Some(file_node) = self.file_node(path) else {
                    continue;
                };
                result.push((
                    virtual_property_edge(&node.id, &file_node.id, "CONTAINS", BTreeMap::new()),
                    file_node,
                ));
            }
        }

        if graph_relation_requested(relationship, "DEPENDS_ON") {
            for ((source, target), count) in self.community_dependencies() {
                if outgoing && source == community {
                    if let Some(target_node) = self.community_node(target) {
                        result.push((
                            virtual_property_edge(
                                &node.id,
                                &target_node.id,
                                "DEPENDS_ON",
                                BTreeMap::from([(
                                    "file_edges".to_string(),
                                    GraphScalar::Integer(count as i64),
                                )]),
                            ),
                            target_node,
                        ));
                    }
                }
                if incoming && target == community {
                    if let Some(source_node) = self.community_node(source) {
                        result.push((
                            virtual_property_edge(
                                &source_node.id,
                                &node.id,
                                "DEPENDS_ON",
                                BTreeMap::from([(
                                    "file_edges".to_string(),
                                    GraphScalar::Integer(count as i64),
                                )]),
                            ),
                            source_node,
                        ));
                    }
                }
            }
        }

        graph_sort_and_dedup(&mut result);
        Ok(result)
    }
}

fn incoming_reference_edges(index: &Codebase, target: &SymbolTarget) -> Result<Vec<CallpathEdge>> {
    let Some(target_file) = index.file(&target.path) else {
        return Ok(Vec::new());
    };
    let Some(target_symbol) = symbol_for_target(target_file, target) else {
        return Ok(Vec::new());
    };
    let unique_name = index.symbols_named(&target.name).len() == 1;
    let mut edges = Vec::new();
    for hit in reference_candidates(index, &target.name)? {
        let Some(scope) = hit.scope else {
            continue;
        };
        if hit.path == target.path && scope.start == target.line_start && scope.name == target.name
        {
            continue;
        }
        let Some(file) = index.file(&hit.path) else {
            continue;
        };
        let Some(symbol) = file.symbols.iter().find(|symbol| {
            symbol.line_start == scope.start
                && symbol.line_end == scope.end
                && symbol.name == scope.name
                && is_context_handoff_source_symbol(symbol)
        }) else {
            continue;
        };
        let qualified = qualified_member_tokens(&hit.text).into_iter().any(|token| {
            token.rsplit('.').next() == Some(target.name.as_str())
                && qualified_call_qualifier_matches(index, &token, target_file, target_symbol) > 0
        });
        if !qualified && !unique_name && file.path != target.path {
            continue;
        }
        edges.push(CallpathEdge {
            target: target_from_symbol(file, symbol),
            relation: if qualified {
                "member_reference".to_string()
            } else {
                "symbol_reference".to_string()
            },
            line: Some(hit.line),
            text: Some(hit.text),
        });
    }
    edges.sort_by(|left, right| {
        symbol_target_key(&left.target).cmp(&symbol_target_key(&right.target))
    });
    edges.dedup_by(|left, right| {
        symbol_target_key(&left.target) == symbol_target_key(&right.target)
    });
    Ok(edges)
}

impl QueryProvider for CodeGraphQueryProvider<'_> {
    fn seed_nodes(&mut self, pattern: &GraphNodePattern) -> Result<Vec<PropertyNode>> {
        let label = pattern.label.as_deref();
        let mut nodes = Vec::new();
        match label {
            Some("File" | "EntryFile" | "BoundaryFile" | "SinkFile") => {
                let paths = self.index.files.keys().cloned().collect::<Vec<_>>();
                nodes.extend(paths.into_iter().filter_map(|path| self.file_node(&path)));
            }
            Some("Community") => {
                nodes.extend(self.community_nodes().into_values());
            }
            Some("SharedState") => {
                for file in self.index.files.values() {
                    for symbol in &file.symbols {
                        if is_shared_state_symbol(symbol) {
                            nodes.push(property_shared_state_node(
                                self.index,
                                &target_from_symbol(file, symbol),
                            ));
                        }
                    }
                }
            }
            Some("Value") => {
                let Some(GraphScalar::String(expression)) = pattern.properties.get("expression")
                else {
                    return Err(anyhow!(
                        "Value nodes require an exact expression predicate or an anchored CallSite"
                    ));
                };
                nodes.extend(self.seed_value_nodes(expression)?);
            }
            Some("Parameter" | "Condition" | "ControlAction" | "CallSite") => {
                let owner_name =
                    pattern
                        .properties
                        .get("owner_name")
                        .and_then(|value| match value {
                            GraphScalar::String(value) => Some(value.as_str()),
                            _ => None,
                        });
                if owner_name.is_none() {
                    return Err(anyhow!(
                        "{label:?} nodes must be reached from an anchored Symbol or constrained with owner_name"
                    ));
                }
                let owners = if let Some(owner_name) = owner_name {
                    self.index
                        .symbols_named(owner_name)
                        .into_iter()
                        .filter(|(_, symbol)| is_context_handoff_source_symbol(symbol))
                        .map(|(file, symbol)| target_from_symbol(file, symbol))
                        .collect::<Vec<_>>()
                } else {
                    self.index
                        .files
                        .values()
                        .flat_map(|file| {
                            file.symbols
                                .iter()
                                .filter(|symbol| is_context_handoff_source_symbol(symbol))
                                .map(|symbol| target_from_symbol(file, symbol))
                        })
                        .collect::<Vec<_>>()
                };
                for owner in owners {
                    let facts = self.control_facts(&owner)?;
                    match label {
                        Some("Parameter") => nodes
                            .extend(facts.parameters.into_iter().map(|parameter| parameter.node)),
                        Some("Condition") => nodes
                            .extend(facts.conditions.into_iter().map(|condition| condition.node)),
                        Some("ControlAction") => {
                            nodes.extend(facts.conditions.into_iter().flat_map(|condition| {
                                condition.branches.into_iter().map(|branch| branch.action)
                            }))
                        }
                        Some("CallSite") => {
                            nodes.extend(facts.conditions.into_iter().flat_map(|condition| {
                                condition.branches.into_iter().flat_map(|branch| {
                                    branch.effects.into_iter().map(|effect| effect.callsite)
                                })
                            }))
                        }
                        _ => {}
                    }
                }
            }
            Some("Symbol") | None => {
                if let Some(GraphScalar::String(name)) = pattern.properties.get("name") {
                    nodes.extend(self.index.symbols_named(name).into_iter().map(
                        |(file, symbol)| {
                            property_symbol_node(self.index, &target_from_symbol(file, symbol))
                        },
                    ));
                } else {
                    for file in self.index.files.values() {
                        nodes.extend(file.symbols.iter().map(|symbol| {
                            property_symbol_node(self.index, &target_from_symbol(file, symbol))
                        }));
                    }
                }
            }
            Some(kind) => {
                for file in self.index.files.values() {
                    for symbol in &file.symbols {
                        let node =
                            property_symbol_node(self.index, &target_from_symbol(file, symbol));
                        if node.labels.iter().any(|candidate| candidate == kind) {
                            nodes.push(node);
                        }
                    }
                }
            }
        }
        nodes.retain(|node| graph_node_matches_pattern(node, pattern));
        nodes.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(nodes)
    }

    fn expand(
        &mut self,
        node: &PropertyNode,
        relationship: &GraphRelationshipPattern,
    ) -> Result<Vec<(PropertyEdge, PropertyNode)>> {
        if node.labels.iter().any(|label| label == "SharedState") {
            self.expand_shared_state(node, relationship)
        } else if node.labels.iter().any(|label| label == "Community") {
            self.expand_community(node, relationship)
        } else if node.labels.iter().any(|label| label == "File") {
            self.expand_file(node, relationship)
        } else if node.labels.iter().any(|label| {
            matches!(
                label.as_str(),
                "Parameter" | "Condition" | "ControlAction" | "CallSite" | "Value"
            )
        }) {
            self.expand_control_node(node, relationship)
        } else if node.labels.iter().any(|label| label == "Symbol") {
            self.expand_symbol(node, relationship)
        } else {
            Ok(Vec::new())
        }
    }
}

#[derive(Clone, Default)]
struct ControlGraphFacts {
    parameters: Vec<ControlParameterFact>,
    conditions: Vec<ControlConditionFact>,
}

#[derive(Clone)]
struct ControlParameterFact {
    node: PropertyNode,
}

#[derive(Clone)]
struct ControlConditionFact {
    node: PropertyNode,
    parameter_uses: Vec<ControlParameterUse>,
    branches: Vec<ControlBranchFact>,
}

#[derive(Clone)]
struct ControlParameterUse {
    parameter_id: String,
    properties: BTreeMap<String, GraphScalar>,
}

#[derive(Clone)]
struct ControlBranchFact {
    relation: &'static str,
    action: PropertyNode,
    effects: Vec<ControlEffectFact>,
}

#[derive(Clone)]
struct ControlEffectFact {
    relation: &'static str,
    callsite: PropertyNode,
}

fn build_control_graph_facts(index: &Codebase, owner: &SymbolTarget) -> Result<ControlGraphFacts> {
    let Some(file) = index.file(&owner.path) else {
        return Ok(ControlGraphFacts::default());
    };
    let Some(symbol) = symbol_for_target(file, owner) else {
        return Ok(ControlGraphFacts::default());
    };
    let Some(parameter_declarations) = signature_parameters(&symbol.detail) else {
        return Ok(ControlGraphFacts::default());
    };
    let parameters = parameter_declarations
        .iter()
        .enumerate()
        .filter_map(|(index, declaration)| {
            let declaration_without_default = declaration.split('=').next().unwrap_or(declaration);
            let name = raw_identifiers(declaration_without_default)
                .into_iter()
                .next_back()?;
            Some((name, index, declaration.clone()))
        })
        .collect::<Vec<_>>();
    if parameters.is_empty() {
        return Ok(ControlGraphFacts::default());
    }

    let parameter_facts = parameters
        .iter()
        .map(|(name, index, declaration)| ControlParameterFact {
            node: control_property_node(
                "Parameter",
                format!("parameter:{}:{index}:{name}", symbol_target_key(owner)),
                owner,
                BTreeMap::from([
                    ("name".to_string(), GraphScalar::String(name.clone())),
                    ("index".to_string(), GraphScalar::Integer(*index as i64)),
                    (
                        "declaration".to_string(),
                        GraphScalar::String(declaration.clone()),
                    ),
                ]),
            ),
        })
        .collect::<Vec<_>>();
    let parameter_ids = parameter_facts
        .iter()
        .map(|parameter| {
            (
                graph_string_property(&parameter.node, "name")
                    .unwrap_or_default()
                    .to_string(),
                parameter.node.id.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();

    let content = index.file_content(file)?;
    let active = mask_comments(file.language.as_str(), &content);
    let body = source_line_slice(
        &active,
        symbol.line_start,
        symbol.line_end.max(symbol.line_start),
    );
    let lines = body.lines().collect::<Vec<_>>();
    let mut depth = 0isize;
    let mut depths = Vec::with_capacity(lines.len());
    for line in &lines {
        let code = strip_strings_and_line_comment(line);
        depths.push(depth);
        depth += code.chars().filter(|ch| *ch == '{').count() as isize;
        depth -= code.chars().filter(|ch| *ch == '}').count() as isize;
    }

    let mut tracked = parameters
        .iter()
        .map(|(name, _, _)| (name.clone(), name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut conditions = Vec::new();
    for (offset, line) in lines.iter().enumerate() {
        let code = strip_strings_and_line_comment(line);
        let roots = tracked
            .iter()
            .filter(|(name, _)| line_contains_identifier_token(&code, name))
            .map(|(_, root)| root.clone())
            .collect::<BTreeSet<_>>();
        if !roots.is_empty()
            && let Some(alias) = assignment_target_identifier(&code)
            && !tracked.contains_key(&alias)
            && let Some(root) = roots.iter().next()
        {
            tracked.insert(alias, root.clone());
        }

        let trimmed = code.trim_start();
        let is_condition = trimmed.starts_with("if")
            || trimmed.starts_with("else if")
            || trimmed.starts_with("while");
        if !is_condition || roots.is_empty() {
            continue;
        }
        let line_number = symbol.line_start + offset;
        let aliases = tracked
            .iter()
            .filter(|(name, root)| {
                roots.contains(*root) && line_contains_identifier_token(&code, name)
            })
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        let negated = aliases
            .iter()
            .any(|alias| identifier_condition_is_negated(&code, alias));
        let condition_node = control_property_node(
            "Condition",
            format!("condition:{}:{line_number}", symbol_target_key(owner)),
            owner,
            BTreeMap::from([
                ("line".to_string(), GraphScalar::Integer(line_number as i64)),
                (
                    "text".to_string(),
                    GraphScalar::String(line.trim().to_string()),
                ),
                ("negated".to_string(), GraphScalar::Boolean(negated)),
                (
                    "aliases".to_string(),
                    GraphScalar::String(aliases.iter().cloned().collect::<Vec<_>>().join(",")),
                ),
            ]),
        );
        let parameter_uses = roots
            .iter()
            .filter_map(|root| {
                let parameter_id = parameter_ids.get(root)?.clone();
                let via = aliases
                    .iter()
                    .filter(|alias| tracked.get(*alias) == Some(root))
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(",");
                Some(ControlParameterUse {
                    parameter_id,
                    properties: BTreeMap::from([("via".to_string(), GraphScalar::String(via))]),
                })
            })
            .collect::<Vec<_>>();

        let action =
            lines
                .iter()
                .enumerate()
                .skip(offset)
                .take(8)
                .find_map(|(action_offset, candidate)| {
                    control_action_kind(&strip_strings_and_line_comment(candidate))
                        .map(|kind| (action_offset, kind, candidate.trim().to_string()))
                });
        let mut branches = Vec::new();
        if let Some((action_offset, action_kind, action_text)) = action {
            let action_line = symbol.line_start + action_offset;
            let true_action = control_property_node(
                "ControlAction",
                format!(
                    "action:{}:{action_line}:{action_kind}",
                    symbol_target_key(owner)
                ),
                owner,
                BTreeMap::from([
                    ("line".to_string(), GraphScalar::Integer(action_line as i64)),
                    (
                        "kind".to_string(),
                        GraphScalar::String(action_kind.to_string()),
                    ),
                    ("text".to_string(), GraphScalar::String(action_text)),
                    (
                        "branch".to_string(),
                        GraphScalar::String("true".to_string()),
                    ),
                ]),
            );
            let false_action = control_property_node(
                "ControlAction",
                format!(
                    "action:{}:{line_number}:fallthrough",
                    symbol_target_key(owner)
                ),
                owner,
                BTreeMap::from([
                    ("line".to_string(), GraphScalar::Integer(line_number as i64)),
                    (
                        "kind".to_string(),
                        GraphScalar::String("fallthrough".to_string()),
                    ),
                    (
                        "text".to_string(),
                        GraphScalar::String(
                            "condition false; continue after guarded action".to_string(),
                        ),
                    ),
                    (
                        "branch".to_string(),
                        GraphScalar::String("false".to_string()),
                    ),
                ]),
            );
            let calls =
                subsequent_control_calls(owner, symbol, &lines, &depths, offset, action_offset);
            branches.push(ControlBranchFact {
                relation: "TRUE",
                action: true_action,
                effects: calls
                    .iter()
                    .cloned()
                    .map(|callsite| ControlEffectFact {
                        relation: "PREVENTS",
                        callsite,
                    })
                    .collect(),
            });
            branches.push(ControlBranchFact {
                relation: "FALSE",
                action: false_action,
                effects: calls
                    .into_iter()
                    .map(|callsite| ControlEffectFact {
                        relation: "REACHES",
                        callsite,
                    })
                    .collect(),
            });
        }
        conditions.push(ControlConditionFact {
            node: condition_node,
            parameter_uses,
            branches,
        });
    }
    Ok(ControlGraphFacts {
        parameters: parameter_facts,
        conditions,
    })
}

fn control_action_kind(line: &str) -> Option<&'static str> {
    let trimmed = line.trim();
    if trimmed == "continue;" || trimmed.contains(" continue;") {
        Some("continue")
    } else if trimmed == "break;" || trimmed.contains(" break;") {
        Some("break")
    } else if trimmed.starts_with("return ") || trimmed == "return;" {
        Some("return")
    } else if trimmed.starts_with("throw ") {
        Some("throw")
    } else if trimmed.starts_with("yield ") {
        Some("yield")
    } else {
        None
    }
}

fn subsequent_control_calls(
    owner: &SymbolTarget,
    symbol: &Symbol,
    lines: &[&str],
    depths: &[isize],
    condition_offset: usize,
    action_offset: usize,
) -> Vec<PropertyNode> {
    let condition_depth = depths.get(condition_offset).copied().unwrap_or_default();
    let mut calls = BTreeMap::<String, PropertyNode>::new();
    for (offset, line) in lines.iter().enumerate().skip(action_offset + 1) {
        if depths.get(offset).copied().unwrap_or_default() < condition_depth {
            break;
        }
        let code = strip_strings_and_line_comment(line);
        for token in qualified_call_tokens(&code) {
            let name = token.rsplit('.').next().unwrap_or(&token).to_string();
            let receiver = token
                .rsplit_once('.')
                .map(|(receiver, _)| receiver.to_string())
                .unwrap_or_default();
            let line_number = symbol.line_start + offset;
            let id = format!(
                "callsite:{}:{line_number}:{token}",
                symbol_target_key(owner)
            );
            calls.entry(id.clone()).or_insert_with(|| {
                control_property_node(
                    "CallSite",
                    id,
                    owner,
                    BTreeMap::from([
                        ("name".to_string(), GraphScalar::String(name)),
                        ("receiver".to_string(), GraphScalar::String(receiver)),
                        ("line".to_string(), GraphScalar::Integer(line_number as i64)),
                        (
                            "text".to_string(),
                            GraphScalar::String(line.trim().to_string()),
                        ),
                    ]),
                )
            });
        }
    }
    calls.into_values().collect()
}

fn control_property_node(
    label: &str,
    id: String,
    owner: &SymbolTarget,
    mut properties: BTreeMap<String, GraphScalar>,
) -> PropertyNode {
    properties.insert(
        "owner_name".to_string(),
        GraphScalar::String(owner.name.clone()),
    );
    properties.insert(
        "owner_path".to_string(),
        GraphScalar::String(owner.path.clone()),
    );
    properties.insert(
        "owner_line".to_string(),
        GraphScalar::Integer(owner.line_start as i64),
    );
    properties.insert(
        "owner_kind".to_string(),
        GraphScalar::String(owner.kind.clone()),
    );
    properties.insert(
        "owner_detail".to_string(),
        GraphScalar::String(owner.detail.clone()),
    );
    PropertyNode {
        id,
        labels: vec![label.to_string()],
        properties,
    }
}

fn property_resolved_callsite_node(
    index: &Codebase,
    owner: &SymbolTarget,
    call: &CallpathEdge,
) -> PropertyNode {
    let line = call.line.unwrap_or(owner.line_start);
    let text = call.text.clone().unwrap_or_default();
    let id = format!(
        "callsite:{}:{line}:{}:{}",
        symbol_target_key(owner),
        call.target.name,
        call.target.line_start
    );
    let mut properties = BTreeMap::from([
        (
            "name".to_string(),
            GraphScalar::String(call.target.name.clone()),
        ),
        ("line".to_string(), GraphScalar::Integer(line as i64)),
        ("text".to_string(), GraphScalar::String(text)),
        (
            "resolution".to_string(),
            GraphScalar::String(call.relation.clone()),
        ),
        (
            "target_name".to_string(),
            GraphScalar::String(call.target.name.clone()),
        ),
        (
            "target_kind".to_string(),
            GraphScalar::String(call.target.kind.clone()),
        ),
        (
            "target_path".to_string(),
            GraphScalar::String(call.target.path.clone()),
        ),
        (
            "target_line".to_string(),
            GraphScalar::Integer(call.target.line_start as i64),
        ),
        (
            "target_detail".to_string(),
            GraphScalar::String(call.target.detail.clone()),
        ),
    ]);
    if let Some(file) = index.file(&owner.path)
        && let Ok(content) = index.file_content(file)
    {
        if let Some(guard) = preprocessor_guard_at_line(&content, line) {
            properties.insert("guard".to_string(), GraphScalar::String(guard));
            properties.insert("guarded".to_string(), GraphScalar::Boolean(true));
        } else {
            properties.insert("guarded".to_string(), GraphScalar::Boolean(false));
        }
    }
    control_property_node("CallSite", id, owner, properties)
}

fn property_syntax_callsite_node(
    index: &Codebase,
    owner: &SymbolTarget,
    token: &str,
    line: usize,
    text: &str,
) -> PropertyNode {
    let name = token.rsplit('.').next().unwrap_or(token);
    let receiver = token
        .rsplit_once('.')
        .map(|(receiver, _)| receiver)
        .unwrap_or_default();
    let id = format!(
        "callsite:{}:{line}:syntax:{token}",
        symbol_target_key(owner)
    );
    let mut properties = BTreeMap::from([
        ("name".to_string(), GraphScalar::String(name.to_string())),
        (
            "receiver".to_string(),
            GraphScalar::String(receiver.to_string()),
        ),
        ("line".to_string(), GraphScalar::Integer(line as i64)),
        ("text".to_string(), GraphScalar::String(text.to_string())),
        (
            "resolution".to_string(),
            GraphScalar::String("syntax".to_string()),
        ),
    ]);
    if let Some(file) = index.file(&owner.path)
        && let Ok(content) = index.file_content(file)
    {
        if let Some(guard) = preprocessor_guard_at_line(&content, line) {
            properties.insert("guard".to_string(), GraphScalar::String(guard));
            properties.insert("guarded".to_string(), GraphScalar::Boolean(true));
        } else {
            properties.insert("guarded".to_string(), GraphScalar::Boolean(false));
        }
    }
    control_property_node("CallSite", id, owner, properties)
}

fn unresolved_qualified_callsite_nodes_on_line(
    index: &Codebase,
    owner: &SymbolTarget,
    line_number: usize,
    text: &str,
    resolved: &BTreeSet<(usize, String)>,
) -> Vec<PropertyNode> {
    let code = strip_strings_and_line_comment(text);
    qualified_call_tokens(&code)
        .into_iter()
        .filter(|token| {
            let name = token.rsplit('.').next().unwrap_or(token);
            !resolved.contains(&(line_number, name.to_string()))
        })
        .map(|token| property_syntax_callsite_node(index, owner, &token, line_number, text.trim()))
        .collect()
}

fn unresolved_qualified_callsite_nodes(
    index: &Codebase,
    owner: &SymbolTarget,
    resolved: &BTreeSet<(usize, String)>,
) -> Result<Vec<PropertyNode>> {
    let Some(file) = index.file(&owner.path) else {
        return Ok(Vec::new());
    };
    let Some(symbol) = symbol_for_target(file, owner) else {
        return Ok(Vec::new());
    };
    let content = index.file_content(file)?;
    let body = source_line_slice(
        &content,
        symbol.line_start,
        symbol.line_end.max(symbol.line_start),
    );
    let mut callsites = Vec::new();
    for (offset, line) in body.lines().enumerate() {
        callsites.extend(unresolved_qualified_callsite_nodes_on_line(
            index,
            owner,
            symbol.line_start + offset,
            line,
            resolved,
        ));
    }
    callsites.sort_by(|left, right| left.id.cmp(&right.id));
    callsites.dedup_by(|left, right| left.id == right.id);
    Ok(callsites)
}

fn property_callsite_values(callsite: &PropertyNode) -> Vec<PropertyNode> {
    let Some(text) = graph_string_property(callsite, "text") else {
        return Vec::new();
    };
    let Some(name) = graph_string_property(callsite, "name") else {
        return Vec::new();
    };
    let Some(owner) = graph_owner_target(callsite) else {
        return Vec::new();
    };
    let target = graph_callsite_target(callsite);
    call_argument_values(text, name)
        .into_iter()
        .enumerate()
        .map(|(index, expression)| {
            let mut properties = BTreeMap::from([
                ("index".to_string(), GraphScalar::Integer(index as i64)),
                ("expression".to_string(), GraphScalar::String(expression)),
                (
                    "callsite_id".to_string(),
                    GraphScalar::String(callsite.id.clone()),
                ),
                (
                    "callsite_name".to_string(),
                    GraphScalar::String(
                        graph_string_property(callsite, "name")
                            .unwrap_or_default()
                            .to_string(),
                    ),
                ),
                (
                    "callsite_line".to_string(),
                    GraphScalar::Integer(
                        graph_integer_property(callsite, "line").unwrap_or_default(),
                    ),
                ),
                (
                    "callsite_text".to_string(),
                    GraphScalar::String(text.to_string()),
                ),
                (
                    "callsite_resolution".to_string(),
                    GraphScalar::String(
                        graph_string_property(callsite, "resolution")
                            .unwrap_or_default()
                            .to_string(),
                    ),
                ),
            ]);
            if let Some(target) = &target {
                properties.insert(
                    "target_name".to_string(),
                    GraphScalar::String(target.name.clone()),
                );
                properties.insert(
                    "target_kind".to_string(),
                    GraphScalar::String(target.kind.clone()),
                );
                properties.insert(
                    "target_path".to_string(),
                    GraphScalar::String(target.path.clone()),
                );
                properties.insert(
                    "target_line".to_string(),
                    GraphScalar::Integer(target.line_start as i64),
                );
                properties.insert(
                    "target_detail".to_string(),
                    GraphScalar::String(target.detail.clone()),
                );
            }
            control_property_node(
                "Value",
                format!("value:{}:{index}", callsite.id),
                &owner,
                properties,
            )
        })
        .collect()
}

fn property_value_callsite(value: &PropertyNode) -> Option<PropertyNode> {
    let owner = graph_owner_target(value)?;
    let id = graph_string_property(value, "callsite_id")?.to_string();
    let mut properties = BTreeMap::from([
        (
            "name".to_string(),
            GraphScalar::String(
                graph_string_property(value, "callsite_name")
                    .unwrap_or_default()
                    .to_string(),
            ),
        ),
        (
            "line".to_string(),
            GraphScalar::Integer(
                graph_integer_property(value, "callsite_line").unwrap_or_default(),
            ),
        ),
        (
            "text".to_string(),
            GraphScalar::String(
                graph_string_property(value, "callsite_text")
                    .unwrap_or_default()
                    .to_string(),
            ),
        ),
        (
            "resolution".to_string(),
            GraphScalar::String(
                graph_string_property(value, "callsite_resolution")
                    .unwrap_or_default()
                    .to_string(),
            ),
        ),
    ]);
    if let Some(target) = graph_value_target(value) {
        properties.insert("target_name".to_string(), GraphScalar::String(target.name));
        properties.insert("target_kind".to_string(), GraphScalar::String(target.kind));
        properties.insert("target_path".to_string(), GraphScalar::String(target.path));
        properties.insert(
            "target_line".to_string(),
            GraphScalar::Integer(target.line_start as i64),
        );
        properties.insert(
            "target_detail".to_string(),
            GraphScalar::String(target.detail),
        );
    }
    Some(control_property_node("CallSite", id, &owner, properties))
}

fn graph_callsite_target(node: &PropertyNode) -> Option<SymbolTarget> {
    Some(SymbolTarget {
        name: graph_string_property(node, "target_name")?.to_string(),
        kind: graph_string_property(node, "target_kind")?.to_string(),
        path: graph_string_property(node, "target_path")?.to_string(),
        line_start: graph_integer_property(node, "target_line")? as usize,
        detail: graph_string_property(node, "target_detail")
            .unwrap_or_default()
            .to_string(),
    })
}

fn graph_value_target(node: &PropertyNode) -> Option<SymbolTarget> {
    graph_callsite_target(node)
}

fn call_argument_values(line: &str, name: &str) -> Vec<String> {
    let mut from = 0usize;
    while let Some(relative) = line.get(from..).and_then(|tail| tail.find(name)) {
        let start = from + relative;
        let end = start + name.len();
        let before_ok = line
            .get(..start)
            .and_then(|prefix| prefix.chars().next_back())
            .is_none_or(|ch| !is_identifier_char(ch));
        let Some(suffix) = line.get(end..) else {
            break;
        };
        let trimmed = suffix.trim_start();
        if before_ok && trimmed.starts_with('(') {
            if let Some(arguments) = delimited_argument_values(trimmed) {
                return arguments;
            }
        }
        from = end.max(from + 1);
    }
    Vec::new()
}

fn delimited_argument_values(value: &str) -> Option<Vec<String>> {
    if !value.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    let mut quote = None::<char>;
    let mut escaped = false;
    let mut start = 1usize;
    let mut arguments = Vec::new();
    for (index, ch) in value.char_indices() {
        if index == 0 {
            depth = 1;
            continue;
        }
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let argument = value.get(start..index)?.trim();
                    if !argument.is_empty() {
                        arguments.push(argument.to_string());
                    }
                    return Some(arguments);
                }
            }
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if depth == 1 && bracket_depth == 0 && brace_depth == 0 => {
                arguments.push(value.get(start..index)?.trim().to_string());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    None
}

fn graph_owner_target(node: &PropertyNode) -> Option<SymbolTarget> {
    Some(SymbolTarget {
        name: graph_string_property(node, "owner_name")?.to_string(),
        kind: graph_string_property(node, "owner_kind")?.to_string(),
        path: graph_string_property(node, "owner_path")?.to_string(),
        line_start: graph_integer_property(node, "owner_line")? as usize,
        detail: graph_string_property(node, "owner_detail")
            .unwrap_or_default()
            .to_string(),
    })
}

fn virtual_property_edge(
    source: &str,
    target: &str,
    edge_type: &str,
    properties: BTreeMap<String, GraphScalar>,
) -> PropertyEdge {
    PropertyEdge {
        id: format!("{source}:{edge_type}:{target}"),
        source: source.to_string(),
        target: target.to_string(),
        edge_type: edge_type.to_string(),
        properties,
    }
}

#[derive(Clone)]
struct SharedStateAccess {
    owner: SymbolTarget,
    state: SymbolTarget,
    relation: &'static str,
    lines: Vec<usize>,
    evidence: Vec<String>,
}

fn symbol_shared_state_accesses(
    index: &Codebase,
    owner: &SymbolTarget,
) -> Result<Vec<SharedStateAccess>> {
    let Some(file) = index.file(&owner.path) else {
        return Ok(Vec::new());
    };
    let Some(symbol) = symbol_for_target(file, owner) else {
        return Ok(Vec::new());
    };
    let content = index.file_content(file)?;
    let active = mask_comments(file.language.as_str(), &content);
    let body = source_line_slice(
        &active,
        symbol.line_start,
        symbol.line_end.max(symbol.line_start),
    );
    let identifiers = raw_identifiers(&body).into_iter().collect::<BTreeSet<_>>();
    let owner_type = enclosing_type_symbol(file, symbol).map(|symbol| symbol.name.clone());
    let mut states = Vec::<SymbolTarget>::new();
    for identifier in identifiers {
        if !is_plausible_shared_state_name(&identifier) {
            continue;
        }
        let mut candidates = index
            .symbols_named(&identifier)
            .into_iter()
            .filter(|(_, candidate)| is_shared_state_symbol(candidate))
            .filter(|(candidate_file, candidate)| {
                candidate_file.path == file.path
                    || owner_type.as_ref().is_some_and(|owner_type| {
                        enclosing_type_symbol(candidate_file, candidate)
                            .is_some_and(|candidate_type| candidate_type.name == *owner_type)
                    })
            })
            .map(|(candidate_file, candidate)| target_from_symbol(candidate_file, candidate))
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            let global = index
                .symbols_named(&identifier)
                .into_iter()
                .filter(|(_, candidate)| is_shared_state_symbol(candidate))
                .collect::<Vec<_>>();
            if global.len() == 1 && body_contains_non_call_member_read(&body, &identifier) {
                states.push(target_from_symbol(global[0].0, global[0].1));
            } else if let Some(state) = synthetic_shared_state_target(file, &identifier, &active) {
                states.push(state);
            }
        } else {
            states.append(&mut candidates);
        }
    }
    states.sort_by_key(symbol_target_key);
    states.dedup_by(|left, right| symbol_target_key(left) == symbol_target_key(right));

    let mut accesses = Vec::new();
    for state in states {
        let mut reads = Vec::new();
        let mut read_text = Vec::new();
        let mut writes = Vec::new();
        let mut write_text = Vec::new();
        for (offset, line) in body.lines().enumerate() {
            let code = strip_strings_and_line_comment(line);
            if !line_contains_identifier_token(&code, &state.name) {
                continue;
            }
            let line_number = symbol.line_start + offset;
            if line_writes_shared_state(&code, &state.name) {
                writes.push(line_number);
                write_text.push(line.trim().to_string());
            }
            if line_reads_shared_state(&code, &state.name) {
                reads.push(line_number);
                read_text.push(line.trim().to_string());
            }
        }
        if !reads.is_empty() {
            accesses.push(SharedStateAccess {
                owner: owner.clone(),
                state: state.clone(),
                relation: "READS",
                lines: reads,
                evidence: read_text,
            });
        }
        if !writes.is_empty() {
            accesses.push(SharedStateAccess {
                owner: owner.clone(),
                state,
                relation: "WRITES",
                lines: writes,
                evidence: write_text,
            });
        }
    }
    Ok(accesses)
}

fn incoming_shared_state_accesses(
    index: &Codebase,
    state: &SymbolTarget,
) -> Result<Vec<SharedStateAccess>> {
    if let Some(file) = index.file(&state.path) {
        let state_type =
            enclosing_type_at_line(file, state.line_start).map(|symbol| symbol.name.clone());
        let mut local = Vec::new();
        for symbol in file
            .symbols
            .iter()
            .filter(|symbol| is_context_handoff_source_symbol(symbol))
            .filter(|symbol| {
                state_type.as_ref().is_none_or(|state_type| {
                    enclosing_type_symbol(file, symbol)
                        .is_some_and(|owner_type| owner_type.name == *state_type)
                })
            })
        {
            let owner = target_from_symbol(file, symbol);
            local.extend(
                symbol_shared_state_accesses(index, &owner)?
                    .into_iter()
                    .filter(|access| symbol_target_key(&access.state) == symbol_target_key(state)),
            );
        }
        if !local.is_empty() {
            return Ok(local);
        }
    }
    let mut owners = BTreeMap::<String, SymbolTarget>::new();
    for hit in reference_candidates(index, &state.name)? {
        let Some(scope) = hit.scope else {
            continue;
        };
        let Some(file) = index.file(&hit.path) else {
            continue;
        };
        let Some(symbol) = file.symbols.iter().find(|symbol| {
            symbol.line_start == scope.start
                && symbol.line_end == scope.end
                && symbol.name == scope.name
                && is_context_handoff_source_symbol(symbol)
        }) else {
            continue;
        };
        let owner = target_from_symbol(file, symbol);
        owners.entry(symbol_target_key(&owner)).or_insert(owner);
    }
    let mut accesses = Vec::new();
    for owner in owners.into_values() {
        accesses.extend(
            symbol_shared_state_accesses(index, &owner)?
                .into_iter()
                .filter(|access| symbol_target_key(&access.state) == symbol_target_key(state)),
        );
    }
    Ok(accesses)
}

fn enclosing_type_at_line(file: &FileEntry, line: usize) -> Option<&Symbol> {
    file.symbols
        .iter()
        .filter(|symbol| {
            matches!(
                symbol.kind.as_str(),
                "class" | "interface" | "struct" | "record" | "trait" | "impl" | "module"
            ) && symbol.line_start <= line
                && line <= symbol.line_end.max(symbol.line_start)
        })
        .min_by_key(|symbol| symbol.line_end.saturating_sub(symbol.line_start))
}

fn is_shared_state_symbol(symbol: &Symbol) -> bool {
    is_plausible_shared_state_name(&symbol.name)
        && matches!(
            symbol.kind.as_str(),
            "field" | "property" | "static" | "const" | "variable"
        )
}

fn is_plausible_shared_state_name(name: &str) -> bool {
    !matches!(
        name,
        "new"
            | "private"
            | "public"
            | "protected"
            | "internal"
            | "static"
            | "readonly"
            | "const"
            | "void"
            | "bool"
            | "byte"
            | "sbyte"
            | "short"
            | "ushort"
            | "int"
            | "uint"
            | "long"
            | "ulong"
            | "float"
            | "double"
            | "decimal"
            | "char"
            | "string"
            | "object"
            | "var"
            | "this"
            | "base"
            | "null"
            | "true"
            | "false"
    )
}

fn body_contains_non_call_member_read(body: &str, name: &str) -> bool {
    body.lines().any(|line| {
        let code = strip_strings_and_line_comment(line);
        [format!(".{name}"), format!("::{name}")]
            .into_iter()
            .any(|needle| {
                let mut from = 0usize;
                while let Some(relative) = code.get(from..).and_then(|tail| tail.find(&needle)) {
                    let end = from + relative + needle.len();
                    let suffix = code.get(end..).unwrap_or_default().trim_start();
                    if !suffix.starts_with('(') {
                        return true;
                    }
                    from = end.max(from + 1);
                }
                false
            })
    })
}

fn synthetic_shared_state_target(
    file: &FileEntry,
    name: &str,
    active_content: &str,
) -> Option<SymbolTarget> {
    for (offset, line) in active_content.lines().enumerate() {
        let line_number = offset + 1;
        if !line_contains_identifier_token(line, name)
            || file.symbols.iter().any(|symbol| {
                is_context_handoff_source_symbol(symbol)
                    && symbol.line_start <= line_number
                    && line_number <= symbol.line_end.max(symbol.line_start)
            })
        {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("using ")
            || trimmed.starts_with("namespace ")
            || trimmed.contains(" class ")
            || trimmed.contains(" interface ")
            || trimmed.contains(" struct ")
            || (!trimmed.contains(';') && !trimmed.contains("{ get") && !trimmed.contains("=>"))
        {
            continue;
        }
        return Some(SymbolTarget {
            name: name.to_string(),
            kind: "field".to_string(),
            path: file.path.clone(),
            line_start: line_number,
            detail: trimmed.to_string(),
        });
    }
    None
}

fn line_writes_shared_state(line: &str, name: &str) -> bool {
    let Some(position) = line.find(name) else {
        return false;
    };
    let suffix = line
        .get(position + name.len()..)
        .unwrap_or_default()
        .trim_start();
    suffix.starts_with('=')
        || suffix.starts_with("+=")
        || suffix.starts_with("-=")
        || suffix.starts_with("++")
        || suffix.starts_with("--")
        || suffix.starts_with(".Add(")
        || suffix.starts_with(".Remove(")
        || suffix.starts_with(".Clear(")
}

fn line_reads_shared_state(line: &str, name: &str) -> bool {
    if !line_contains_identifier_token(line, name) {
        return false;
    }
    let Some(position) = line.find(name) else {
        return true;
    };
    let suffix = line
        .get(position + name.len()..)
        .unwrap_or_default()
        .trim_start();
    !suffix.starts_with('=')
        || line.get(position + name.len()..).is_some_and(|tail| {
            tail.find('=').is_some_and(|equal| {
                line.get(position + name.len() + equal + 1..)
                    .is_some_and(|right| line_contains_identifier_token(right, name))
            })
        })
}

fn property_symbol_node(index: &Codebase, target: &SymbolTarget) -> PropertyNode {
    let symbol = index
        .file(&target.path)
        .and_then(|file| symbol_for_target(file, target));
    let line_end = symbol
        .map(|symbol| symbol.line_end.max(symbol.line_start))
        .unwrap_or(target.line_start);
    let language = index
        .file(&target.path)
        .map(|file| file.language.to_string());
    let mut properties = BTreeMap::from([
        ("name".to_string(), GraphScalar::String(target.name.clone())),
        ("kind".to_string(), GraphScalar::String(target.kind.clone())),
        ("path".to_string(), GraphScalar::String(target.path.clone())),
        (
            "line_start".to_string(),
            GraphScalar::Integer(target.line_start as i64),
        ),
        (
            "line_end".to_string(),
            GraphScalar::Integer(line_end as i64),
        ),
        (
            "detail".to_string(),
            GraphScalar::String(target.detail.clone()),
        ),
    ]);
    if let Some(language) = language {
        properties.insert("language".to_string(), GraphScalar::String(language));
    }
    if let Some(symbol) = symbol
        && let Some(file) = index.file(&target.path)
        && let Some(enclosing) = enclosing_type_symbol(file, symbol)
    {
        properties.insert(
            "enclosing_type".to_string(),
            GraphScalar::String(enclosing.name.clone()),
        );
    }
    let kind_label = graph_kind_label(&target.kind);
    PropertyNode {
        id: symbol_target_key(target),
        labels: vec!["Symbol".to_string(), kind_label],
        properties,
    }
}

fn property_shared_state_node(index: &Codebase, target: &SymbolTarget) -> PropertyNode {
    let mut node = property_symbol_node(index, target);
    node.id = format!("state:{}", node.id);
    node.labels.insert(0, "SharedState".to_string());
    node
}

fn property_file_node(
    index: &Codebase,
    graph: &crate::graph::CodeGraph,
    file: &FileEntry,
) -> PropertyNode {
    let graph_id = graph.file_graph.id(&file.path);
    let community = graph_id.and_then(|id| graph.file_graph.community(id));
    let degree = graph_id.map_or(0, |id| graph.file_graph.degree(id));
    let boundary_degree = graph_id.map_or(0, |id| {
        graph
            .file_graph
            .neighbor_ids(id)
            .into_iter()
            .filter(|neighbor| graph.file_graph.community(*neighbor) != community)
            .count()
    });
    let outgoing_degree = index.deps_for(&file.path).len();
    let incoming_degree = index.reverse_deps_for(&file.path).len();
    let mut properties = BTreeMap::from([
        ("path".to_string(), GraphScalar::String(file.path.clone())),
        (
            "language".to_string(),
            GraphScalar::String(file.language.to_string()),
        ),
        (
            "line_count".to_string(),
            GraphScalar::Integer(file.line_count as i64),
        ),
        (
            "symbol_count".to_string(),
            GraphScalar::Integer(file.symbols.len() as i64),
        ),
        ("degree".to_string(), GraphScalar::Integer(degree as i64)),
        (
            "outgoing_degree".to_string(),
            GraphScalar::Integer(outgoing_degree as i64),
        ),
        (
            "incoming_degree".to_string(),
            GraphScalar::Integer(incoming_degree as i64),
        ),
        (
            "boundary_degree".to_string(),
            GraphScalar::Integer(boundary_degree as i64),
        ),
    ]);
    if let Some(community) = community {
        properties.insert(
            "community".to_string(),
            GraphScalar::Integer(community as i64),
        );
    }
    let mut labels = vec!["File".to_string()];
    if incoming_degree == 0 && outgoing_degree > 0 {
        labels.push("EntryFile".to_string());
    }
    if boundary_degree > 0 {
        labels.push("BoundaryFile".to_string());
    }
    if outgoing_degree == 0 && incoming_degree > 0 {
        labels.push("SinkFile".to_string());
    }
    PropertyNode {
        id: format!("file:{}", file.path),
        labels,
        properties,
    }
}

fn graph_file_community(graph: &crate::graph::CodeGraph, path: &str) -> Option<usize> {
    graph
        .file_graph
        .id(path)
        .and_then(|id| graph.file_graph.community(id))
}

fn property_community_nodes(graph: &crate::graph::CodeGraph) -> BTreeMap<usize, PropertyNode> {
    #[derive(Default)]
    struct Metrics {
        size: usize,
        total_degree: usize,
        internal_links: usize,
        boundary_links: usize,
        representative: Option<(usize, String)>,
        path_prefix: Option<Vec<String>>,
    }

    let mut metrics = BTreeMap::<usize, Metrics>::new();
    for (id, path) in graph.file_graph.paths.iter().enumerate() {
        let Some(community) = graph.file_graph.community(id) else {
            continue;
        };
        let degree = graph.file_graph.degree(id);
        let row = metrics.entry(community).or_default();
        row.size += 1;
        row.total_degree += degree;
        let components = path.split('/').map(str::to_string).collect::<Vec<_>>();
        if let Some(prefix) = &mut row.path_prefix {
            let shared = prefix
                .iter()
                .zip(&components)
                .take_while(|(left, right)| left == right)
                .count();
            prefix.truncate(shared);
        } else {
            row.path_prefix = Some(components);
        }
        match &row.representative {
            Some((current_degree, current_path))
                if *current_degree > degree
                    || (*current_degree == degree && current_path <= path) => {}
            _ => row.representative = Some((degree, path.clone())),
        }
        for neighbor in graph.file_graph.neighbor_ids(id) {
            if graph.file_graph.community(neighbor) == Some(community) {
                row.internal_links += 1;
            } else {
                row.boundary_links += 1;
            }
        }
    }

    metrics
        .into_iter()
        .map(|(community, metrics)| {
            let (max_degree, representative_path) = metrics.representative.unwrap_or_default();
            let name = metrics
                .path_prefix
                .filter(|prefix| !prefix.is_empty())
                .map(|prefix| prefix.join("/"))
                .unwrap_or_else(|| representative_path.clone());
            (
                community,
                PropertyNode {
                    id: format!("community:{community}"),
                    labels: vec!["Community".to_string()],
                    properties: BTreeMap::from([
                        ("id".to_string(), GraphScalar::Integer(community as i64)),
                        ("name".to_string(), GraphScalar::String(name)),
                        (
                            "size".to_string(),
                            GraphScalar::Integer(metrics.size as i64),
                        ),
                        (
                            "total_degree".to_string(),
                            GraphScalar::Integer(metrics.total_degree as i64),
                        ),
                        (
                            "max_degree".to_string(),
                            GraphScalar::Integer(max_degree as i64),
                        ),
                        (
                            "internal_links".to_string(),
                            GraphScalar::Integer(metrics.internal_links as i64),
                        ),
                        (
                            "boundary_links".to_string(),
                            GraphScalar::Integer(metrics.boundary_links as i64),
                        ),
                        (
                            "representative_path".to_string(),
                            GraphScalar::String(representative_path),
                        ),
                    ]),
                },
            )
        })
        .collect()
}

fn property_call_edge(
    index: &Codebase,
    source: &SymbolTarget,
    target: &SymbolTarget,
    edge: &CallpathEdge,
) -> PropertyEdge {
    let mut properties = BTreeMap::from([(
        "resolution".to_string(),
        GraphScalar::String(edge.relation.clone()),
    )]);
    if let Some(line) = edge.line {
        properties.insert("line".to_string(), GraphScalar::Integer(line as i64));
        if let Some(file) = index.file(&source.path)
            && let Ok(content) = index.file_content(file)
        {
            if let Some(guard) = preprocessor_guard_at_line(&content, line) {
                properties.insert("guard".to_string(), GraphScalar::String(guard));
                properties.insert("guarded".to_string(), GraphScalar::Boolean(true));
            } else {
                properties.insert("guarded".to_string(), GraphScalar::Boolean(false));
            }
        }
    }
    if let Some(text) = &edge.text {
        properties.insert("text".to_string(), GraphScalar::String(text.clone()));
    }
    property_edge(source, target, "CALLS", properties)
}

fn property_reference_edge(
    index: &Codebase,
    source: &SymbolTarget,
    target: &SymbolTarget,
    edge: &CallpathEdge,
) -> PropertyEdge {
    let mut result = property_call_edge(index, source, target, edge);
    result.edge_type = "REFERENCES".to_string();
    result.id = property_edge_id(source, target, "REFERENCES", edge.line);
    result
}

fn property_state_access_edge(source: &SymbolTarget, access: &SharedStateAccess) -> PropertyEdge {
    let target_id = format!("state:{}", symbol_target_key(&access.state));
    let mut properties = BTreeMap::new();
    if let Some(line) = access.lines.first() {
        properties.insert("line".to_string(), GraphScalar::Integer(*line as i64));
    }
    properties.insert(
        "lines".to_string(),
        GraphScalar::String(
            access
                .lines
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    properties.insert(
        "evidence".to_string(),
        GraphScalar::String(access.evidence.join(" | ")),
    );
    PropertyEdge {
        id: format!(
            "{}:{}:{}:{}",
            symbol_target_key(source),
            access.relation,
            target_id,
            access.lines.first().copied().unwrap_or_default()
        ),
        source: symbol_target_key(source),
        target: target_id,
        edge_type: access.relation.to_string(),
        properties,
    }
}

fn property_edge(
    source: &SymbolTarget,
    target: &SymbolTarget,
    edge_type: &str,
    properties: BTreeMap<String, GraphScalar>,
) -> PropertyEdge {
    let line = properties.get("line").and_then(|value| match value {
        GraphScalar::Integer(value) => Some(*value as usize),
        _ => None,
    });
    PropertyEdge {
        id: property_edge_id(source, target, edge_type, line),
        source: symbol_target_key(source),
        target: symbol_target_key(target),
        edge_type: edge_type.to_string(),
        properties,
    }
}

fn property_edge_id(
    source: &SymbolTarget,
    target: &SymbolTarget,
    edge_type: &str,
    line: Option<usize>,
) -> String {
    format!(
        "{}:{}:{}:{}",
        symbol_target_key(source),
        edge_type,
        symbol_target_key(target),
        line.unwrap_or_default()
    )
}

fn property_file_contains_edge(path: &str, target: &SymbolTarget) -> PropertyEdge {
    PropertyEdge {
        id: format!("file:{path}:CONTAINS:{}", symbol_target_key(target)),
        source: format!("file:{path}"),
        target: symbol_target_key(target),
        edge_type: "CONTAINS".to_string(),
        properties: BTreeMap::new(),
    }
}

fn property_file_dependency_edge(source: &str, target: &str) -> PropertyEdge {
    PropertyEdge {
        id: format!("file:{source}:DEPENDS_ON:file:{target}"),
        source: format!("file:{source}"),
        target: format!("file:{target}"),
        edge_type: "DEPENDS_ON".to_string(),
        properties: BTreeMap::from([("count".to_string(), GraphScalar::Integer(1))]),
    }
}

fn graph_string_property<'a>(node: &'a PropertyNode, name: &str) -> Option<&'a str> {
    match node.properties.get(name)? {
        GraphScalar::String(value) => Some(value),
        _ => None,
    }
}

fn graph_integer_property(node: &PropertyNode, name: &str) -> Option<i64> {
    match node.properties.get(name)? {
        GraphScalar::Integer(value) => Some(*value),
        _ => None,
    }
}

fn graph_relation_requested(relationship: &GraphRelationshipPattern, relation: &str) -> bool {
    relationship.types.is_empty()
        || relationship
            .types
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(relation))
}

fn graph_node_matches_pattern(node: &PropertyNode, pattern: &GraphNodePattern) -> bool {
    pattern
        .label
        .as_ref()
        .is_none_or(|label| node.labels.iter().any(|candidate| candidate == label))
        && pattern
            .properties
            .iter()
            .all(|(name, value)| node.properties.get(name) == Some(value))
}

fn graph_kind_label(kind: &str) -> String {
    let mut chars = kind.chars();
    chars
        .next()
        .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_else(|| "Symbol".to_string())
}

fn graph_sort_and_dedup(values: &mut Vec<(PropertyEdge, PropertyNode)>) {
    values.sort_by(|left, right| {
        left.1
            .id
            .cmp(&right.1.id)
            .then_with(|| left.0.edge_type.cmp(&right.0.edge_type))
            .then_with(|| left.0.id.cmp(&right.0.id))
    });
    values.dedup_by(|left, right| left.0.id == right.0.id && left.1.id == right.1.id);
}

#[derive(Clone)]
struct CallpathParent {
    previous: String,
    relation: String,
}

#[derive(Clone)]
struct CallpathNext {
    next: String,
    relation: String,
}

fn lazy_symbol_callpath(
    index: &Codebase,
    from: &str,
    to: &str,
    source_path: Option<&str>,
    source_line: Option<usize>,
    target_path: Option<&str>,
    target_line: Option<usize>,
    max_depth: usize,
) -> Result<Option<Value>> {
    let Some(mut scoped) = lazy_symbol_callpath_with_scope(
        index,
        from,
        to,
        source_path,
        source_line,
        target_path,
        target_line,
        max_depth,
        Some(2),
        false,
    )?
    else {
        return Ok(None);
    };
    if scoped.get("found").and_then(Value::as_bool) == Some(true) {
        if let Some(object) = scoped.as_object_mut() {
            object.insert(
                "candidate_scope".to_string(),
                json!("broad_file_graph_corridor"),
            );
        }
        return Ok(Some(scoped));
    }

    let strict_expanded = scoped
        .get("expanded")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let Some(mut corridor) = lazy_symbol_callpath_with_scope(
        index,
        from,
        to,
        source_path,
        source_line,
        target_path,
        target_line,
        max_depth,
        Some(2),
        true,
    )?
    else {
        return Ok(Some(scoped));
    };
    if corridor.get("found").and_then(Value::as_bool) == Some(true) {
        if let Some(object) = corridor.as_object_mut() {
            object.insert(
                "candidate_scope".to_string(),
                json!("broad_file_graph_corridor_with_weak_edges"),
            );
            object.insert(
                "strict_attempt_expanded".to_string(),
                json!(strict_expanded),
            );
        }
        return Ok(Some(corridor));
    }

    let corridor_expanded = corridor
        .get("expanded")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let Some(mut fallback) = lazy_symbol_callpath_with_scope(
        index,
        from,
        to,
        source_path,
        source_line,
        target_path,
        target_line,
        max_depth,
        None,
        true,
    )?
    else {
        return Ok(Some(scoped));
    };
    if let Some(object) = fallback.as_object_mut() {
        object.insert(
            "candidate_scope".to_string(),
            json!("unrestricted_fallback"),
        );
        object.insert(
            "corridor_attempt_expanded".to_string(),
            json!(corridor_expanded),
        );
        object.insert(
            "strict_attempt_expanded".to_string(),
            json!(strict_expanded),
        );
    }
    Ok(Some(fallback))
}

#[allow(clippy::too_many_arguments)]
fn lazy_symbol_callpath_with_scope(
    index: &Codebase,
    from: &str,
    to: &str,
    source_path: Option<&str>,
    source_line: Option<usize>,
    target_path: Option<&str>,
    target_line: Option<usize>,
    max_depth: usize,
    corridor_depth: Option<usize>,
    include_weak_references: bool,
) -> Result<Option<Value>> {
    let sources = callpath_symbol_candidates(index, from, source_path, source_line, usize::MAX);
    let targets = callpath_symbol_candidates(index, to, target_path, target_line, usize::MAX);
    if sources.is_empty() || targets.is_empty() {
        return Ok(None);
    }

    let mut nodes = HashMap::<String, SymbolTarget>::new();
    let mut forward_parent = HashMap::<String, CallpathParent>::new();
    let mut backward_next = HashMap::<String, CallpathNext>::new();
    let mut forward_distance = HashMap::<String, usize>::new();
    let mut backward_distance = HashMap::<String, usize>::new();
    let mut forward_frontier = Vec::<String>::new();
    let mut backward_frontier = Vec::<String>::new();

    for source in sources.iter().cloned() {
        let key = symbol_target_key(&source);
        nodes.insert(key.clone(), source);
        if forward_distance.insert(key.clone(), 0).is_none() {
            forward_frontier.push(key);
        }
    }
    for target in targets.iter().cloned() {
        let key = symbol_target_key(&target);
        nodes.insert(key.clone(), target);
        if backward_distance.insert(key.clone(), 0).is_none() {
            backward_frontier.push(key);
        }
    }
    forward_frontier.sort();
    backward_frontier.sort();

    if let Some(key) = forward_distance
        .keys()
        .filter(|key| backward_distance.contains_key(*key))
        .min()
        .cloned()
    {
        let path = reconstruct_bidirectional_callpath(&key, &forward_parent, &backward_next);
        return Ok(Some(format_lazy_symbol_callpath(
            index,
            true,
            from,
            to,
            &path,
            &nodes,
            sources.len(),
            targets.len(),
            0,
            0,
            None,
        )));
    }

    let mut outgoing_cache = HashMap::<String, Vec<CallpathEdge>>::new();
    let mut incoming_cache = HashMap::<String, Vec<CallpathEdge>>::new();
    let allowed_paths = corridor_depth
        .map(|depth| callpath_graph_corridor(index, &sources, &targets, depth))
        .unwrap_or_default();
    let allowed_paths = (!allowed_paths.is_empty()).then_some(allowed_paths);
    let mut active_content_cache = HashMap::<String, Arc<String>>::new();
    let mut forward_depth = 0usize;
    let mut backward_depth = 0usize;
    let mut expanded_forward = 0usize;
    let mut expanded_backward = 0usize;
    let mut best_meeting: Option<(usize, String)> = None;
    let mut backward_cost_cache = HashMap::<String, usize>::new();

    while !forward_frontier.is_empty() || !backward_frontier.is_empty() {
        if forward_frontier.is_empty() {
            break;
        }
        if forward_depth.saturating_add(backward_depth) >= max_depth {
            break;
        }
        if best_meeting
            .as_ref()
            .is_some_and(|(hops, _)| forward_depth.saturating_add(backward_depth) >= *hops)
        {
            break;
        }

        let can_expand_forward = !forward_frontier.is_empty() && forward_depth < max_depth;
        let can_expand_backward = !backward_frontier.is_empty() && backward_depth < max_depth;
        if !can_expand_forward && !can_expand_backward {
            break;
        }
        let forward_cost = can_expand_forward.then(|| {
            callpath_forward_frontier_cost(index, &nodes, &forward_frontier, &outgoing_cache)
        });
        let backward_cost = if can_expand_backward {
            Some(callpath_backward_frontier_cost(
                index,
                &nodes,
                &backward_frontier,
                &mut backward_cost_cache,
            )?)
        } else {
            None
        };
        let expand_forward = can_expand_forward
            && (!can_expand_backward
                || forward_cost.unwrap_or(usize::MAX) <= backward_cost.unwrap_or(usize::MAX));

        if expand_forward {
            let next_depth = forward_depth + 1;
            let mut next_frontier = Vec::<String>::new();
            for current_key in std::mem::take(&mut forward_frontier) {
                let Some(current) = nodes.get(&current_key).cloned() else {
                    continue;
                };
                expanded_forward += 1;
                let edges = cached_callpath_edges(
                    index,
                    &current_key,
                    &current,
                    allowed_paths.as_ref(),
                    include_weak_references,
                    &mut active_content_cache,
                    &mut outgoing_cache,
                )?;
                for edge in edges {
                    let next_key = symbol_target_key(&edge.target);
                    if forward_distance.contains_key(&next_key) {
                        continue;
                    }
                    nodes.insert(next_key.clone(), edge.target);
                    forward_distance.insert(next_key.clone(), next_depth);
                    forward_parent.insert(
                        next_key.clone(),
                        CallpathParent {
                            previous: current_key.clone(),
                            relation: edge.relation,
                        },
                    );
                    if let Some(backward_hops) = backward_distance.get(&next_key) {
                        update_callpath_meeting(
                            &mut best_meeting,
                            next_depth.saturating_add(*backward_hops),
                            &next_key,
                            max_depth,
                        );
                    }
                    next_frontier.push(next_key);
                }
            }
            next_frontier.sort();
            next_frontier.dedup();
            forward_frontier = next_frontier;
            forward_depth = next_depth;
        } else {
            let next_depth = backward_depth + 1;
            let mut next_frontier = Vec::<String>::new();
            for current_key in std::mem::take(&mut backward_frontier) {
                let Some(current) = nodes.get(&current_key).cloned() else {
                    continue;
                };
                expanded_backward += 1;
                let edges = cached_callpath_incoming_edges(
                    index,
                    &current_key,
                    &current,
                    allowed_paths.as_ref(),
                    include_weak_references,
                    &mut active_content_cache,
                    &mut incoming_cache,
                    &mut outgoing_cache,
                )?;
                for edge in edges {
                    let previous_key = symbol_target_key(&edge.target);
                    if backward_distance.contains_key(&previous_key) {
                        continue;
                    }
                    nodes.insert(previous_key.clone(), edge.target);
                    backward_distance.insert(previous_key.clone(), next_depth);
                    backward_next.insert(
                        previous_key.clone(),
                        CallpathNext {
                            next: current_key.clone(),
                            relation: edge.relation,
                        },
                    );
                    if let Some(forward_hops) = forward_distance.get(&previous_key) {
                        update_callpath_meeting(
                            &mut best_meeting,
                            next_depth.saturating_add(*forward_hops),
                            &previous_key,
                            max_depth,
                        );
                    }
                    next_frontier.push(previous_key);
                }
            }
            next_frontier.sort();
            next_frontier.dedup();
            backward_frontier = next_frontier;
            backward_depth = next_depth;
        }
    }

    if let Some((_, meeting)) = best_meeting {
        let path = reconstruct_bidirectional_callpath(&meeting, &forward_parent, &backward_next);
        return Ok(Some(format_lazy_symbol_callpath(
            index,
            true,
            from,
            to,
            &path,
            &nodes,
            sources.len(),
            targets.len(),
            expanded_forward,
            expanded_backward,
            None,
        )));
    }

    Ok(Some(format_lazy_symbol_callpath(
        index,
        false,
        from,
        to,
        &[],
        &nodes,
        sources.len(),
        targets.len(),
        expanded_forward,
        expanded_backward,
        Some("no directed symbol path found within max_hops"),
    )))
}

fn callpath_forward_frontier_cost(
    index: &Codebase,
    nodes: &HashMap<String, SymbolTarget>,
    frontier: &[String],
    outgoing_cache: &HashMap<String, Vec<CallpathEdge>>,
) -> usize {
    frontier.iter().fold(0usize, |cost, key| {
        let node_cost = outgoing_cache.get(key).map(Vec::len).unwrap_or_else(|| {
            nodes
                .get(key)
                .and_then(|target| index.file(&target.path).map(|file| (target, file)))
                .and_then(|(target, file)| symbol_for_target(file, target))
                .map(|symbol| {
                    symbol
                        .line_end
                        .saturating_sub(symbol.line_start)
                        .saturating_add(1)
                })
                .unwrap_or(1)
        });
        cost.saturating_add(node_cost.max(1))
    })
}

fn callpath_backward_frontier_cost(
    index: &Codebase,
    nodes: &HashMap<String, SymbolTarget>,
    frontier: &[String],
    cache: &mut HashMap<String, usize>,
) -> Result<usize> {
    let mut cost = 0usize;
    for key in frontier {
        let Some(target) = nodes.get(key) else {
            continue;
        };
        let name_cost = if let Some(cost) = cache.get(&target.name) {
            *cost
        } else {
            let count = index
                .word_hits(&target.name)?
                .len()
                .max(1)
                .saturating_mul(8);
            cache.insert(target.name.clone(), count);
            count
        };
        cost = cost.saturating_add(name_cost);
    }
    Ok(cost)
}

#[derive(Clone)]
struct CallpathEdge {
    target: SymbolTarget,
    relation: String,
    line: Option<usize>,
    text: Option<String>,
}

fn callpath_graph_corridor(
    index: &Codebase,
    sources: &[SymbolTarget],
    targets: &[SymbolTarget],
    neighbor_depth: usize,
) -> BTreeSet<String> {
    if let Some(paths) = callpath_dependency_corridor(index, sources, targets, neighbor_depth) {
        return paths;
    }
    let graph = index.graph();
    let file_graph = &graph.file_graph;
    if file_graph.paths.is_empty() {
        return BTreeSet::new();
    }
    let allowed = vec![true; file_graph.paths.len()];
    let mut candidate_ids = BTreeSet::<usize>::new();
    let mut found_route = false;
    for source in sources {
        let Some(source_id) = file_graph.id(&source.path) else {
            continue;
        };
        for target in targets {
            let Some(target_id) = file_graph.id(&target.path) else {
                continue;
            };
            let route = file_graph.weighted_shortest_path(source_id, target_id, &allowed);
            if route.is_empty() {
                continue;
            }
            found_route = true;
            candidate_ids.extend(route);
        }
    }
    if !found_route {
        return BTreeSet::new();
    }

    let mut frontier = candidate_ids.iter().copied().collect::<Vec<_>>();
    for _ in 0..neighbor_depth {
        let mut next = Vec::new();
        for file_id in frontier {
            for neighbor in file_graph.neighbor_ids(file_id) {
                if candidate_ids.insert(neighbor) {
                    next.push(neighbor);
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    let mut paths = candidate_ids
        .into_iter()
        .filter_map(|file_id| file_graph.paths.get(file_id).cloned())
        .collect::<BTreeSet<_>>();
    paths.extend(sources.iter().map(|source| source.path.clone()));
    paths.extend(targets.iter().map(|target| target.path.clone()));
    paths
}

fn callpath_dependency_corridor(
    index: &Codebase,
    sources: &[SymbolTarget],
    targets: &[SymbolTarget],
    neighbor_depth: usize,
) -> Option<BTreeSet<String>> {
    let source_paths = sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<BTreeSet<_>>();
    let target_paths = targets
        .iter()
        .map(|target| target.path.clone())
        .collect::<BTreeSet<_>>();
    if let Some(shared) = source_paths.intersection(&target_paths).next() {
        return Some(BTreeSet::from([shared.clone()]));
    }

    let mut forward_frontier = source_paths.iter().cloned().collect::<Vec<_>>();
    let mut backward_frontier = target_paths.iter().cloned().collect::<Vec<_>>();
    let mut forward_parent = HashMap::<String, String>::new();
    let mut backward_next = HashMap::<String, String>::new();
    let mut forward_visited = source_paths;
    let mut backward_visited = target_paths;
    let mut meeting = None::<String>;
    let mut adjacency_cache = HashMap::<String, Vec<String>>::new();
    while !forward_frontier.is_empty() && !backward_frontier.is_empty() {
        let forward_cost =
            callpath_dependency_frontier_cost(index, &forward_frontier, &mut adjacency_cache);
        let backward_cost =
            callpath_dependency_frontier_cost(index, &backward_frontier, &mut adjacency_cache);
        let expand_forward = forward_cost <= backward_cost;
        if expand_forward {
            let mut next = Vec::new();
            for current in std::mem::take(&mut forward_frontier) {
                for neighbor in callpath_dependency_neighbors(index, &current, &mut adjacency_cache)
                {
                    if !forward_visited.insert(neighbor.clone()) {
                        continue;
                    }
                    forward_parent.insert(neighbor.clone(), current.clone());
                    if backward_visited.contains(neighbor) {
                        meeting = Some(neighbor.clone());
                        break;
                    }
                    next.push(neighbor.clone());
                }
                if meeting.is_some() {
                    break;
                }
            }
            forward_frontier = next;
        } else {
            let mut next = Vec::new();
            for current in std::mem::take(&mut backward_frontier) {
                for neighbor in callpath_dependency_neighbors(index, &current, &mut adjacency_cache)
                {
                    if !backward_visited.insert(neighbor.clone()) {
                        continue;
                    }
                    backward_next.insert(neighbor.clone(), current.clone());
                    if forward_visited.contains(neighbor) {
                        meeting = Some(neighbor.clone());
                        break;
                    }
                    next.push(neighbor.clone());
                }
                if meeting.is_some() {
                    break;
                }
            }
            backward_frontier = next;
        }
        if meeting.is_some() {
            break;
        }
    }
    let meeting = meeting?;

    let mut route = BTreeSet::<String>::new();
    let mut current = meeting.clone();
    route.insert(current.clone());
    while let Some(previous) = forward_parent.get(&current) {
        route.insert(previous.clone());
        current = previous.clone();
    }
    current = meeting;
    while let Some(next) = backward_next.get(&current) {
        route.insert(next.clone());
        current = next.clone();
    }
    let mut frontier = route.iter().cloned().collect::<Vec<_>>();
    for _ in 0..neighbor_depth {
        let mut next = Vec::new();
        for current in frontier {
            for neighbor in callpath_dependency_neighbors(index, &current, &mut adjacency_cache) {
                if route.insert(neighbor.clone()) {
                    next.push(neighbor.clone());
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Some(route)
}

fn callpath_dependency_frontier_cost(
    index: &Codebase,
    frontier: &[String],
    adjacency_cache: &mut HashMap<String, Vec<String>>,
) -> usize {
    frontier.iter().fold(0usize, |cost, path| {
        cost.saturating_add(
            callpath_dependency_neighbors(index, path, adjacency_cache)
                .len()
                .max(1),
        )
    })
}

fn callpath_dependency_neighbors<'a>(
    index: &Codebase,
    path: &str,
    adjacency_cache: &'a mut HashMap<String, Vec<String>>,
) -> &'a [String] {
    adjacency_cache.entry(path.to_string()).or_insert_with(|| {
        let mut neighbors = index.deps_for(path).into_iter().collect::<BTreeSet<_>>();
        neighbors.extend(index.reverse_deps_for(path));
        neighbors.into_iter().collect()
    })
}

fn lazy_symbol_callpath_edges(
    index: &Codebase,
    target: &SymbolTarget,
    allowed_paths: Option<&BTreeSet<String>>,
    include_weak_references: bool,
    active_content_cache: &mut HashMap<String, Arc<String>>,
) -> Result<Vec<CallpathEdge>> {
    let Some(file) = index.file(&target.path) else {
        return Ok(Vec::new());
    };
    let Some(symbol) = symbol_for_target(file, target) else {
        return Ok(Vec::new());
    };
    let symbol_end = symbol.line_end.max(symbol.line_start);
    let active_content = if let Some(content) = active_content_cache.get(&file.path) {
        content.clone()
    } else {
        let content = index.file_content(file)?;
        let active = Arc::new(mask_comments(file.language.as_str(), &content));
        active_content_cache.insert(file.path.clone(), active.clone());
        active
    };
    let body = source_line_slice(&active_content, symbol.line_start, symbol_end);
    Ok(callpath_edges_from_body(
        index,
        file,
        symbol,
        &body,
        allowed_paths,
        include_weak_references,
    ))
}

fn callpath_edges_from_body(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    allowed_paths: Option<&BTreeSet<String>>,
    include_weak_references: bool,
) -> Vec<CallpathEdge> {
    let mut edges = BTreeMap::<String, CallpathEdge>::new();
    let receiver_types = qualified_receiver_type_hints(index, file, symbol, body);
    for lead in symbol_body_qualified_tail_call_leads(index, file, symbol, body, usize::MAX) {
        if allowed_paths.is_some_and(|paths| !paths.contains(&lead.target.path)) {
            continue;
        }
        let key = symbol_target_key(&lead.target);
        insert_callpath_edge(
            &mut edges,
            key,
            CallpathEdge {
                target: lead.target,
                relation: "qualified_call".to_string(),
                line: Some(lead.line),
                text: Some(lead.text),
            },
        );
    }
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    for (line_offset, line) in body.lines().enumerate() {
        let code_line = strip_strings_and_line_comment(line);
        for token in qualified_member_tokens(&code_line) {
            let receiver_type = qualified_token_receiver(&token)
                .and_then(|receiver| receiver_types.get(receiver))
                .map(String::as_str);
            let Some((target, _)) = resolve_qualified_call_target(
                index,
                file,
                symbol,
                &deps,
                &code_line,
                &token,
                false,
                receiver_type,
            ) else {
                continue;
            };
            if allowed_paths.is_some_and(|paths| !paths.contains(&target.path)) {
                continue;
            }
            let key = symbol_target_key(&target);
            insert_callpath_edge(
                &mut edges,
                key,
                CallpathEdge {
                    target,
                    relation: "qualified_member".to_string(),
                    line: Some(symbol.line_start + line_offset),
                    text: Some(line.trim().to_string()),
                },
            );
        }
    }
    for edge in structural_symbol_callpath_edges(
        index,
        file,
        symbol,
        body,
        allowed_paths,
        include_weak_references,
        &receiver_types,
    ) {
        let key = symbol_target_key(&edge.target);
        insert_callpath_edge(&mut edges, key, edge);
    }
    edges.into_values().collect()
}

fn insert_callpath_edge(
    edges: &mut BTreeMap<String, CallpathEdge>,
    key: String,
    edge: CallpathEdge,
) {
    edges.entry(key).or_insert(edge);
}

fn structural_symbol_callpath_edges(
    index: &Codebase,
    file: &FileEntry,
    symbol: &Symbol,
    body: &str,
    allowed_paths: Option<&BTreeSet<String>>,
    include_weak_references: bool,
    receiver_types: &BTreeMap<String, String>,
) -> Vec<CallpathEdge> {
    let deps = index
        .deps_for(&file.path)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut edges = BTreeMap::<String, CallpathEdge>::new();
    for (line_offset, line) in body.lines().enumerate() {
        let code_line = strip_strings_and_line_comment(line);
        let qualified_call_tokens = qualified_call_tokens(&code_line);
        let qualified_call_members = qualified_call_tokens
            .iter()
            .filter(|token| {
                let receiver_type = qualified_token_receiver(token)
                    .and_then(|receiver| receiver_types.get(receiver))
                    .map(String::as_str);
                resolve_qualified_call_target(
                    index,
                    file,
                    symbol,
                    &deps,
                    &code_line,
                    token,
                    true,
                    receiver_type,
                )
                .is_some()
            })
            .filter_map(|token| token.rsplit('.').next().map(ToString::to_string))
            .collect::<BTreeSet<_>>();
        let unresolved_static_call_members = qualified_call_tokens
            .iter()
            .filter(|token| {
                let receiver_type = qualified_token_receiver(token)
                    .and_then(|receiver| receiver_types.get(receiver))
                    .map(String::as_str);
                resolve_qualified_call_target(
                    index,
                    file,
                    symbol,
                    &deps,
                    &code_line,
                    token,
                    true,
                    receiver_type,
                )
                .is_none()
                    && !qualified_token_has_graph_receiver(file, symbol, token)
            })
            .filter_map(|token| token.rsplit('.').next().map(ToString::to_string))
            .collect::<BTreeSet<_>>();
        let qualified_value_members = qualified_member_tokens(&code_line)
            .into_iter()
            .filter_map(|token| token.rsplit('.').next().map(ToString::to_string))
            .collect::<BTreeSet<_>>();
        for token in qualified_member_tokens(&code_line) {
            if qualified_call_tokens.iter().any(|call| call == &token) {
                continue;
            }
            let Some(member) = token.rsplit('.').next() else {
                continue;
            };
            let mut candidates = index
                .symbols_named(member)
                .into_iter()
                .filter(|(candidate_file, candidate_symbol)| {
                    is_context_handoff_source_symbol(candidate_symbol)
                        && allowed_paths.is_none_or(|paths| paths.contains(&candidate_file.path))
                        && qualified_call_qualifier_matches(
                            index,
                            &token,
                            candidate_file,
                            candidate_symbol,
                        ) > 0
                })
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| {
                qualified_call_qualifier_matches(index, &token, right.0, right.1)
                    .cmp(&qualified_call_qualifier_matches(
                        index, &token, left.0, left.1,
                    ))
                    .then_with(|| left.0.path.cmp(&right.0.path))
                    .then_with(|| left.1.line_start.cmp(&right.1.line_start))
            });
            let Some((candidate_file, candidate_symbol)) = candidates.first().copied() else {
                continue;
            };
            let best =
                qualified_call_qualifier_matches(index, &token, candidate_file, candidate_symbol);
            if candidates.get(1).is_some_and(|(file, symbol)| {
                qualified_call_qualifier_matches(index, &token, file, symbol) == best
            }) {
                continue;
            }
            let target = target_from_symbol(candidate_file, candidate_symbol);
            edges
                .entry(symbol_target_key(&target))
                .or_insert_with(|| CallpathEdge {
                    target,
                    relation: "member_reference".to_string(),
                    line: Some(symbol.line_start + line_offset),
                    text: Some(line.trim().to_string()),
                });
        }
        let mut seen_names = BTreeSet::<String>::new();
        for identifier in raw_identifiers(&code_line) {
            if (line_offset == 0 && identifier == symbol.name)
                || !seen_names.insert(identifier.clone())
            {
                continue;
            }
            let call_arities = identifier_call_argument_counts(&code_line, &identifier);
            let direct_call = !call_arities.is_empty();
            let (unqualified_call, qualified_call) =
                identifier_call_receiver_kinds(&code_line, &identifier);
            let member_reference = identifier_has_member_receiver(&code_line, &identifier);
            if direct_call
                && qualified_call
                && !unqualified_call
                && qualified_call_members.contains(&identifier)
            {
                continue;
            }
            if direct_call
                && qualified_call
                && !unqualified_call
                && unresolved_static_call_members.contains(&identifier)
            {
                continue;
            }
            if !direct_call && member_reference && qualified_value_members.contains(&identifier) {
                continue;
            }
            if !direct_call && !member_reference && !include_weak_references {
                continue;
            }
            let mut candidates = index
                .symbols_named(&identifier)
                .into_iter()
                .filter(|(candidate_file, candidate_symbol)| {
                    is_context_handoff_source_symbol(candidate_symbol)
                        && allowed_paths.is_none_or(|paths| paths.contains(&candidate_file.path))
                        && (candidate_file.path != file.path
                            || candidate_symbol.line_start != symbol.line_start
                            || candidate_symbol.name != symbol.name)
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                continue;
            }

            if direct_call {
                let arity_matches = candidates
                    .iter()
                    .copied()
                    .filter(|(_, candidate_symbol)| {
                        call_arities.iter().any(|arity| {
                            signature_accepts_argument_count(&candidate_symbol.detail, *arity)
                        })
                    })
                    .collect::<Vec<_>>();
                if !arity_matches.is_empty() {
                    candidates = arity_matches;
                }
            }
            if direct_call && unqualified_call {
                let same_file = candidates
                    .iter()
                    .copied()
                    .filter(|(candidate_file, _)| candidate_file.path == file.path)
                    .collect::<Vec<_>>();
                if !same_file.is_empty() {
                    candidates = same_file;
                }
            }

            for (candidate_file, candidate_symbol) in candidates {
                let target = target_from_symbol(candidate_file, candidate_symbol);
                let key = symbol_target_key(&target);
                edges.entry(key).or_insert_with(|| CallpathEdge {
                    target,
                    relation: if direct_call {
                        "direct_call".to_string()
                    } else if member_reference {
                        "member_reference".to_string()
                    } else {
                        "symbol_reference".to_string()
                    },
                    line: Some(symbol.line_start + line_offset),
                    text: Some(line.trim().to_string()),
                });
            }
        }
    }
    edges.into_values().collect()
}

fn identifier_has_member_receiver(line: &str, identifier: &str) -> bool {
    line.contains(&format!(".{identifier}")) || line.contains(&format!("::{identifier}"))
}

fn identifier_call_receiver_kinds(line: &str, identifier: &str) -> (bool, bool) {
    let mut unqualified = false;
    let mut qualified = false;
    let mut from = 0usize;
    while let Some(relative) = line.get(from..).and_then(|tail| tail.find(identifier)) {
        let start = from + relative;
        let end = start + identifier.len();
        let before_ok = line
            .get(..start)
            .and_then(|prefix| prefix.chars().next_back())
            .is_none_or(|ch| !is_identifier_char(ch));
        let after_ok = line
            .get(end..)
            .and_then(|suffix| suffix.chars().next())
            .is_none_or(|ch| !is_identifier_char(ch));
        let is_call = line
            .get(end..)
            .map(str::trim_start)
            .is_some_and(|suffix| suffix.starts_with('('));
        if before_ok && after_ok && is_call {
            let prefix = line.get(..start).unwrap_or_default().trim_end();
            if prefix.ends_with('.') || prefix.ends_with("::") {
                qualified = true;
            } else {
                unqualified = true;
            }
        }
        from = end.max(from + 1);
    }
    (unqualified, qualified)
}

fn cached_callpath_edges(
    index: &Codebase,
    key: &str,
    target: &SymbolTarget,
    allowed_paths: Option<&BTreeSet<String>>,
    include_weak_references: bool,
    active_content_cache: &mut HashMap<String, Arc<String>>,
    cache: &mut HashMap<String, Vec<CallpathEdge>>,
) -> Result<Vec<CallpathEdge>> {
    if let Some(edges) = cache.get(key) {
        return Ok(edges.clone());
    }
    let edges = lazy_symbol_callpath_edges(
        index,
        target,
        allowed_paths,
        include_weak_references,
        active_content_cache,
    )?;
    let edges = precise_callpath_edges(target, edges);
    cache.insert(key.to_string(), edges.clone());
    Ok(edges)
}

fn precise_callpath_edges(source: &SymbolTarget, edges: Vec<CallpathEdge>) -> Vec<CallpathEdge> {
    let mut precise = Vec::new();
    let mut direct_groups = BTreeMap::<(Option<usize>, String, String), Vec<CallpathEdge>>::new();
    for edge in edges {
        if edge.relation != "direct_call" {
            precise.push(edge);
            continue;
        }
        direct_groups
            .entry((
                edge.line,
                edge.target.name.clone(),
                edge.text.clone().unwrap_or_default(),
            ))
            .or_default()
            .push(edge);
    }
    for ((_, name, text), mut candidates) in direct_groups {
        let (unqualified, qualified) = identifier_call_receiver_kinds(&text, &name);
        if qualified && !unqualified {
            continue;
        }
        candidates.sort_by(|left, right| {
            symbol_target_key(&left.target).cmp(&symbol_target_key(&right.target))
        });
        candidates.dedup_by(|left, right| {
            symbol_target_key(&left.target) == symbol_target_key(&right.target)
        });
        if candidates.len() == 1 {
            precise.push(candidates.remove(0));
            continue;
        }
        let mut same_file = candidates
            .into_iter()
            .filter(|candidate| candidate.target.path == source.path)
            .collect::<Vec<_>>();
        if same_file.len() == 1 {
            precise.push(same_file.remove(0));
        }
    }
    precise.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.relation.cmp(&right.relation))
            .then_with(|| symbol_target_key(&left.target).cmp(&symbol_target_key(&right.target)))
    });
    precise
}

fn cached_callpath_incoming_edges(
    index: &Codebase,
    key: &str,
    target: &SymbolTarget,
    allowed_paths: Option<&BTreeSet<String>>,
    include_weak_references: bool,
    active_content_cache: &mut HashMap<String, Arc<String>>,
    incoming_cache: &mut HashMap<String, Vec<CallpathEdge>>,
    outgoing_cache: &mut HashMap<String, Vec<CallpathEdge>>,
) -> Result<Vec<CallpathEdge>> {
    if let Some(edges) = incoming_cache.get(key) {
        return Ok(edges.clone());
    }
    let target_key = symbol_target_key(target);
    let mut callers = BTreeMap::<String, SymbolTarget>::new();
    for hit in reference_candidates(index, &target.name)? {
        let Some(scope) = hit.scope else {
            continue;
        };
        let Some(file) = index.file(&hit.path) else {
            continue;
        };
        if allowed_paths.is_some_and(|paths| !paths.contains(&file.path)) {
            continue;
        }
        let Some(symbol) = file.symbols.iter().find(|symbol| {
            symbol.line_start == scope.start
                && symbol.line_end == scope.end
                && symbol.name == scope.name
                && is_context_handoff_source_symbol(symbol)
        }) else {
            continue;
        };
        let caller = target_from_symbol(file, symbol);
        let caller_key = symbol_target_key(&caller);
        if caller_key != target_key {
            callers.entry(caller_key).or_insert(caller);
        }
    }

    let mut incoming = Vec::<CallpathEdge>::new();
    for (caller_key, caller) in callers {
        let outgoing = cached_callpath_edges(
            index,
            &caller_key,
            &caller,
            allowed_paths,
            include_weak_references,
            active_content_cache,
            outgoing_cache,
        )?;
        if let Some(edge) = outgoing
            .into_iter()
            .find(|edge| symbol_target_key(&edge.target) == target_key)
        {
            incoming.push(CallpathEdge {
                target: caller,
                relation: edge.relation,
                line: edge.line,
                text: edge.text,
            });
        }
    }
    incoming.sort_by(|left, right| {
        symbol_target_key(&left.target).cmp(&symbol_target_key(&right.target))
    });
    incoming_cache.insert(key.to_string(), incoming.clone());
    Ok(incoming)
}

fn update_callpath_meeting(
    best: &mut Option<(usize, String)>,
    hops: usize,
    key: &str,
    max_depth: usize,
) {
    if hops > max_depth {
        return;
    }
    match best {
        Some((best_hops, best_key))
            if *best_hops < hops || (*best_hops == hops && best_key.as_str() <= key) => {}
        _ => *best = Some((hops, key.to_string())),
    }
}

fn reconstruct_bidirectional_callpath(
    meeting: &str,
    forward_parent: &HashMap<String, CallpathParent>,
    backward_next: &HashMap<String, CallpathNext>,
) -> Vec<(String, Option<String>)> {
    let mut forward_keys = vec![meeting.to_string()];
    let mut current = meeting.to_string();
    while let Some(parent) = forward_parent.get(&current) {
        current = parent.previous.clone();
        forward_keys.push(current.clone());
    }
    forward_keys.reverse();

    let mut path = forward_keys
        .into_iter()
        .map(|key| {
            let relation = forward_parent
                .get(&key)
                .map(|parent| parent.relation.clone());
            (key, relation)
        })
        .collect::<Vec<_>>();
    current = meeting.to_string();
    while let Some(next) = backward_next.get(&current) {
        path.push((next.next.clone(), Some(next.relation.clone())));
        current = next.next.clone();
    }
    path
}

fn callpath_symbol_candidates(
    index: &Codebase,
    term: &str,
    path: Option<&str>,
    line: Option<usize>,
    limit: usize,
) -> Vec<SymbolTarget> {
    let names = callpath_symbol_query_names(term);
    if names.is_empty() {
        return Vec::new();
    }
    let mut candidates = Vec::<SymbolTarget>::new();
    let mut seen = BTreeSet::<(String, usize, String)>::new();
    for name in &names {
        let mut matches = if let Some(path) = path {
            index
                .file(path)
                .map(|file| {
                    file.symbols
                        .iter()
                        .filter(|symbol| symbol.name == *name)
                        .filter(|symbol| {
                            line.is_none_or(|line| {
                                symbol.line_start == line
                                    || (symbol.line_start <= line && line <= symbol.line_end)
                            })
                        })
                        .map(|symbol| target_from_symbol(file, symbol))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            index
                .symbols_named(name)
                .into_iter()
                .map(|(file, symbol)| target_from_symbol(file, symbol))
                .collect::<Vec<_>>()
        };
        if matches.is_empty() && path.is_some() {
            matches = index
                .symbols_named(name)
                .into_iter()
                .map(|(file, symbol)| target_from_symbol(file, symbol))
                .collect::<Vec<_>>();
        }
        for candidate in matches {
            if seen.insert((
                candidate.path.clone(),
                candidate.line_start,
                candidate.name.clone(),
            )) {
                candidates.push(candidate);
            }
        }
    }
    candidates.sort_by(|left, right| {
        callpath_endpoint_score(right, &names, path, line)
            .cmp(&callpath_endpoint_score(left, &names, path, line))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line_start.cmp(&right.line_start))
    });
    candidates.truncate(limit);
    candidates
}

fn callpath_symbol_query_names(term: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    let leaf = callpath_leaf_name(term);
    for value in [
        term.trim(),
        term.trim().trim_end_matches("()"),
        leaf.as_str(),
    ] {
        let name = value.trim();
        if name.is_empty() || !name.chars().any(|ch| ch.is_ascii_alphabetic() || ch == '_') {
            continue;
        }
        if seen.insert(name.to_string()) {
            names.push(name.to_string());
        }
    }
    names
}

fn callpath_leaf_name(term: &str) -> String {
    term.trim()
        .trim_end_matches("()")
        .rsplit(|ch| matches!(ch, '.' | ':' | '#' | '/' | '\\'))
        .find(|part| !part.is_empty())
        .unwrap_or(term)
        .to_string()
}

fn callpath_endpoint_score(
    target: &SymbolTarget,
    names: &[String],
    path: Option<&str>,
    line: Option<usize>,
) -> usize {
    let mut score = symbol_kind_lead_weight_from_kind(&target.kind)
        + symbol_name_specificity_weight_from_name(&target.name);
    if names.iter().any(|name| target.name == *name) {
        score += 400;
    }
    if let Some(path) = path
        && target.path == path
    {
        score += 200;
    }
    if let Some(line) = line
        && target.line_start == line
    {
        score += 120;
    }
    score
}

fn format_lazy_symbol_callpath(
    index: &Codebase,
    found: bool,
    from: &str,
    to: &str,
    path_keys: &[(String, Option<String>)],
    nodes: &HashMap<String, SymbolTarget>,
    source_candidate_count: usize,
    target_candidate_count: usize,
    expanded_forward: usize,
    expanded_backward: usize,
    message: Option<&str>,
) -> Value {
    let mut path = Vec::new();
    if found {
        for (key, relation) in path_keys {
            if let Some(target) = nodes.get(key) {
                path.push(callpath_step_json(index, target, relation.as_deref()));
            }
        }
    }

    let source = path
        .first()
        .and_then(|step| step.get("id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let target = path
        .last()
        .and_then(|step| step.get("id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    json!({
        "found": found,
        "mode": "bidirectional_symbol_graph",
        "from": from,
        "to": to,
        "source": source,
        "target": target,
        "hops": path.len().saturating_sub(1),
        "path": path,
        "expanded": expanded_forward.saturating_add(expanded_backward),
        "expanded_by_direction": {
            "forward": expanded_forward,
            "backward": expanded_backward
        },
        "candidate_counts": {
            "source": source_candidate_count,
            "target": target_candidate_count
        },
        "message": message
    })
}

fn callpath_step_json(index: &Codebase, target: &SymbolTarget, relation: Option<&str>) -> Value {
    let active_body = index.file(&target.path).and_then(|file| {
        let symbol = symbol_for_target(file, target)?;
        let content = index.file_content(file).ok()?;
        let active_content = mask_comments(file.language.as_str(), &content);
        Some(extract_lines(
            &active_content,
            symbol.line_start,
            symbol.line_end.max(symbol.line_start),
            true,
        ))
    });
    json!({
        "id": symbol_target_key(target),
        "label": target.name,
        "type": target.kind,
        "path": target.path,
        "line": target.line_start,
        "detail": target.detail,
        "active_body": active_body,
        "via_relation": relation,
        "via_direction": relation.map(|_| "outgoing")
    })
}

fn symbol_target_key(target: &SymbolTarget) -> String {
    format!(
        "symbol:{}:{}:{}",
        target.path, target.line_start, target.name
    )
}

fn handle_diagnostics(args: &Value) -> Result<String> {
    let path = required_str(args, "path")?;
    Ok(format!("no diagnostics available yet for {path}"))
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

fn handle_module_atlas(index: &Codebase, args: &Value) -> Result<String> {
    let started = Instant::now();
    let limit = get_usize(args, "limit").unwrap_or(5000).clamp(1, 5000);
    let min_files = get_usize(args, "min_files").unwrap_or(2).clamp(1, 1000);
    let include_files = get_bool(args, "include_files");
    let split_files = get_bool(args, "split_files");
    let path_prefix = get_str(args, "path_prefix")
        .and_then(|prefix| (!prefix.trim().is_empty()).then(|| normalize_dir_prefix(&prefix)));
    let raws = build_file_module_raws_for_atlas(index, min_files, path_prefix.as_deref());
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
    let mut path_to_point_id = HashMap::<&str, usize>::new();
    for module in &modules {
        for path in &module.files {
            if index.files.contains_key(path) && !path_to_point_id.contains_key(path.as_str()) {
                let id = path_to_point_id.len();
                path_to_point_id.insert(path.as_str(), id);
            }
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
        "algorithm": "dependency-connected file graph + label propagation",
        "generationMs": started.elapsed().as_millis() as u64,
        "projection": "dependency-aware organic module layout + dependency-aware intra-module file layout; rendered by the graph atlas view",
    });
    if split_files {
        metadata["pointsPath"] = Value::String("module-atlas-points.json".to_string());
    }

    let mut data = json!({
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
        "points": if split_files {
            Value::Null
        } else {
            Value::Array(build_module_atlas_points_values(
                index,
                &modules,
                &layouts,
                &path_to_point_id,
            ))
        },
    });
    if let Some(output_path) = get_str(args, "output_path") {
        let output_path = resolve_output_path(&index.root, &output_path);
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        if split_files {
            let points_path = output_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("module-atlas-points.json");
            write_module_atlas_points(&points_path, index, &modules, &layouts, &path_to_point_id)?;
        }
        data["metadata"]["generationMs"] = json!(started.elapsed().as_millis() as u64);
        let file = File::create(&output_path)?;
        serde_json::to_writer(BufWriter::new(file), &data)?;
        Ok(format!(
            "exported module atlas to {}",
            output_path.display()
        ))
    } else {
        data["metadata"]["generationMs"] = json!(started.elapsed().as_millis() as u64);
        serde_json::to_string(&data).map_err(Into::into)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModuleAtlasPoint<'a> {
    id: usize,
    path: &'a str,
    language: crate::types::LanguageId,
    language_label: &'static str,
    module_id: usize,
    module_label: &'a str,
    x: f32,
    y: f32,
    category: usize,
    symbols: Vec<&'a str>,
    line_count: usize,
    dep_in: usize,
    dep_out: usize,
    dep_in_ids: Vec<usize>,
    dep_out_ids: Vec<usize>,
}

struct ModuleAtlasDeps<'a> {
    forward: parking_lot::RwLockReadGuard<'a, Option<HashMap<String, Vec<String>>>>,
    reverse: parking_lot::RwLockReadGuard<'a, Option<HashMap<String, Vec<String>>>>,
}

impl<'a> ModuleAtlasDeps<'a> {
    fn outgoing(&self, path: &str) -> Option<&[String]> {
        self.forward
            .as_ref()
            .and_then(|deps| deps.get(path).map(Vec::as_slice))
    }

    fn incoming(&self, path: &str) -> Option<&[String]> {
        self.reverse
            .as_ref()
            .and_then(|deps| deps.get(path).map(Vec::as_slice))
    }
}

fn module_atlas_deps(index: &Codebase) -> ModuleAtlasDeps<'_> {
    let _ = index.deps_for("");
    let _ = index.reverse_deps_for("");
    ModuleAtlasDeps {
        forward: index.deps_forward.read(),
        reverse: index.deps_reverse.read(),
    }
}

fn build_module_atlas_points_values(
    index: &Codebase,
    modules: &[ModuleAtlasModule],
    layouts: &HashMap<usize, ModuleLayout>,
    path_to_point_id: &HashMap<&str, usize>,
) -> Vec<Value> {
    let mut points = Vec::new();
    for module in modules {
        let layout = layouts
            .get(&module.community_id)
            .copied()
            .unwrap_or(ModuleLayout {
                x: 0.0,
                y: 0.0,
                radius: module_layout_radius(module.file_count),
            });
        let local_offsets = module_file_offsets(index, module, layout.radius);
        let deps = module_atlas_deps(index);
        for path in &module.files {
            let Some(file) = index.files.get(path) else {
                continue;
            };
            let point_id = path_to_point_id
                .get(path.as_str())
                .copied()
                .unwrap_or(points.len());
            let local = local_offsets.get(path).copied().unwrap_or((0.0, 0.0));
            let dep_in = deps.incoming(path);
            let dep_out = deps.outgoing(path);
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
                "depIn": dep_in.map_or(0, |items| items.len()),
                "depOut": dep_out.map_or(0, |items| items.len()),
                "depInIds": atlas_dependency_ids(dep_in, path_to_point_id, 80),
                "depOutIds": atlas_dependency_ids(dep_out, path_to_point_id, 80),
            }));
        }
    }
    points
}

fn write_module_atlas_points(
    path: &Path,
    index: &Codebase,
    modules: &[ModuleAtlasModule],
    layouts: &HashMap<usize, ModuleLayout>,
    path_to_point_id: &HashMap<&str, usize>,
) -> Result<()> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    let mut serializer = serde_json::Serializer::new(writer);
    let mut sequence = serializer.serialize_seq(None)?;
    for module in modules {
        let layout = layouts
            .get(&module.community_id)
            .copied()
            .unwrap_or(ModuleLayout {
                x: 0.0,
                y: 0.0,
                radius: module_layout_radius(module.file_count),
            });
        let local_offsets = module_file_offsets(index, module, layout.radius);
        let deps = module_atlas_deps(index);
        for path in &module.files {
            let Some(file) = index.files.get(path) else {
                continue;
            };
            let Some(point_id) = path_to_point_id.get(path.as_str()).copied() else {
                continue;
            };
            let local = local_offsets.get(path).copied().unwrap_or((0.0, 0.0));
            let dep_in = deps.incoming(path);
            let dep_out = deps.outgoing(path);
            let point = ModuleAtlasPoint {
                id: point_id,
                path,
                language: file.language,
                language_label: language_label(file.language.as_str()),
                module_id: module.community_id,
                module_label: &module.label,
                x: layout.x + local.0,
                y: layout.y + local.1,
                category: module.community_id % 12,
                symbols: file
                    .symbols
                    .iter()
                    .take(12)
                    .map(|symbol| symbol.name.as_str())
                    .collect(),
                line_count: file.line_count,
                dep_in: dep_in.map_or(0, |items| items.len()),
                dep_out: dep_out.map_or(0, |items| items.len()),
                dep_in_ids: atlas_dependency_ids(dep_in, path_to_point_id, 80),
                dep_out_ids: atlas_dependency_ids(dep_out, path_to_point_id, 80),
            };
            sequence.serialize_element(&point)?;
        }
    }
    sequence.end()?;
    Ok(())
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

fn build_file_module_raws_for_atlas(
    index: &Codebase,
    min_files: usize,
    path_prefix: Option<&str>,
) -> Vec<ModuleRaw> {
    build_file_module_raws_inner(index, min_files, path_prefix, true)
}

fn build_file_module_raws_inner(
    index: &Codebase,
    min_files: usize,
    path_prefix: Option<&str>,
    group_small_modules: bool,
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
    let mut raws = Vec::new();
    let mut small_groups = BTreeMap::<String, Vec<String>>::new();
    for files in communities {
        if files.len() < min_files {
            if group_small_modules {
                for file in files {
                    small_groups
                        .entry(module_path_root(&file))
                        .or_default()
                        .push(file);
                }
            }
            continue;
        }
        let community_id = raws.len();
        raws.push(module_raw_from_files(index, community_id, files));
    }
    if group_small_modules {
        for (_, files) in small_groups {
            let community_id = raws.len();
            raws.push(module_raw_from_files(index, community_id, files));
        }
    }
    raws
}

fn module_raw_from_files(index: &Codebase, community_id: usize, files: Vec<String>) -> ModuleRaw {
    let file_set = files.iter().cloned().collect::<BTreeSet<_>>();
    let token_counts = module_token_counts(index, &files);
    let symbol_count = files
        .iter()
        .filter_map(|path| index.files.get(path))
        .map(|file| file.symbols.len())
        .sum();
    let (internal_deps, outgoing_deps, incoming_deps) = module_dependency_counts(index, &file_set);
    ModuleRaw {
        community_id,
        fallback_label: module_label_from_files(index, &files),
        files,
        token_counts,
        symbol_count,
        internal_deps,
        outgoing_deps,
        incoming_deps,
    }
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
    let _ = name;
    match kind {
        "class" | "interface" | "struct" | "enum" | "record" => 8,
        "method" | "constructor" | "function" => 4,
        "property" | "field" => 2,
        _ => 1,
    }
}

fn entry_point_score(kind: &str, name: &str, path: &str) -> usize {
    let _ = (name, path);
    let mut score = 0usize;
    if matches!(
        kind,
        "class" | "interface" | "struct" | "record" | "method" | "function" | "constructor"
    ) {
        score += 4;
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
    paths: Option<&[String]>,
    path_to_point_id: &HashMap<&str, usize>,
    limit: usize,
) -> Vec<usize> {
    let Some(paths) = paths else {
        return Vec::new();
    };
    let mut items = paths
        .iter()
        .filter_map(|path| path_to_point_id.get(path.as_str()).copied())
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

    for _ in 0..72 {
        let mut delta = vec![(0.0f32, 0.0f32); items.len()];
        let mut grid = HashMap::<(i32, i32), Vec<usize>>::new();
        for i in 0..items.len() {
            let cell = (
                (items[i].x / 48.0).floor() as i32,
                (items[i].y / 48.0).floor() as i32,
            );
            grid.entry(cell).or_default().push(i);
        }
        for i in 0..items.len() {
            let cell = (
                (items[i].x / 48.0).floor() as i32,
                (items[i].y / 48.0).floor() as i32,
            );
            for gx in (cell.0 - 1)..=(cell.0 + 1) {
                for gy in (cell.1 - 1)..=(cell.1 + 1) {
                    let Some(neighbors) = grid.get(&(gx, gy)) else {
                        continue;
                    };
                    for &j in neighbors {
                        if j <= i {
                            continue;
                        }
                        let dx = items[j].x - items[i].x;
                        let dy = items[j].y - items[i].y;
                        let distance_sq = (dx * dx + dy * dy).max(0.04);
                        let distance = distance_sq.sqrt();
                        let min_distance = items[i].radius + items[j].radius + 8.0;
                        if distance >= min_distance {
                            continue;
                        }
                        let force = (min_distance - distance) * 0.18;
                        let nx = dx / distance;
                        let ny = dy / distance;
                        delta[i].0 -= nx * force;
                        delta[i].1 -= ny * force;
                        delta[j].0 += nx * force;
                        delta[j].1 += ny * force;
                    }
                }
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
    for _ in 0..16 {
        let mut moved = false;
        let mut grid = HashMap::<(i32, i32), Vec<usize>>::new();
        for i in 0..items.len() {
            let cell = (
                (items[i].x / 48.0).floor() as i32,
                (items[i].y / 48.0).floor() as i32,
            );
            grid.entry(cell).or_default().push(i);
        }
        for i in 0..items.len() {
            let cell = (
                (items[i].x / 48.0).floor() as i32,
                (items[i].y / 48.0).floor() as i32,
            );
            for gx in (cell.0 - 1)..=(cell.0 + 1) {
                for gy in (cell.1 - 1)..=(cell.1 + 1) {
                    let Some(neighbors) = grid.get(&(gx, gy)) else {
                        continue;
                    };
                    for &j in neighbors {
                        if j <= i {
                            continue;
                        }
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
    let cell_size = (radius * 0.09).max(0.75);
    for _ in 0..36 {
        let mut delta = vec![(0.0f32, 0.0f32); items.len()];
        if items.len() <= 96 {
            for i in 0..items.len() {
                for j in (i + 1)..items.len() {
                    repel_file_layout_pair(items, &mut delta, radius, i, j);
                }
            }
        } else {
            let mut grid = HashMap::<(i32, i32), Vec<usize>>::new();
            for i in 0..items.len() {
                let cell = (
                    (items[i].x / cell_size).floor() as i32,
                    (items[i].y / cell_size).floor() as i32,
                );
                grid.entry(cell).or_default().push(i);
            }
            for i in 0..items.len() {
                let cell = (
                    (items[i].x / cell_size).floor() as i32,
                    (items[i].y / cell_size).floor() as i32,
                );
                for gx in (cell.0 - 1)..=(cell.0 + 1) {
                    for gy in (cell.1 - 1)..=(cell.1 + 1) {
                        let Some(neighbors) = grid.get(&(gx, gy)) else {
                            continue;
                        };
                        for &j in neighbors {
                            if j <= i {
                                continue;
                            }
                            repel_file_layout_pair(items, &mut delta, radius, i, j);
                        }
                    }
                }
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

fn repel_file_layout_pair(
    items: &[FileLayoutItem],
    delta: &mut [(f32, f32)],
    radius: f32,
    i: usize,
    j: usize,
) {
    let dx = items[j].x - items[i].x;
    let dy = items[j].y - items[i].y;
    let distance_sq = (dx * dx + dy * dy).max(0.0004);
    let distance = distance_sq.sqrt();
    let min_distance = items[i].radius + items[j].radius + radius * 0.018;
    if distance >= min_distance {
        return;
    }
    let force = (min_distance - distance) * 0.24;
    let nx = dx / distance;
    let ny = dy / distance;
    delta[i].0 -= nx * force;
    delta[i].1 -= ny * force;
    delta[j].0 += nx * force;
    delta[j].1 += ny * force;
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
    "base",
    "build",
    "cache",
    "code",
    "default",
    "file",
    "generated",
    "index",
    "main",
    "module",
    "modules",
    "object",
    "package",
    "packages",
    "test",
    "tests",
    "type",
    "types",
];

fn handle_index(manager: &ProjectManager, args: &Value) -> Result<String> {
    let path = required_str(args, "path")?;
    if is_agent_instruction_path(&path) {
        return Ok("error: codedb_index is for source projects, not agent skill or instruction directories".to_string());
    }
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

fn is_agent_instruction_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    path.contains("/.agents/skills/")
        || path.contains("/.codex/skills/")
        || path.ends_with("/.agents/skills")
        || path.ends_with("/.codex/skills")
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
    let mut sections = Vec::<String>::new();
    let mut evidence_index = Vec::<String>::new();
    for (idx, op) in ops.iter().enumerate() {
        let tool = get_str(op, "tool").unwrap_or_default();
        let mut section = String::new();
        if tool.is_empty() {
            let start = Instant::now();
            event_log::log_tool_failure(
                "<missing>",
                ToolLogContext::bundle(idx),
                start,
                "missing 'tool' field",
            );
            section.push_str(&format!(
                "--- [{idx}] <missing> ---\nerror: missing 'tool' field\n"
            ));
            sections.push(section);
            continue;
        }
        if tool == "codedb_bundle" {
            let start = Instant::now();
            event_log::log_tool_failure(
                &tool,
                ToolLogContext::bundle(idx),
                start,
                "codedb_bundle not allowed in bundle",
            );
            section.push_str(&format!(
                "--- [{idx}] {tool} ---\nerror: codedb_bundle not allowed in bundle\n"
            ));
            sections.push(section);
            continue;
        }
        let arguments = op.get("arguments").unwrap_or(op).clone();
        let start = Instant::now();
        let result =
            dispatch_tool_with_context(manager, &tool, &arguments, ToolLogContext::bundle(idx));
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        section.push_str(&format!("--- [{}] {} ---\n", idx, tool));
        if timing {
            section.push_str(&format!("time_ms: {:.3}\n", elapsed_ms));
        }
        if discard_output {
            let first_line = result.lines().next().unwrap_or_default();
            section.push_str(&format!("summary: {first_line}\n"));
        } else {
            if let Some(summary) = bundle_symbol_evidence_summary(idx, &tool, &arguments, &result) {
                evidence_index.push(summary);
            }
            section.push_str(&result);
            if !section.ends_with('\n') {
                section.push('\n');
            }
        }
        sections.push(section);
    }
    let mut out = format!("bundle {} ops\n", ops.len());
    if !evidence_index.is_empty() {
        out.push_str("bundle evidence index (complete details follow; consume exact next handoffs before search/find):\n");
        for summary in evidence_index {
            out.push_str(&summary);
        }
    }
    for section in sections {
        out.push_str(&section);
    }
    Ok(out)
}

fn bundle_symbol_evidence_summary(
    index: usize,
    tool: &str,
    arguments: &Value,
    result: &str,
) -> Option<String> {
    if tool != "codedb_symbol"
        || !arguments
            .get("body")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || result.starts_with("error:")
        || result.starts_with("no results")
    {
        return None;
    }
    let name = get_str(arguments, "name").unwrap_or_else(|| "<symbol>".to_string());
    let definition = result
        .lines()
        .find(|line| line.starts_with("  ") && line.contains(" score="))
        .map(|line| compact_inline_text(line.trim(), 240))
        .unwrap_or_else(|| name.to_string());
    let (qualified, flow) = bundle_symbol_handoff_summary_lines(result);
    let mut out = format!("  [{index}] {definition}\n");
    let selected = qualified
        .into_iter()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .chain(
            flow.into_iter()
                .rev()
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev(),
        )
        .collect::<Vec<_>>();
    if !selected.is_empty() {
        out.push_str("      next: ");
        out.push_str(&selected.join("; "));
        out.push('\n');
    }
    Some(out)
}

fn bundle_symbol_handoff_summary_lines(result: &str) -> (Vec<String>, Vec<String>) {
    #[derive(Clone, Copy)]
    enum Section {
        None,
        Qualified,
        Flow,
    }
    let mut section = Section::None;
    let mut qualified = Vec::new();
    let mut flow = Vec::new();
    for line in result.lines() {
        if line.starts_with("body qualified tail call leads") {
            section = Section::Qualified;
            continue;
        }
        if line.starts_with("body flow handoff leads") {
            section = Section::Flow;
            continue;
        }
        if !line.starts_with("  L") {
            if !line.starts_with("  ") {
                section = Section::None;
            }
            continue;
        }
        let compact = line
            .trim()
            .split_once(" //")
            .map(|(lead, _)| lead)
            .unwrap_or_else(|| line.trim())
            .to_string();
        let target = match section {
            Section::Qualified => &mut qualified,
            Section::Flow => &mut flow,
            Section::None => continue,
        };
        if !target.contains(&compact) {
            target.push(compact);
        }
    }
    (qualified, flow)
}

fn truncate_string(value: &mut String, max_bytes: usize) -> bool {
    if value.len() <= max_bytes {
        return false;
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    true
}

fn format_paths_only_line_hits(
    query: &str,
    hits: Vec<crate::types::SearchHit>,
    offset: usize,
) -> String {
    let mut out = format!("{} paths '{}' offset={offset}:\n", hits.len(), query);
    for hit in hits {
        out.push_str(&format!("{}:{}\n", hit.path, hit.line));
    }
    out
}

fn format_json_line_hits(
    query: &str,
    hits: Vec<crate::types::SearchHit>,
    regex: bool,
    offset: usize,
    results_clipped: bool,
) -> Result<String> {
    let results = hits
        .into_iter()
        .map(|hit| {
            json!({
                "path": hit.path,
                "line": hit.line,
                "text": hit.text,
                "scope": hit.scope.map(|scope| json!({
                    "name": scope.name,
                    "kind": scope.kind,
                    "line_start": scope.start,
                    "line_end": scope.end
                }))
            })
        })
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&json!({
        "ok": true,
        "tool": "codedb_search",
        "query": query,
        "mode": if regex { "regex" } else { "substring" },
        "offset": offset,
        "results_clipped": results_clipped,
        "max_json_results": SEARCH_JSON_MAX_RESULTS,
        "results": results
    }))
    .map_err(Into::into)
}

fn format_line_hits(query: &str, hits: Vec<crate::types::SearchHit>, compact: bool) -> String {
    const MAX_PER_FILE: usize = 5;

    struct FileHits {
        total: usize,
        shown: Vec<String>,
        lines: Vec<usize>,
        scopes: BTreeSet<String>,
        followups: BTreeSet<String>,
    }

    let total_hits = hits.len();
    let mut file_order = Vec::<String>::new();
    let mut grouped = BTreeMap::<String, FileHits>::new();

    for hit in hits {
        if !grouped.contains_key(&hit.path) {
            file_order.push(hit.path.clone());
            grouped.insert(
                hit.path.clone(),
                FileHits {
                    total: 0,
                    shown: Vec::new(),
                    lines: Vec::new(),
                    scopes: BTreeSet::new(),
                    followups: BTreeSet::new(),
                },
            );
        }
        let Some(entry) = grouped.get_mut(&hit.path) else {
            continue;
        };
        entry.total += 1;
        entry.lines.push(hit.line);
        if let Some(scope) = &hit.scope {
            entry.scopes.insert(format!(
                "{} {} L{}-L{}",
                scope.kind, scope.name, scope.start, scope.end
            ));
            entry.followups.insert(format!(
                "codedb_symbol name={} path={} body=true max_results=1",
                scope.name, hit.path
            ));
        }
        if compact {
            if entry.shown.len() < 2 {
                let text = compact_inline_text(&hit.text, 180);
                if let Some(scope) = &hit.scope {
                    entry.shown.push(format!(
                        "    L{}: {} [in {} {}, L{}-L{}]",
                        hit.line, text, scope.kind, scope.name, scope.start, scope.end
                    ));
                } else {
                    entry.shown.push(format!("    L{}: {}", hit.line, text));
                }
            }
            continue;
        }
        if entry.shown.len() >= MAX_PER_FILE {
            continue;
        }
        let line = if let Some(scope) = hit.scope {
            format!(
                "    L{}: {} [in {} {}, L{}-L{}]",
                hit.line, hit.text, scope.kind, scope.name, scope.start, scope.end
            )
        } else {
            format!("    L{}: {}", hit.line, hit.text)
        };
        entry.shown.push(line);
    }

    if compact {
        let mut out = format!(
            "{} compact '{}' files={}:\n",
            total_hits,
            query,
            file_order.len()
        );
        for path in file_order {
            let Some(entry) = grouped.get(&path) else {
                continue;
            };
            let lines = entry
                .lines
                .iter()
                .take(12)
                .map(|line| format!("L{line}"))
                .collect::<Vec<_>>()
                .join(", ");
            let more_lines = entry.total.saturating_sub(entry.lines.len().min(12));
            let suffix = if more_lines > 0 {
                format!(" (+{more_lines})")
            } else {
                String::new()
            };
            out.push_str(&format!("  {path}: {lines}{suffix}\n"));
            for line in &entry.shown {
                out.push_str(line);
                out.push('\n');
            }
            let scopes = entry.scopes.iter().take(3).cloned().collect::<Vec<_>>();
            if !scopes.is_empty() {
                out.push_str(&format!("    scopes:{}\n", scopes.join("; ")));
            }
            let followups = entry.followups.iter().take(2).cloned().collect::<Vec<_>>();
            if !followups.is_empty() {
                out.push_str(&format!("    followups:{}\n", followups.join("; ")));
            }
        }
        return out;
    }

    let mut out = format!(
        "{} results '{}' files={}:\n",
        total_hits,
        query,
        file_order.len()
    );
    let mut shown = 0usize;
    for path in file_order {
        let Some(entry) = grouped.get(&path) else {
            continue;
        };
        out.push_str(&format!("  {path}\n"));
        for line in &entry.shown {
            out.push_str(line);
            out.push('\n');
            shown += 1;
        }
        if entry.total > entry.shown.len() {
            out.push_str(&format!("    [+{}]\n", entry.total - entry.shown.len()));
        }
    }
    if shown < total_hits {
        out.push_str(&format!("[{shown} shown, {} hidden]\n", total_hits - shown));
    }
    out
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

fn extract_lines_limited(
    content: &str,
    start: usize,
    end: usize,
    compact: bool,
    max_lines: usize,
) -> (String, bool) {
    let mut out = String::new();
    let mut shown = 0usize;
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if line_no < start || line_no > end {
            continue;
        }
        if compact && is_comment_or_blank(line) {
            continue;
        }
        if shown >= max_lines {
            return (out, true);
        }
        out.push_str(&format!("{line_no}: {line}\n"));
        shown += 1;
    }
    (out, false)
}

fn source_line_slice(content: &str, start: usize, end: usize) -> String {
    let mut out = String::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if line_no < start || line_no > end {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn source_code_identifiers(source: &str) -> Vec<String> {
    source
        .lines()
        .flat_map(|line| raw_identifiers(&strip_strings_and_line_comment(line)))
        .collect()
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

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut memo = HashMap::<(usize, usize), bool>::new();
    wildcard_match_inner(pattern, text, 0, 0, &mut memo)
}

fn wildcard_match_inner(
    pattern: &[u8],
    text: &[u8],
    pattern_index: usize,
    text_index: usize,
    memo: &mut HashMap<(usize, usize), bool>,
) -> bool {
    if let Some(value) = memo.get(&(pattern_index, text_index)) {
        return *value;
    }
    let matched = if pattern_index == pattern.len() {
        text_index == text.len()
    } else if pattern[pattern_index] == b'*' {
        wildcard_match_inner(pattern, text, pattern_index + 1, text_index, memo)
            || (text_index < text.len()
                && wildcard_match_inner(pattern, text, pattern_index, text_index + 1, memo))
    } else {
        text_index < text.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == text[text_index])
            && wildcard_match_inner(pattern, text, pattern_index + 1, text_index + 1, memo)
    };
    memo.insert((pattern_index, text_index), matched);
    matched
}

fn fuzzy_score(path: &str, query: &str) -> Option<f32> {
    let path_lower = path.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if query_lower.is_empty() {
        return None;
    }
    let normalized_path = normalize_rel_path(path).to_ascii_lowercase();
    let normalized_query = normalize_rel_path(query).to_ascii_lowercase();
    if !normalized_query.is_empty() {
        let query_dir = normalize_dir_prefix(&normalized_query);
        if normalized_path == normalized_query {
            return Some(12_000.0);
        }
        if !query_dir.is_empty() && normalized_path.starts_with(&query_dir) {
            return Some(
                11_000.0 + normalized_query.len() as f32 / normalized_path.len().max(1) as f32,
            );
        }
        if normalized_path.contains(&normalized_query) {
            return Some(
                10_000.0 + normalized_query.len() as f32 / normalized_path.len().max(1) as f32,
            );
        }
    }
    if path_lower.contains(&query_lower) {
        return Some(9_000.0 + query_lower.len() as f32 / path_lower.len().max(1) as f32);
    }
    let compact_path = compact_fuzzy_text(&path_lower);
    let compact_query = compact_fuzzy_text(&query_lower);
    if !compact_query.is_empty() && compact_path.contains(&compact_query) {
        return Some(8_000.0 + compact_query.len() as f32 / compact_path.len().max(1) as f32);
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

fn get_bool_default(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn get_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key)?.as_u64().map(|n| n as usize)
}

fn get_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key)?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn project_name_alias_resolves_to_default_root() {
        let default_root = PathBuf::from(r"F:\workspace\main\Unicorn\u3dclient");
        assert_eq!(
            requested_project_path(&default_root, Some("u3dclient")),
            default_root
        );
    }

    #[test]
    fn nonmatching_project_name_remains_an_explicit_path() {
        let default_root = PathBuf::from(r"F:\workspace\main\Unicorn\u3dclient");
        assert_eq!(
            requested_project_path(&default_root, Some("another-project")),
            PathBuf::from("another-project")
        );
    }

    #[test]
    fn module_inventory_keeps_a_leaf_group_for_each_broad_graph_root() {
        let row = |prefix: &str, file_count: usize, score: f32| ContextModuleInventoryRow {
            prefix: prefix.to_string(),
            file_count,
            degree: file_count * 10,
            outgoing: file_count * 7,
            incoming: file_count * 3,
            representatives: vec![format!("{}.cs", prefix.rsplit('/').next().unwrap())],
            depth: path_component_count(prefix),
            score,
        };
        let mut rows = vec![
            row("Assets/Scripts/GameAOT/GameStart", 8, 1.0),
            row("Assets/Scripts/GameAOT/Logic", 12, 1.0),
        ];
        for index in 0..8 {
            rows.push(row(
                &format!("Packages/com.example.package{index}/Runtime"),
                100 + index,
                1_000.0 + index as f32,
            ));
            rows.push(row(
                &format!("Packages/com.example.package{index}/Editor"),
                80 + index,
                900.0 + index as f32,
            ));
        }
        let broad_rows = vec![row("Assets/Scripts/GameAOT", 20, 0.1)];

        let groups = select_context_module_inventory_leaf_groups(&rows, &broad_rows);

        assert_eq!(groups.first().unwrap().parent, "Assets/Scripts/GameAOT");
        assert!(
            groups[0]
                .children
                .iter()
                .any(|child| child.name == "GameStart")
        );
        assert!(!groups[0].children[0].representatives.is_empty());
    }

    #[test]
    fn module_inventory_orders_entry_oriented_children_before_shared_libraries() {
        let rows = vec![
            ContextModuleInventoryRow {
                prefix: "Assets/Scripts/App/Common".to_string(),
                file_count: 20,
                degree: 1_020,
                outgoing: 20,
                incoming: 1_000,
                representatives: vec!["CommonService.cs".to_string()],
                depth: 4,
                score: 100.0,
            },
            ContextModuleInventoryRow {
                prefix: "Assets/Scripts/App/Startup".to_string(),
                file_count: 10,
                degree: 210,
                outgoing: 200,
                incoming: 10,
                representatives: vec!["Launcher.cs".to_string()],
                depth: 4,
                score: 50.0,
            },
        ];

        let children = context_module_inventory_leaf_children_from_rows(&rows);

        assert_eq!(children[0].name, "Startup");
        assert_eq!(children[0].representatives, vec!["Launcher.cs"]);
    }

    #[test]
    fn body_exact_reference_terms_keep_member_shapes() {
        let terms = body_exact_reference_terms(
            "return AlphaService.BetaRunner.ExecuteNow(inputValue) && OutputMode.FastPath != mode;",
            4,
        );

        assert!(terms.contains(&"AlphaService.BetaRunner.ExecuteNow".to_string()));
        assert!(terms.contains(&"OutputMode.FastPath".to_string()));
        assert!(!terms.contains(&"inputValue".to_string()));
    }

    fn test_body_symbol_lead(name: &str, order: usize, score: usize) -> BodySymbolLead {
        BodySymbolLead {
            order,
            score,
            query_matches: BTreeSet::new(),
            target: SymbolTarget {
                name: name.to_string(),
                kind: "struct".to_string(),
                path: format!("src/{name}.rs"),
                line_start: order + 1,
                detail: String::new(),
            },
        }
    }

    #[test]
    fn data_type_lead_selection_reserves_ranked_capacity() {
        let mut source_ordered = (0..20)
            .map(|idx| test_body_symbol_lead(&format!("Ordered{idx}"), idx, 10))
            .collect::<Vec<_>>();
        source_ordered[10] = test_body_symbol_lead("HighScoreTarget", 10, 999);
        let mut ranked = source_ordered.clone();
        ranked.sort_by(|left, right| {
            right
                .score
                .cmp(&left.score)
                .then_with(|| left.order.cmp(&right.order))
                .then_with(|| left.target.path.cmp(&right.target.path))
                .then_with(|| left.target.line_start.cmp(&right.target.line_start))
        });

        let selected = select_balanced_body_data_type_leads(ranked, source_ordered, 8);

        assert!(
            selected
                .iter()
                .any(|lead| lead.target.name == "HighScoreTarget")
        );
        assert!(selected.len() <= 8);
    }

    #[test]
    fn candidate_symbols_prefer_direct_hit_scope() {
        let file = FileEntry {
            path: "src/flow.rs".to_string(),
            language: crate::types::LanguageId::Rust,
            line_count: 80,
            byte_size: 0,
            modified_unix_ms: 0,
            content_hash: String::new(),
            namespace: None,
            imports: Vec::new(),
            symbols: vec![
                Symbol {
                    name: "OuterFlow".to_string(),
                    kind: SymbolKind::Module,
                    line_start: 1,
                    line_end: 80,
                    detail: String::new(),
                },
                Symbol {
                    name: "SharedOperationMode".to_string(),
                    kind: SymbolKind::Enum,
                    line_start: 20,
                    line_end: 28,
                    detail: String::new(),
                },
                Symbol {
                    name: "UnrelatedEntry".to_string(),
                    kind: SymbolKind::Function,
                    line_start: 40,
                    line_end: 48,
                    detail: String::new(),
                },
            ],
            content: String::new(),
        };
        let mut candidate = ContextCandidate::new(file.path.clone());
        candidate
            .reasons
            .insert("exact text for 状态切换".to_string());
        candidate.hit_lines.insert(23);
        candidate.ranges.push(ContextRange { start: 23, end: 23 });

        let selected = selected_candidate_symbols(&file, &candidate, "状态切换逻辑", 2);

        assert_eq!(
            selected.first().map(|symbol| symbol.name.as_str()),
            Some("SharedOperationMode")
        );
        assert!(selected.iter().any(|symbol| symbol.name == "OuterFlow"));
    }

    #[test]
    fn data_type_usage_context_scores_constructor_like_occurrences() {
        let scores = body_data_type_usage_context_scores(
            "let alpha = BuilderType(input);\nNamespace.MemberType.call();\nnode { ShapeType: value }\nlet beta = MultilineType\n{ value: 1 }",
        );

        assert!(scores.get("BuilderType").copied().unwrap_or_default() > 0);
        assert_eq!(scores.get("MemberType").copied().unwrap_or_default(), 0);
        assert!(scores.get("ShapeType").copied().unwrap_or_default() > 0);
        assert!(scores.get("MultilineType").copied().unwrap_or_default() > 0);
    }

    #[test]
    fn data_type_leads_skip_weak_member_write_names() {
        let root = temp_tools_test_dir("data_type_leads_skip_weak_members");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public void Producer(SourceState source) {
        var carrier = new CarrierState {
            Value00 = source.Value00,
            Value01 = source.Value01,
            Value02 = source.Value02,
            Value03 = source.Value03,
            Value04 = source.Value04,
            Value05 = source.Value05,
            Value06 = source.Value06,
            Value07 = source.Value07,
            Value08 = source.Value08,
            Value09 = source.Value09,
        };
        var created = new CreatedState();
        var payload = new PayloadState { Count = source.Count };
        Emit(carrier, created, payload);
    }

    private void Emit(object first, object second, object third) {}
}

public class SourceState {
    public int Value00 { get; set; }
    public int Value01 { get; set; }
    public int Value02 { get; set; }
    public int Value03 { get; set; }
    public int Value04 { get; set; }
    public int Value05 { get; set; }
    public int Value06 { get; set; }
    public int Value07 { get; set; }
    public int Value08 { get; set; }
    public int Value09 { get; set; }
    public int Count { get; set; }
}

public class CarrierState {}
public class CreatedState {}
public class PayloadState {}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Producer",
                "body": true,
                "expand": true,
                "max_results": 1
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);
        let data_section = out
            .split("body data/type leads")
            .nth(1)
            .unwrap_or_default()
            .split("body data/type external reference leads")
            .next()
            .unwrap_or_default();
        assert!(data_section.contains("CarrierState"));
        assert!(data_section.contains("CreatedState"));
        assert!(data_section.contains("PayloadState"));
        assert!(!data_section.contains("Value00 ->"));
        assert!(!data_section.contains("Count ->"));
    }

    #[test]
    fn data_type_reference_scope_continuation_keeps_tail_state_types() {
        let root = temp_tools_test_dir("data_type_reference_scope_continuation");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public void Producer() {
        var source = new SourceState();
        source.Count = 1;
    }

    public void Consumer() {
        var source = Read<SourceState>();
        var later = new LaterState { Count = source.Count };
        var tail = new TailState(later.Count);
        Emit(tail);
    }

    private T Read<T>() { return default(T); }
    private void Emit(object value) {}
}

public class SourceState { public int Count; }
public class LaterState { public int Count; }
public class TailState { public TailState(int count) {} }
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Producer",
                "body": true,
                "expand": true,
                "max_results": 1
            }),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("reference scope continuation leads"));
        assert!(out.contains("LaterState"));
        assert!(out.contains("TailState"));
    }

    #[test]
    fn data_type_reference_scope_path_allows_related_external_files() {
        let root = temp_tools_test_dir("data_type_reference_external_scope_path");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/producer.rs"),
            r#"pub fn build() {
    let state = SharedState { count: 1 };
    publish(state);
}

fn publish<T>(_value: T) {}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/consumer.rs"),
            r#"pub fn consume() {
    let state = read::<SharedState>();
    let view = ViewState { count: state.count };
    render(view);
}

fn read<T>() -> T { panic!() }
fn render<T>(_value: T) {}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/state.rs"),
            r#"pub struct SharedState { pub count: i32 }
pub struct ViewState { pub count: i32 }
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let consumer_file = index.file("src/consumer.rs").unwrap();
        let allowed =
            data_type_reference_scope_path_allowed(&index, "src/producer.rs", consumer_file);

        let _ = std::fs::remove_dir_all(&root);
        assert!(allowed);
    }

    #[test]
    fn symbol_body_flow_handoff_leads_same_file_assignment_calls() {
        let root = temp_tools_test_dir("symbol_body_flow_handoff");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public void Producer() {
        target.StateId = BuildState(new SourceState());
    }

    private SourceState BuildState(SourceState value) {
        return value;
    }
}

public class SourceState {}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Producer",
                "body": true,
                "max_results": 1
            }),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("body flow handoff leads"));
        assert!(out.contains("BuildState"));
    }

    #[test]
    fn symbol_body_flow_handoff_leads_same_file_callback_values() {
        let root = temp_tools_test_dir("symbol_body_flow_callback");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public void Producer() {
        Register(OnDone);
    }

    private void Register(object callback) {
    }

    private void OnDone() {
    }
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Producer",
                "body": true,
                "max_results": 1
            }),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("body flow handoff leads"));
        assert!(out.contains("OnDone"));
    }

    #[test]
    fn symbol_body_flow_handoff_prefers_direct_split_family_calls() {
        let root = temp_tools_test_dir("symbol_body_flow_split_family");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public partial class Flow {
    public void Producer() {
        Register(OnChanged);
        FinishStep();
    }

    private void Register(object callback) {}
    private void OnChanged() {}
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Flow_Control.cs"),
            r#"
public partial class Flow {
    private void FinishStep() {
        Complete();
    }

    private void Complete() {}
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Producer",
                "path": "src/Flow.cs",
                "body": true,
                "max_results": 1
            }),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("FinishStep -> src/Flow_Control.cs"));
        assert!(out.contains("body handoff previews"));
    }

    #[test]
    fn outline_body_followups_rank_local_symbol_graph_bridges() {
        let root = temp_tools_test_dir("outline_body_followups");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public void Entry() {
        Middle();
    }

    public int Middle() {
        var value = Leaf();
        return value;
    }

    private int Leaf() {
        return 1;
    }
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_outline(
            &index,
            &json!({
                "path": "src/Flow.cs",
                "compact": true
            }),
        )
        .unwrap();
        let mut candidate = ContextCandidate::new("src/Flow.cs".to_string());
        candidate.score = 10.0;
        candidate.reasons.insert("semantic task recall".to_string());
        let mut flow_table = String::new();
        append_context_flow_candidate_table(
            &index,
            &[candidate],
            "entry middle leaf",
            &mut flow_table,
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("same-file atomicity"));
        assert!(out.contains("include_connected_ranges=true"));
        assert!(out.contains("outline body follow-up candidates"));
        assert!(out.contains("codedb_symbol name=Middle"));
        assert!(out.contains("body=true"));
        assert!(flow_table.contains("structural body followups"));
        assert!(flow_table.contains("codedb_symbol name=Middle"));
    }

    #[test]
    fn outline_body_followups_keep_short_bridge_when_state_surfaces_exist() {
        let root = temp_tools_test_dir("outline_short_bridge");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public int Progress => External.GetProgress();
    public int Status => External.GetStatus();

    public void Entry() {
        Heavy();
        Finish();
    }

    private void Heavy() {
        External.One(); External.Two(); External.Three(); External.Four();
    }

    private void Finish() {
        External.Done();
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/External.cs"),
            r#"
public static class External {
    public static int GetProgress() => 1;
    public static int GetStatus() => 1;
    public static void One() {}
    public static void Two() {}
    public static void Three() {}
    public static void Four() {}
    public static void Done() {}
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_outline(
            &index,
            &json!({
                "path": "src/Flow.cs",
                "compact": true
            }),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("codedb_symbol name=Finish"));
    }

    #[test]
    fn long_symbol_bodies_put_active_tail_leads_before_comment_free_source() {
        let root = temp_tools_test_dir("long_active_symbol_body");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let comments = (0..100)
            .map(|idx| format!("        // LegacyCall{idx}();"))
            .collect::<Vec<_>>()
            .join("\n");
        let active_lines = (0..450)
            .map(|idx| format!("        value += {idx};"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(
            root.join("src/Flow.cs"),
            format!(
                "public class Flow {{\n    public void Complete() {{\n        int value = 0;\n{comments}\n{active_lines}\n        ProcedureHelper.GotoCity();\n    }}\n}}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            root.join("src/ProcedureHelper.cs"),
            "public static class ProcedureHelper { public static void GotoCity() {} }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Complete",
                "body": true,
                "max_results": 1
            }),
        )
        .unwrap();
        let lead = out.find("GotoCity").unwrap();
        let body = out
            .find("body lines (active code; comments omitted)")
            .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(lead < body);
        assert!(!out.contains("LegacyCall"));
        assert!(out.contains("value += 449;"));
        assert!(!out.contains("active body capped"));
    }

    #[test]
    fn symbol_body_surfaces_exact_cross_file_qualified_tail_call() {
        let root = temp_tools_test_dir("qualified_tail_call");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Controller.cs"),
            r#"
public class Controller {
    public void Initialize() {
        Helpers.One();
        Helpers.Two();
        Helpers.Three();
        Helpers.Four();
        Helpers.Five();
        Helpers.Six();
        Helpers.Seven();
        Helpers.Eight();
        MODULE.HomeScene.EnterHomeScene(oldParam);
        Game.OnInitK();
        Helpers.Done();
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Helpers.cs"),
            r#"
public static class Helpers {
    public static void One() {}
    public static void Two() {}
    public static void Three() {}
    public static void Four() {}
    public static void Five() {}
    public static void Six() {}
    public static void Seven() {}
    public static void Eight() {}
    public static void Done() {}
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Game.cs"),
            "public static class Game { public static void OnInitK() {} }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/HomeSceneModule.cs"),
            "public class HomeSceneModule { public void EnterHomeScene(CityStateParams param) {} }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/HomeSceneMediator.cs"),
            "public class HomeSceneMediator { public void EnterHomeScene(long cityId, float x, float y, int level) {} }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Initialize",
                "path": "src/Controller.cs",
                "body": true,
                "max_results": 1
            }),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("exact body evidence: src/Controller.cs"));
        assert!(out.contains("not a checklist"));
        assert!(out.contains("body qualified tail call leads"));
        assert!(out.contains("EnterHomeScene -> src/HomeSceneModule.cs"));
        assert!(out.contains("OnInitK -> src/Game.cs"));
        assert!(out.contains("Game.OnInitK();"));
    }

    #[test]
    fn symbol_body_continuation_chain_expands_the_next_node() {
        let root = temp_tools_test_dir("continuation_chain_next_node");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Entry.cs"),
            "public class Entry { public void Start() { First.StepOne(); } }",
        )
        .unwrap();
        std::fs::write(
            root.join("src/First.cs"),
            "public static class First { public static void StepOne() { Second.StepTwo(); } }",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Second.cs"),
            "public static class Second { public static void StepTwo() { Third.Finish(); } }",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Third.cs"),
            "public static class Third { public static void Finish() { } }",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH SHORTEST p=(start:Symbol)-[:CALLS*1..5]->(finish:Symbol) WHERE start.name='Start' AND start.path='src/Entry.cs' AND finish.name='Finish' AND finish.path='src/Third.cs' RETURN p"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("StepOne"));
        assert!(out.contains("StepTwo"));
        assert!(out.contains("Finish"));
        assert!(out.contains("\"count\": 1"));
        assert!(!out.contains("active_body"));
    }

    #[test]
    fn outline_groups_connected_members_into_one_atomic_read_range() {
        let root = temp_tools_test_dir("outline_connected_member_range");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public void Start() { Other.Select(); Step(); }
    private void Step() { Finish(); }
    private void Finish() { }
    private void Isolated() { }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Other.cs"),
            "public static class Other { public static void Select() { } }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_outline(
            &index,
            &json!({
                "path": "src/Flow.cs",
                "compact": true,
                "include_connected_ranges": true,
                "include_body_followups": false
            }),
        )
        .unwrap();

        let ranges = out
            .split("outline connected member ranges")
            .nth(1)
            .unwrap_or_default();
        assert!(ranges.contains("members=[Start, Step, Finish]"));
        assert!(ranges.contains("codedb_read path=src/Flow.cs"));
        assert!(ranges.contains("connected_range=true"));
        assert!(ranges.contains("include_symbol_leads=true"));
        assert!(!ranges.contains("Isolated"));

        let read = handle_read(
            &index,
            &json!({
                "path": "src/Flow.cs",
                "line_start": 3,
                "line_end": 5,
                "compact": true,
                "connected_range": true,
                "include_symbol_leads": false
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(read.contains("connected range closure"));
        assert!(read.contains("contained members=[Start, Step, Finish]"));
        assert!(read.contains("connected range cross-file handoff frontier"));
        assert!(read.contains("direct handoff: Select"));
        assert!(read.contains("Other.cs"));
        assert!(!read.contains("follow-up: codedb_symbol name=Start"));
    }

    #[test]
    fn connected_range_emits_each_member_cross_file_frontier_without_name_ranking_loss() {
        let root = temp_tools_test_dir("connected_range_cross_file_frontier");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public void Start() { var selected = Api.Select(); Step(); }
    private void Step() { LongerSpecificMemberName(); }
    private void LongerSpecificMemberName() { Other.Notify(); }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Api.cs"),
            "public static class Api { public static int Select() { return 1; } }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Other.cs"),
            "public static class Other { public static void Notify() { } }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let read = handle_read(
            &index,
            &json!({
                "path": "src/Flow.cs",
                "line_start": 3,
                "line_end": 5,
                "compact": true,
                "connected_range": true,
                "include_symbol_leads": true
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(read.contains("connected range cross-file handoff frontier"));
        assert!(read.contains("Start L3"));
        assert!(read.contains("value/control boundary: Select -> src/Api.cs"));
        assert!(read.contains("codedb_symbol name=Select path=src/Api.cs body=true"));
        assert!(read.contains("LongerSpecificMemberName L5"));
        assert!(read.contains("Notify -> src/Other.cs"));
    }

    #[test]
    fn connected_range_incoming_frontier_preserves_call_site_preprocessor_guards() {
        let root = temp_tools_test_dir("connected_range_incoming_guards");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Target.cs"),
            r#"
public class Target {
    public void GuardedEntry() { }
    public void UnguardedEntry() { }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Caller.cs"),
            r#"
public class Caller {
    public void Run() {
#if OPTIONAL_FEATURE
        new Target().GuardedEntry();
#endif
        new Target().UnguardedEntry();
    }
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let read = handle_read(
            &index,
            &json!({
                "path": "src/Target.cs",
                "line_start": 3,
                "line_end": 4,
                "compact": true,
                "connected_range": true,
                "include_symbol_leads": true
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(read.contains("connected range incoming call frontier"));
        assert!(read.contains("GuardedEntry L3 <- src/Caller.cs:5 [#if OPTIONAL_FEATURE]"));
        assert!(
            read.contains("UnguardedEntry L4 <- src/Caller.cs:7 [no enclosing preprocessor guard]")
        );
    }

    #[test]
    fn qualified_receiver_declaration_type_disambiguates_same_named_callees() {
        let root = temp_tools_test_dir("qualified_receiver_declared_type");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Api.cs"),
            r#"
public static class Api {
    private static RuntimeService _service;
    public static int Load() { return _service.Fetch(); }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/RuntimeService.cs"),
            "public class RuntimeService { public int Fetch() { return 1; } }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/WrongService.cs"),
            "public class WrongService { public int Fetch() { return 2; } }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Load",
                "path": "src/Api.cs",
                "body": true,
                "max_results": 1
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(out.contains("Fetch -> src/RuntimeService.cs"));
        assert!(!out.contains("Fetch -> src/WrongService.cs"));
    }

    #[test]
    fn interface_methods_emit_dispatch_branches_without_fake_implementation_chain() {
        let root = temp_tools_test_dir("interface_dispatch_branches");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/IRunner.cs"),
            "public interface IRunner { int Execute(); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/LiveRunner.cs"),
            r#"public class LiveRunner : IRunner {
    public int Execute() {
        return Execute(1);
    }
    private int Execute(int value) {
        return value;
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/AltRunner.cs"),
            "public class AltRunner : IRunner { public int Execute() { return 2; } }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (interface:Symbol {name:'Execute', path:'src/IRunner.cs'})-[:DISPATCHES_TO]->(implementation:Symbol) RETURN implementation.path, implementation.line_start, implementation.detail"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(out.contains("src/AltRunner.cs"));
        assert!(out.contains("src/LiveRunner.cs"));
        assert!(out.contains("\"count\": 2"));
        assert!(!out.contains("branch preview"));
    }

    #[test]
    fn graph_query_exposes_communities_and_rankable_file_metrics() {
        let root = temp_tools_test_dir("graph_query_community_metrics");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/entry.js"),
            "import { run } from './service.js';\nexport function start() { return run(); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/service.js"),
            "import { load } from './store.js';\nexport function run() { return load(); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/store.js"),
            "export function load() { return 1; }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/isolated.js"),
            "export function isolated() { return 0; }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let communities = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (community:Community) RETURN community.id, community.size, community.representative_path ORDER BY community.size DESC"
            }),
        )
        .unwrap();
        let files = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (community:Community)-[:CONTAINS]->(file:File) RETURN community.id, file.path, file.degree, file.incoming_degree, file.outgoing_degree ORDER BY file.degree DESC"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(communities.contains("community.representative_path"));
        assert!(communities.contains("community.size"));
        assert!(files.contains("src/entry.js"));
        assert!(files.contains("src/service.js"));
        assert!(files.contains("file.incoming_degree"));
        assert!(files.contains("file.outgoing_degree"));
        assert!(files.contains("\"count\": 4"));
    }

    #[test]
    fn graph_query_exposes_topology_file_labels() {
        let root = temp_tools_test_dir("graph_query_topology_labels");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Entry.cs"),
            r#"
using Demo.Services;
namespace Demo
{
    public class Entry
    {
        private Service service;
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Service.cs"),
            r#"
namespace Demo.Services
{
    public class Service
    {
    }
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let entries = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (file:EntryFile) RETURN file.path, file.outgoing_degree ORDER BY file.outgoing_degree DESC"
            }),
        )
        .unwrap();
        let sinks = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (file:SinkFile) RETURN file.path, file.incoming_degree ORDER BY file.incoming_degree DESC"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(entries.contains("src/Entry.cs"), "{entries}");
        assert!(sinks.contains("src/Service.cs"), "{sinks}");
    }

    #[test]
    fn graph_query_resolves_incoming_references_from_an_exact_target() {
        let root = temp_tools_test_dir("graph_query_incoming_reference");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/UIDefine.cs"),
            "public static class UIDefine { public const string MainPanel = \"main_panel\"; }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Entry.cs"),
            "public class Entry { public void Open() { VIEW.OpenUI(UIDefine.MainPanel); } }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let result = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (caller:Symbol)-[reference:REFERENCES]->(target:Symbol) WHERE target.name='MainPanel' AND target.path='src/UIDefine.cs' RETURN caller.name, caller.path, reference.line"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(result.contains("\"caller.name\": \"Open\""), "{result}");
        assert!(result.contains("\"caller.path\": \"src/Entry.cs\""));
        assert!(result.contains("\"count\": 1"));
    }

    #[test]
    fn graph_query_seeds_exact_argument_values_and_returns_owners() {
        let root = temp_tools_test_dir("graph_query_exact_argument_value");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/EventDefine.cs"),
            "public static class EventDefine { public const int OnInitEnd = 1; }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/EventBus.cs"),
            r#"
public static class EventBus {
    public static void Broadcast(int id) { }
    public static void AddListener(int id, System.Action action) { }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public void Publish() { EventBus.Broadcast(EventDefine.OnInitEnd); }
    public void Subscribe() { Event.Instance.AddListener(this, EventDefine.OnInitEnd, () => OnReady()); }
    private void OnReady() { }
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let result = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (owner:Symbol)-[:HAS_CALLSITE]->(call:CallSite)-[:ARGUMENT]->(value:Value) WHERE value.expression='EventDefine.OnInitEnd' RETURN owner.name, call.name, call.line, value.index ORDER BY owner.name"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(result.contains("\"owner.name\": \"Publish\""), "{result}");
        assert!(result.contains("\"owner.name\": \"Subscribe\""));
        assert!(result.contains("\"call.name\": \"Broadcast\""));
        assert!(result.contains("\"call.name\": \"AddListener\""));
        assert!(result.contains("\"value.index\": 1"));
        assert!(result.contains("\"count\": 2"));
    }

    #[test]
    fn forwarding_chain_contracts_into_terminal_dispatch_branch_previews() {
        let root = temp_tools_test_dir("forwarding_chain_dispatch_preview");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Entry.cs"),
            r#"public class Entry {
    public int Run(bool skip) {
        return Api.Fetch(skip);
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Api.cs"),
            r#"public static class Api {
    private static Package _package;
    public static int Fetch(bool skip) {
        return _package.Fetch(skip);
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Package.cs"),
            r#"public class Package {
    private IFetchService _service;
    public int Fetch(bool skip) {
        return _service.Fetch(skip);
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/IFetchService.cs"),
            "public interface IFetchService { int Fetch(bool skip); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/LiveFetchService.cs"),
            "public class LiveFetchService : IFetchService { public int Fetch(bool skip) { return skip ? 0 : 1; } }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/AltFetchService.cs"),
            "public class AltFetchService : IFetchService { public int Fetch(bool skip) { return 2; } }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH SHORTEST p=(start:Symbol)-[:CALLS*1..4]->(terminal:Symbol), (terminal)-[:DISPATCHES_TO]->(implementation:Symbol) WHERE start.name='Run' AND start.path='src/Entry.cs' AND terminal.name='Fetch' AND terminal.path='src/IFetchService.cs' RETURN p, implementation.path, implementation.detail"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(out.contains("src/Api.cs"));
        assert!(out.contains("src/Package.cs"));
        assert!(out.contains("src/IFetchService.cs"));
        assert!(out.contains("src/LiveFetchService.cs"));
        assert!(out.contains("src/AltFetchService.cs"));
        assert!(out.contains("\"count\": 2"));
    }

    #[test]
    fn dispatch_preview_emits_parameter_to_control_action_evidence() {
        let root = temp_tools_test_dir("dispatch_parameter_control_evidence");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/ISelector.cs"),
            "public interface ISelector { List<Item> Select(string[] tags, bool filter = true); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/HostSelector.cs"),
            r#"public class HostSelector : ISelector {
    public List<Item> Select(string[] tags, bool filter = true) {
        List<Item> selected = new List<Item>();
        foreach (var item in Items) {
            if (filter && item.Has(tags)) {
                continue;
            }
            selected.Add(item);
        }
        return selected;
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/TagSelector.cs"),
            r#"public class TagSelector : ISelector {
    public List<Item> Select(string[] tags, bool filter = true) {
        List<Item> selected = new List<Item>();
        foreach (var item in Items) {
            bool isTagOK = item.Has(tags);
            if (!isTagOK) continue;
            selected.Add(item);
        }
        return selected;
    }
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let host = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (leaf:Symbol {name:'Select', path:'src/HostSelector.cs'})-[:HAS_PARAMETER]->(parameter:Parameter)-[:USED_IN]->(condition:Condition)-[:TRUE]->(skip:ControlAction)-[:PREVENTS]->(append:CallSite) WHERE parameter.name='tags' AND append.name='Add' RETURN condition.text, condition.negated, skip.kind, append.text"
            }),
        )
        .unwrap();
        let tag = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (leaf:Symbol {name:'Select', path:'src/TagSelector.cs'})-[:HAS_PARAMETER]->(parameter:Parameter)-[use:USED_IN]->(condition:Condition)-[:TRUE]->(skip:ControlAction)-[:PREVENTS]->(append:CallSite) WHERE parameter.name='tags' AND append.name='Add' RETURN use.via, condition.text, condition.negated, skip.kind, append.text"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(host.contains("item.Has(tags)"));
        assert!(host.contains("\"condition.negated\": false"));
        assert!(host.contains("\"skip.kind\": \"continue\""));
        assert!(host.contains("selected.Add(item)"));
        assert!(tag.contains("\"use.via\": \"isTagOK\""));
        assert!(tag.contains("if (!isTagOK) continue;"));
        assert!(tag.contains("\"condition.negated\": true"));
        assert!(tag.contains("selected.Add(item)"));
    }

    #[test]
    fn connected_range_value_boundary_emits_contracted_terminal_dispatch_preview() {
        let root = temp_tools_test_dir("connected_range_dispatch_preview");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    public int Run(bool skip) { var selected = Api.Fetch(skip); return selected; }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Api.cs"),
            r#"public static class Api {
    private static IFetchService _service;
    public static int Fetch(bool skip) {
        return _service.Fetch(skip);
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/IFetchService.cs"),
            "public interface IFetchService { int Fetch(bool skip); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/LiveFetchService.cs"),
            "public class LiveFetchService : IFetchService { public int Fetch(bool skip) { return skip ? 0 : 1; } }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let read = handle_read(
            &index,
            &json!({
                "path": "src/Flow.cs",
                "line_start": 3,
                "line_end": 3,
                "compact": true,
                "connected_range": true,
                "include_symbol_leads": true
            }),
        )
        .unwrap();
        let graph = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH SHORTEST p=(start:Symbol)-[:CALLS*1..3]->(terminal:Symbol), (terminal)-[:DISPATCHES_TO]->(implementation:Symbol) WHERE start.name='Run' AND start.path='src/Flow.cs' AND terminal.name='Fetch' AND terminal.path='src/IFetchService.cs' RETURN p, implementation.path"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(read.contains("value/control boundary: Fetch -> src/Api.cs"));
        assert!(!read.contains("contracted leaf corridor"));
        assert!(graph.contains("src/IFetchService.cs"));
        assert!(graph.contains("src/LiveFetchService.cs"));
    }

    #[test]
    fn value_boundary_contracts_independently_when_body_has_other_cross_file_calls() {
        let root = temp_tools_test_dir("value_boundary_with_other_calls");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"public class Flow {
    public int Run(bool skip) {
        var selected = Api.Fetch(skip);
        Audit.Record();
        return selected;
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/Api.cs"),
            r#"public static class Api {
    private static IFetchService _service;
    public static int Fetch(bool skip) {
        return _service.Fetch(skip);
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/IFetchService.cs"),
            "public interface IFetchService { int Fetch(bool skip); }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/LiveFetchService.cs"),
            "public class LiveFetchService : IFetchService { public int Fetch(bool skip) { return skip ? 0 : 1; } }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Audit.cs"),
            "public static class Audit { public static void Record() {} }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Run",
                "path": "src/Flow.cs",
                "body": true,
                "max_results": 1
            }),
        )
        .unwrap();
        let graph = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH SHORTEST p=(start:Symbol)-[:CALLS*1..3]->(terminal:Symbol), (terminal)-[:DISPATCHES_TO]->(implementation:Symbol) WHERE start.name='Run' AND start.path='src/Flow.cs' AND terminal.name='Fetch' AND terminal.path='src/IFetchService.cs' RETURN p, implementation.path"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);

        assert!(out.contains("Fetch -> src/Api.cs"));
        assert!(out.contains("Record -> src/Audit.cs"));
        assert!(!out.contains("contracted leaf corridor"));
        assert!(graph.contains("src/IFetchService.cs"));
        assert!(graph.contains("src/LiveFetchService.cs"));
    }

    #[test]
    fn callpath_uses_qualified_handoffs_and_returns_active_bodies() {
        let root = temp_tools_test_dir("qualified_callpath");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Controller.cs"),
            "public class Controller { public void Initialize() { Game.OnInitK(); } }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Game.cs"),
            "public static class Game { public static void OnInitK() { Ready(); } public static void Ready() {} }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_callpath(
            &index,
            &json!({
                "from": "Initialize",
                "from_path": "src/Controller.cs",
                "to": "OnInitK",
                "to_path": "src/Game.cs",
                "max_hops": 4
            }),
        )
        .unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(value["found"].as_bool(), Some(true));
        assert_eq!(value["hops"].as_u64(), Some(1));
        assert!(
            value["path"][0]["active_body"]
                .as_str()
                .is_some_and(|body| body.contains("Game.OnInitK"))
        );
        assert!(
            value["path"][1]["active_body"]
                .as_str()
                .is_some_and(|body| body.contains("Ready"))
        );
    }

    #[test]
    fn callpath_argument_matching_handles_repeated_names_and_optional_parameters() {
        let counts = identifier_call_argument_counts(
            "AddListener(EventDefine.OnInitEnd, () => OnInitEnd(context));",
            "OnInitEnd",
        );
        assert_eq!(counts, BTreeSet::from([1]));
        assert_eq!(
            identifier_call_receiver_kinds(
                "AddListener(EventDefine.OnInitEnd, () => OnInitEnd(context));",
                "OnInitEnd"
            ),
            (true, false)
        );
        assert!(signature_accepts_argument_count(
            "SetState(IState state, IParams params_ = null)",
            1
        ));
        assert!(signature_accepts_argument_count(
            "SetState(IState state, IParams params_ = null)",
            2
        ));
        assert!(!signature_accepts_argument_count(
            "SetState(IState state, IParams params_ = null)",
            0
        ));
    }

    #[test]
    fn callpath_does_not_turn_declarations_or_local_calls_into_cross_type_edges() {
        let root = temp_tools_test_dir("callpath_cross_type_false_edges");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/A.cs"),
            "public class A\n{\n    public void Start()\n    {\n        Debug.Log(\"Start finished\");\n        GameStateManager.instance.ToString();\n    }\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/B.cs"),
            "public class B\n{\n    public void Start()\n    {\n        AddListener(EventDefine.OnShow, () => OnShow(false));\n    }\n\n    private void OnShow(bool value)\n    {\n    }\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/C.cs"),
            "public class C\n{\n    public static object instance => null;\n\n    public void OnShow(object value)\n    {\n    }\n}\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let declaration_out = handle_callpath(
            &index,
            &json!({
                "from": "Start",
                "from_path": "src/A.cs",
                "to": "Start",
                "to_path": "src/B.cs",
                "max_hops": 1
            }),
        )
        .unwrap();
        let declaration_value: Value = serde_json::from_str(&declaration_out).unwrap();
        assert_eq!(declaration_value["found"].as_bool(), Some(false));

        let qualified_member_out = handle_callpath(
            &index,
            &json!({
                "from": "Start",
                "from_path": "src/A.cs",
                "to": "instance",
                "to_path": "src/C.cs",
                "max_hops": 1
            }),
        )
        .unwrap();
        let qualified_member_value: Value = serde_json::from_str(&qualified_member_out).unwrap();
        assert_eq!(qualified_member_value["found"].as_bool(), Some(false));

        let local_out = handle_callpath(
            &index,
            &json!({
                "from": "Start",
                "from_path": "src/B.cs",
                "to": "OnShow",
                "to_path": "src/C.cs",
                "max_hops": 1
            }),
        )
        .unwrap();
        let local_value: Value = serde_json::from_str(&local_out).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(local_value["found"].as_bool(), Some(false), "{local_out}");
    }

    #[test]
    fn graph_flow_scoring_prefers_compact_roots_over_giant_hubs() {
        let entry = graph_flow_root_priority(8, 0, 8, 6, 120);
        let giant_hub = graph_flow_root_priority(938, 1467, 2405, 10_000, 20_000);

        assert!(entry > giant_hub);
        assert!(
            graph_flow_boundary_priority(entry, 3, 8, 8)
                > graph_flow_boundary_priority(giant_hub, 900, 938, 2405)
        );
        assert!(graph_flow_reach_bonus(40) > graph_flow_reach_bonus(2));
    }

    #[test]
    fn context_token_budget_does_not_have_a_server_file_ceiling() {
        assert_eq!(context_max_files_for_token_budget(80_000), 80);
    }

    #[test]
    fn bundle_executes_all_operations_without_output_budget_or_argument_rewrite() {
        let root = temp_tools_test_dir("unbounded_bundle");
        let manager = ProjectManager::new_lazy(root, IndexOptions::default());
        let ops = (0..5)
            .map(|_| json!({"tool": "codedb_projects", "arguments": {}}))
            .collect::<Vec<_>>();

        let out = handle_bundle(
            &manager,
            &json!({
                "ops": ops,
                "max_output_chars": 1,
                "max_child_chars": 1
            }),
        )
        .unwrap();

        assert!(out.starts_with("bundle 5 ops\n"));
        assert_eq!(out.matches("no projects indexed").count(), 5);
        assert!(out.contains("--- [4] codedb_projects ---"));
        assert!(!out.contains("truncated"));
        assert!(!out.contains("skipped"));
    }

    #[test]
    fn symbol_body_literal_bridges_use_original_identifier_terms() {
        let root = temp_tools_test_dir("literal_metadata_bridge");
        std::fs::create_dir_all(root.join("Assets/Scripts/GameAOT/GameStart/GameState")).unwrap();
        std::fs::create_dir_all(root.join("Assets/Scripts/HotFix/Runtime/GameStart")).unwrap();
        std::fs::create_dir_all(root.join("Assets/Scripts/HotFix/PreClientCode/Audio/UnityAudio"))
            .unwrap();
        std::fs::write(
            root.join("Assets/Scripts/GameAOT/GameStart/GameState/GameInitState.cs"),
            r#"
public class GameInitState {
    private const string InitPrefabName = "GameInit_AudioBro";
    public void Enter() {
        Loader.Instantiate(InitPrefabName);
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("Assets/Scripts/HotFix/Runtime/GameStart/GameInitController.cs"),
            "public class GameInitController { public void Initialize() {} }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("Assets/Scripts/GameAOT/GameStart/Loader.cs"),
            "public static class Loader { public static void Instantiate(string name) {} }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("Assets/Scripts/HotFix/PreClientCode/Audio/UnityAudio/AudioPlayer.cs"),
            "public class AudioPlayer { public void InitAudio() {} }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_symbol(
            &index,
            &json!({
                "name": "Enter",
                "path": "Assets/Scripts/GameAOT/GameStart/GameState/GameInitState.cs",
                "body": true,
                "max_results": 1
            }),
        )
        .unwrap();
        let outline = handle_outline(
            &index,
            &json!({
                "path": "Assets/Scripts/GameAOT/GameStart/GameState/GameInitState.cs",
                "compact": true,
                "include_body_followups": true
            }),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("body literal bridge leads"));
        assert!(out.contains("GameInit_AudioBro"));
        assert!(out.contains("Assets/Scripts/HotFix/Runtime/GameStart/GameInitController.cs"));
        let bridge_section = out.split("body literal bridge leads:\n").nth(1).unwrap();
        assert!(
            bridge_section
                .lines()
                .next()
                .unwrap_or_default()
                .contains("GameInitController.cs")
        );
        assert!(outline.contains("outline literal bridge leads"));
        assert!(outline.contains("GameInitController.cs"));
    }

    #[test]
    fn compact_read_and_word_refs_ignore_block_commented_code() {
        let root = temp_tools_test_dir("compact_read_active_code");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public class Flow {
    /*
    public void Deprecated() { OldApi(); }
    */
    public void Current() { CurrentApi(); }
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_read_one(
            &index,
            &json!({
                "path": "src/Flow.cs",
                "line_start": 1,
                "line_end": 8,
                "compact": true
            }),
        )
        .unwrap();
        let old_hits = index.word_hits("OldApi").unwrap();
        let current_hits = index.word_hits("CurrentApi").unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(!out.contains("Deprecated"));
        assert!(!out.contains("OldApi"));
        assert!(out.contains("CurrentApi"));
        assert!(old_hits.is_empty());
        assert_eq!(current_hits.len(), 1);
    }

    #[test]
    fn outline_body_followups_include_live_external_handoffs_only() {
        let root = temp_tools_test_dir("outline_external_handoff");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Hero.cs"),
            r#"
public class Hero {
    private int _power;

    public int Power {
        get { return _power + EquipService.GetTotal(); }
    }

    public int Legacy {
        get {
            /* return _power + OldService.GetTotal(); */
            return _power;
        }
    }
}
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/EquipService.cs"),
            "public static class EquipService { public static int GetTotal() => 1; }",
        )
        .unwrap();
        std::fs::write(
            root.join("src/OldService.cs"),
            "public static class OldService { public static int GetTotal() => 2; }",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let out = handle_outline(
            &index,
            &json!({
                "path": "src/Hero.cs",
                "compact": true
            }),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("codedb_symbol name=Power"));
        assert!(!out.contains("OldService"));
    }

    #[test]
    fn ranked_search_surfaces_definition_before_dense_mentions() {
        let root = temp_tools_test_dir("definition_first_search");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/aaa_mentions.rs"),
            r#"pub fn use_one(value: SessionManager) { let _ = value; }
pub fn use_two(value: SessionManager) { let _ = value; }
pub fn use_three(value: SessionManager) { let _ = value; }
pub fn use_four(value: SessionManager) { let _ = value; }
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("src/zzz_definition.rs"),
            r#"pub struct SessionManager {
    pub active: bool,
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let hits =
            ranked_line_hits_with_scores(&index, "SessionManager", 5, None, true, true).unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            hits.first().map(|(hit, _)| hit.path.as_str()),
            Some("src/zzz_definition.rs")
        );
        assert_eq!(hits.first().map(|(hit, _)| hit.line), Some(1));
    }

    #[test]
    fn ranked_search_excludes_generated_web_assets_by_default() {
        let root = temp_tools_test_dir("search_excludes_generated_assets");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("public/assets")).unwrap();
        std::fs::write(
            root.join("src/runtime.rs"),
            "pub struct SessionManager { pub active: bool }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("public/assets/bundle.js"),
            "function SessionManager(){}; SessionManager(); SessionManager();\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();

        let hits =
            ranked_line_hits_with_scores_mode(&index, "SessionManager", 5, None, true, true, false)
                .unwrap();

        let _ = std::fs::remove_dir_all(&root);
        assert!(hits.iter().any(|(hit, _)| hit.path == "src/runtime.rs"));
        assert!(
            hits.iter()
                .all(|(hit, _)| !hit.path.contains("public/assets/"))
        );
    }

    #[test]
    fn assignment_target_identifier_extracts_lhs_symbol() {
        assert_eq!(
            assignment_target_identifier("Total = record.value;"),
            Some("Total".to_string())
        );
        assert_eq!(
            assignment_target_identifier("this.Result.Value = Calculate(input);"),
            Some("Value".to_string())
        );
        assert_eq!(
            assignment_target_identifier("cache[key] = value;"),
            Some("cache".to_string())
        );
        assert_eq!(
            assignment_target_identifier("if (left == right) return;"),
            None
        );
    }

    #[test]
    fn same_path_family_requires_shared_parent_and_family_key() {
        assert!(same_path_family(
            "src/domain/Thing.base.rs",
            "src/domain/Thing.state.rs"
        ));
        assert!(!same_path_family(
            "src/domain/Thing.base.rs",
            "src/other/Thing.state.rs"
        ));
        assert!(!same_path_family(
            "src/domain/Thing.base.rs",
            "src/domain/Other.state.rs"
        ));
        assert!(same_path_family(
            "src/domain/Flow.cs",
            "src/domain/Flow_Control.cs"
        ));
        assert!(!same_path_family(
            "src/domain/Login_Module.cs",
            "src/domain/Login_Manager.cs"
        ));
    }

    #[test]
    fn generic_path_prior_demotes_generated_and_secondary_sources() {
        assert_eq!(generic_source_path_score("src/runtime.rs"), 0.0);
        assert!(
            generic_source_path_score("viewer/public/assets/index-AbCd1234.js")
                < generic_source_path_score("tests/runtime_test.rs")
        );
        assert!(generic_source_path_score("vendor/dependency.rs") < 0.0);
        assert!(generic_source_path_score("docs/example.rs") < 0.0);
        assert!(generic_source_path_score("src/generated/schema.rs") < 0.0);
        assert!(generic_source_path_score("Runtime/Generate/conf/ConfHelper.cs") < 0.0);
        assert!(generic_source_path_score("Library/PackageCache/pkg/Runtime.cs") < 0.0);
        assert!(generic_source_path_score("Assets/Plugins/3rdPlugins/Runtime.cs") < 0.0);
        assert!(generic_source_path_score("Packages/tool/Editor/Setup.cs") < 0.0);
        assert!(generic_source_path_score("Assets/SRDebugger/SROptions.cs") < 0.0);
    }

    #[test]
    fn flow_quality_caps_uncorroborated_graph_navigation() {
        let quality = context_flow_quality(0, 6, 12, 8, 4, 0, 4, 6);
        let mut out = String::new();
        append_context_flow_quality(&quality, &mut out);

        assert_eq!(quality.score, 0.45);
        assert!(!quality.high_confidence);
        assert!(out.contains("confidence=low"));
        assert!(out.contains("mode=graph_navigation"));
        assert!(out.contains("connected_files=0"));
    }

    #[test]
    fn flow_quality_accepts_connected_graph_corroboration() {
        let quality = context_flow_quality(3, 6, 12, 8, 4, 2, 4, 6);
        let mut out = String::new();
        append_context_flow_quality(&quality, &mut out);

        assert!(quality.high_confidence);
        assert!(quality.score > 0.82);
        assert!(out.contains("mode=graph_corroborated"));
        assert!(out.contains("connected_files=3"));
        assert!(out.contains("traces=2/4"));
    }

    #[test]
    fn flow_trace_selection_prefers_multi_root_evidence() {
        let target = |name: &str, path: &str, line_start: usize| SymbolTarget {
            name: name.to_string(),
            kind: "method".to_string(),
            path: path.to_string(),
            line_start,
            detail: String::new(),
        };
        let root_a = target("RootA", "src/a.rs", 10);
        let root_b = target("RootB", "src/b.rs", 20);
        let roots = [symbol_target_key(&root_a), symbol_target_key(&root_b)]
            .into_iter()
            .collect::<BTreeSet<_>>();
        let tangential = ContextFlowTrace {
            steps: vec![
                root_a.clone(),
                target("Helper", "src/helper.rs", 30),
                target("Leaf", "src/leaf.rs", 40),
            ],
            score: 999.0,
            reason: "one-root branch".to_string(),
        };
        let corroborated = ContextFlowTrace {
            steps: vec![root_a, root_b],
            score: 1.0,
            reason: "two-root path".to_string(),
        };

        let selected = select_context_flow_traces(
            vec![tangential, corroborated],
            &roots,
            &["needle".to_string(), "signal".to_string()],
            2,
        );

        assert_eq!(selected[0].reason, "two-root path");
    }

    #[test]
    fn flow_spine_candidate_keeps_query_covered_symbol_and_rejects_generic_one() {
        let mut candidates = BTreeMap::new();
        let generic = SymbolTarget {
            name: "Create".to_string(),
            kind: "method".to_string(),
            path: "src/common/factory.rs".to_string(),
            line_start: 10,
            detail: String::new(),
        };
        let relevant = SymbolTarget {
            name: "TryJoinRally".to_string(),
            kind: "method".to_string(),
            path: "src/alliance/rally.rs".to_string(),
            line_start: 20,
            detail: String::new(),
        };
        let query_terms = vec![
            "alliance".to_string(),
            "rally".to_string(),
            "joining".to_string(),
        ];
        push_context_flow_spine_candidate(
            &mut candidates,
            generic.clone(),
            40.0,
            true,
            false,
            false,
            &query_terms,
        );
        push_context_flow_spine_candidate(
            &mut candidates,
            relevant.clone(),
            40.0,
            true,
            false,
            false,
            &query_terms,
        );

        assert!(candidates.contains_key(&symbol_target_key(&relevant)));
        assert!(!candidates.contains_key(&symbol_target_key(&generic)));
    }

    #[test]
    fn flow_spine_rejects_path_only_symbol_match() {
        let mut candidates = BTreeMap::new();
        let path_only = SymbolTarget {
            name: "Create".to_string(),
            kind: "method".to_string(),
            path: "src/alliance/rally.rs".to_string(),
            line_start: 10,
            detail: String::new(),
        };
        push_context_flow_spine_candidate(
            &mut candidates,
            path_only,
            40.0,
            true,
            false,
            false,
            &["alliance".to_string(), "rally".to_string()],
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn flow_spine_rejects_signature_only_symbol_match() {
        let mut candidates = BTreeMap::new();
        let signature_only = SymbolTarget {
            name: "DetermineRelation".to_string(),
            kind: "method".to_string(),
            path: "src/world/troop.rs".to_string(),
            line_start: 10,
            detail: "fn DetermineRelation(troop: WorldTroopInfo)".to_string(),
        };
        push_context_flow_spine_candidate(
            &mut candidates,
            signature_only,
            40.0,
            true,
            false,
            false,
            &["world".to_string(), "troop".to_string()],
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn flow_spine_prefers_query_terms_that_are_distinctive_within_candidates() {
        let candidate = |name: &str| ContextFlowSpineCandidate {
            target: SymbolTarget {
                name: name.to_string(),
                kind: "method".to_string(),
                path: format!("src/{name}.rs"),
                line_start: 1,
                detail: String::new(),
            },
            score: 100.0,
            direct: true,
            linked_root: false,
            split_family: false,
        };
        let mut candidates = vec![
            candidate("NetSendHeroRecruit"),
            candidate("OpenHeroPanel"),
            candidate("Power"),
        ];
        apply_context_flow_spine_query_distinctiveness(
            &mut candidates,
            &["hero".to_string(), "power".to_string()],
        );

        let hero_score = candidates[0].score;
        let power_score = candidates[2].score;
        assert!(power_score > hero_score);
    }

    #[test]
    fn flow_spine_source_includes_query_relevant_split_family_symbol() {
        let root = temp_tools_test_dir("flow_spine_split_family");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Hero.base.cs"),
            "public partial class Hero { public void Update() { } }\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src/Hero.power.cs"),
            "public partial class Hero { public float Power { get { return _power + equipPower; } } }\n",
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();
        let base_file = index.file("src/Hero.base.cs").unwrap();
        let update = base_file
            .symbols
            .iter()
            .find(|symbol| symbol.name == "Update")
            .unwrap();
        let flow_symbols = vec![ContextFlowSymbol {
            rank: 1,
            order: update.line_start,
            score: 10.0,
            target: target_from_symbol(base_file, update),
        }];
        let mut out = String::new();

        append_context_flow_spine_source(
            &index,
            &flow_symbols,
            &[],
            &["hero".to_string(), "power".to_string()],
            true,
            CONTEXT_FLOW_SPINE_SOURCE_TOTAL_CHARS,
            &mut out,
        );

        let _ = std::fs::remove_dir_all(&root);
        assert!(out.contains("Hero.power.cs"));
        assert!(out.contains("property Power"));
        assert!(out.contains("_power + equipPower"));
    }

    #[test]
    fn graph_query_returns_call_dispatch_guard_and_shared_state_facts() {
        let root = temp_tools_test_dir("graph_query_semantic_facts");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/Flow.cs"),
            r#"
public interface IService {
    void Filter(string[] tags);
}

public class HostService : IService {
    public void Filter(string[] tags) {}
}

public class Flow {
    private IService service;
    private object _task;

    public void LoadReady(string[] tags) {
#if MINI
        StartMiniUpdate();
#endif
        DownBundle();
        service.Filter(tags);
    }

    private void StartMiniUpdate() {}
    private void DownBundle() {}
    public void Produce() { _task = CreateTask(); }
    public void Consume() { if (_task != null) Use(_task); }
    public void Select(string[] tags) {
        bool isTagOK = HasTag(tags);
        foreach (var item in tags) {
            if (!isTagOK) {
                continue;
            }
            results.Add(item);
        }
    }
    private object CreateTask() { return new object(); }
    private bool HasTag(string[] tags) { return true; }
    private void Use(object value) {}
}
"#,
        )
        .unwrap();
        let mut options = IndexOptions::default();
        options.storage.enabled = false;
        options.respect_gitignore = false;
        let index = Codebase::index(&root, options).unwrap();
        let consume_target = index
            .symbols_named("Consume")
            .into_iter()
            .map(|(file, symbol)| target_from_symbol(file, symbol))
            .next()
            .unwrap();
        let consume_accesses = symbol_shared_state_accesses(&index, &consume_target).unwrap();
        assert!(
            !consume_accesses.is_empty(),
            "consume accesses missing; symbols={:?}",
            index
                .files
                .values()
                .flat_map(|file| file
                    .symbols
                    .iter()
                    .map(|symbol| (&symbol.name, symbol.kind.as_str())))
                .collect::<Vec<_>>()
        );
        let task_state = consume_accesses[0].state.clone();
        let incoming_accesses = incoming_shared_state_accesses(&index, &task_state).unwrap();
        assert!(
            !incoming_accesses.is_empty(),
            "incoming accesses missing for {:?}",
            task_state.name
        );

        let calls = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH p=(caller:Symbol)-[:CALLS|DISPATCHES_TO*1..2]->(leaf:Symbol) WHERE caller.name='LoadReady' RETURN p"
            }),
        )
        .unwrap();
        assert!(calls.contains("DISPATCHES_TO"));
        assert!(calls.contains("HostService"), "{calls}");

        let binding = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (caller:Symbol)-[:HAS_CALLSITE]->(call:CallSite)-[argument:ARGUMENT]->(value:Value)-[:BINDS_TO]->(parameter:Parameter) WHERE caller.name='LoadReady' AND call.name='Filter' AND value.index=0 RETURN value.expression, argument.index, parameter.name"
            }),
        )
        .unwrap();
        assert!(
            binding.contains("\"value.expression\": \"tags\""),
            "{binding}"
        );
        assert!(
            binding.contains("\"parameter.name\": \"tags\""),
            "{binding}"
        );

        let guarded = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (caller:Symbol)-[call:CALLS]->(leaf:Symbol) WHERE caller.name='LoadReady' AND leaf.name='StartMiniUpdate' RETURN call.guard, call.line"
            }),
        )
        .unwrap();
        assert!(guarded.contains("#if MINI"));

        let unguarded = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (caller:Symbol)-[call:CALLS]->(leaf:Symbol) WHERE caller.name='LoadReady' AND leaf.name='DownBundle' RETURN call.guarded, call.line"
            }),
        )
        .unwrap();
        assert!(unguarded.contains("\"call.guarded\": false"));

        let control = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (leaf:Symbol)-[:HAS_PARAMETER]->(param:Parameter)-[use:USED_IN]->(cond:Condition)-[:TRUE]->(skip:ControlAction)-[:PREVENTS]->(append:CallSite), (cond)-[:FALSE]->(fallthrough:ControlAction)-[:REACHES]->(append) WHERE leaf.name='Select' AND param.name='tags' AND append.name='Add' RETURN param.name, use.via, cond.text, cond.negated, skip.kind, append.text"
            }),
        )
        .unwrap();
        assert!(control.contains("\"cond.negated\": true"), "{control}");
        assert!(control.contains("\"skip.kind\": \"continue\""), "{control}");
        assert!(control.contains("results.Add(item)"), "{control}");

        let state = handle_graph_query(
            &index,
            &json!({
                "query": "MATCH (consumer:Symbol)-[:READS]->(state:SharedState)<-[:WRITES]-(producer:Symbol) WHERE consumer.name='Consume' AND state.name='_task' RETURN consumer.name, state.name, producer.name"
            }),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(state.contains("\"producer.name\": \"Produce\""), "{state}");
    }

    fn temp_tools_test_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "codedb_mcp_tools_{name}_{}_{}",
            std::process::id(),
            stamp
        ))
    }
}
