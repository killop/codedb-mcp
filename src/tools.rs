use crate::graph::GraphCommunity;
use crate::indexer::{
    ChangedFile, Codebase, IndexOptions, build_globset, fingerprint_project, hash_content,
    normalize_rel_path,
};
use crate::language::{is_comment_or_blank, scope_for_line};
use crate::search::hybrid_search;
use crate::tokens::has_whole_word;
use crate::types::{FileEntry, SearchHit, Symbol};
use anyhow::{Result, anyhow};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAX_BATCH_ITEMS: usize = 100;

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
            out.push_str(&extract_lines(
                &file.content,
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
    Ok(format_chunk_hits(&query, hits))
}

fn handle_word(index: &Codebase, args: &Value) -> Result<String> {
    let word = required_str(args, "word")?;
    let hits = index.word_index.get(&word).cloned().unwrap_or_default();
    let mut out = format!("{} hits for '{}':\n", hits.len(), word);
    for hit in hits {
        out.push_str(&format!("  {}:{}\n", hit.path, hit.line));
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
    let mut hits = reference_candidates(index, &name);
    hits.retain(|hit| is_lsp_like_reference(index, &target, hit, &all_definition_sites));
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

fn reference_candidates(index: &Codebase, name: &str) -> Vec<SearchHit> {
    let Some(word_hits) = index.word_index.get(name) else {
        return Vec::new();
    };
    word_hits
        .iter()
        .filter_map(|hit| {
            let file = index.file(&hit.path)?;
            let text = file.content.lines().nth(hit.line.saturating_sub(1))?;
            let scope = scope_for_line(&file.symbols, hit.line);
            Some(SearchHit {
                path: hit.path.clone(),
                line: hit.line,
                text: text.trim().to_string(),
                scope,
            })
        })
        .collect()
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
        kind: symbol.kind.clone(),
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
        index.deps_forward.get(&path).cloned().unwrap_or_default()
    } else {
        index.deps_reverse.get(&path).cloned().unwrap_or_default()
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
    let hash = hash_content(&file.content);
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
        out.push_str(&extract_lines(&file.content, start, end, compact));
    } else {
        out.push_str(&file.content);
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
        "codedb status:\n  seq: {}\n  files: {}\n  outlines: {}\n  chunks: {}\n  graph: {} nodes, {} edges, {} communities\n  vector_index: vicinity HNSW ({} {} vectors)\n  embedding_model: model2vec-rs {} ({} dims)\n  scan: {}\n  extensions: {}\n  cache: {}\n  storage: {}\n",
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
    let snapshot = json!({
        "root": index.root.display().to_string(),
        "seq": index.seq,
        "stats": index.stats(),
        "files": index.files.values().collect::<Vec<_>>(),
        "deps": {
            "forward": &index.deps_forward,
            "reverse": &index.deps_reverse,
        },
        "graph": {
            "nodes": index.graph.nodes.len(),
            "edges": index.graph.edges.len(),
            "communities": index.graph.communities.len(),
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
                paths = Some(hits.iter().map(|hit| hit.chunk.file_path.clone()).collect());
                out = format_chunk_hits(&query, hits);
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
    match format.as_str() {
        "summary" => {
            let stats = index.graph.stats();
            let analysis = index.graph.analysis(10);
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
        "json" => serde_json::to_string_pretty(&index.graph.limited_json(max_nodes, max_edges))
            .map_err(Into::into),
        "graphml" => Ok(index.graph.to_graphml(max_nodes, max_edges)),
        "cypher" | "neo4j" => Ok(index.graph.to_cypher(max_nodes, max_edges)),
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
    match index.graph.explain(&term, limit) {
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
    let result = index.graph.shortest_path(&source, &target, max_depth);
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
        return serde_json::to_string_pretty(&index.graph.subcommunities_summary_for(
            parent,
            &subcommunities,
            child_id,
            limit,
            community_limit,
            include_members,
        ))
        .map_err(Into::into);
    }

    serde_json::to_string_pretty(&index.graph.communities_summary_for(
        communities,
        "lazy-louvain",
        community_id,
        limit,
        community_limit,
        include_members,
    ))
    .map_err(Into::into)
}

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

    let communities = index.graph.louvain_communities();
    save_louvain_cache(index, &graph_hash, &communities);
    *index.louvain_communities.write() = Some(communities);
}

fn louvain_graph_hash(index: &Codebase) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"codebase-mcp-louvain-v2");
    hasher.update(index.graph.nodes.len().to_string().as_bytes());
    hasher.update(index.graph.edges.len().to_string().as_bytes());
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

    let communities =
        index
            .graph
            .louvain_subcommunities(&parent.nodes, &index.files, Some(&parent.label));
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
    serde_json::to_string_pretty(&index.graph.analysis(top_n)).map_err(Into::into)
}

fn handle_export(index: &Codebase, args: &Value) -> Result<String> {
    let format = get_str(args, "format").unwrap_or_else(|| "json".to_string());
    let returning = get_str(args, "output_path").is_none();
    let max_nodes =
        get_usize(args, "max_nodes").unwrap_or(if returning { 1000 } else { usize::MAX });
    let max_edges =
        get_usize(args, "max_edges").unwrap_or(if returning { 3000 } else { usize::MAX });
    let content = match format.as_str() {
        "json" => serde_json::to_string_pretty(&index.graph.limited_json(max_nodes, max_edges))?,
        "graphml" => index.graph.to_graphml(max_nodes, max_edges),
        "cypher" | "neo4j" => index.graph.to_cypher(max_nodes, max_edges),
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
        out.push_str(&format!("--- [{}] {} ---\n", idx, tool));
        out.push_str(&dispatch_tool(manager, &tool, arguments));
        if !out.ends_with('\n') {
            out.push('\n');
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

fn format_chunk_hits(query: &str, hits: Vec<crate::types::ChunkSearchHit>) -> String {
    let mut out = format!("{} results for '{}':\n", hits.len(), query);
    for hit in hits {
        let preview = hit
            .chunk
            .content
            .lines()
            .filter(|line| !is_comment_or_blank(line))
            .take(5)
            .map(str::trim)
            .collect::<Vec<_>>()
            .join("\n    ");
        out.push_str(&format!(
            "  {}:{}-{}  [score={:.3}, {}]\n    {}\n",
            hit.chunk.file_path,
            hit.chunk.start_line,
            hit.chunk.end_line,
            hit.score,
            hit.source,
            preview
        ));
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

fn transitive_deps(
    index: &Codebase,
    path: &str,
    forward: bool,
    max_depth: Option<usize>,
) -> Vec<String> {
    let graph = if forward {
        &index.deps_forward
    } else {
        &index.deps_reverse
    };
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([(path.to_string(), 0usize)]);
    while let Some((current, depth)) = queue.pop_front() {
        if max_depth.is_some_and(|max| depth >= max) {
            continue;
        }
        for dep in graph.get(&current).into_iter().flatten() {
            if seen.insert(dep.clone()) {
                queue.push_back((dep.clone(), depth + 1));
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
