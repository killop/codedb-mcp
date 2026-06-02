use crate::event_log;
use crate::indexer::{
    IndexOptions, is_indexed_source_file, normalize_rel_path, source_watch_roots,
};
use crate::tools::{ProjectManager, ReindexCheck};
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub fn start_project_watcher(
    manager: Arc<ProjectManager>,
    poll_interval: Duration,
) -> Result<JoinHandle<()>> {
    let root = manager
        .default_root()
        .canonicalize()
        .context("failed to resolve watcher root")?;
    let poll_interval = poll_interval.max(Duration::from_secs(1));

    let handle = thread::Builder::new()
        .name("codebase-mcp-watch".to_string())
        .spawn(move || {
            if let Err(err) = watch_loop(manager, root, poll_interval) {
                event_log::log_file_watch_error("watcher_stop", &err.to_string());
                eprintln!("codebase-mcp watcher stopped: {err:#}");
            }
        })
        .context("failed to spawn watcher thread")?;
    Ok(handle)
}

fn watch_loop(manager: Arc<ProjectManager>, root: PathBuf, poll_interval: Duration) -> Result<()> {
    loop {
        let options = manager.options();
        let revision = manager.options_revision();
        run_watch_generation(&manager, &root, options, revision, poll_interval)?;
    }
}

fn run_watch_generation(
    manager: &Arc<ProjectManager>,
    root: &Path,
    options: IndexOptions,
    revision: u64,
    poll_interval: Duration,
) -> Result<()> {
    let extensions = options
        .extensions
        .iter()
        .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let watch_roots = source_watch_roots(root, &options);
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |event| {
            let _ = tx.send(event);
        },
        Config::default(),
    )
    .context("failed to create filesystem watcher")?;

    for watch_root in &watch_roots {
        watcher
            .watch(watch_root, RecursiveMode::Recursive)
            .with_context(|| format!("failed to watch {}", watch_root.display()))?;
    }

    eprintln!(
        "codebase-mcp watching {} every {:.1}s for: {}",
        display_watch_roots(&watch_roots),
        poll_interval.as_secs_f32(),
        display_extensions(&extensions)
    );
    if event_log::enabled() {
        event_log::log_file_watch_start(
            &display_watch_roots(&watch_roots),
            poll_interval.as_millis(),
            &display_extensions(&extensions),
        );
    }

    let mut known = match manager.get(None) {
        Ok(index) => known_from_index(index.as_ref()),
        Err(err) => {
            event_log::log_file_watch_error("initial_state_failed", &err.to_string());
            eprintln!("codebase-mcp watcher initial state failed: {err:#}");
            BTreeSet::new()
        }
    };
    let mut pending_changed = BTreeSet::<String>::new();
    let mut pending_deleted = BTreeSet::<String>::new();

    loop {
        thread::sleep(poll_interval);
        if manager.options_revision() != revision {
            event_log::log_file_watch_reconfigure("index_scope_changed");
            return Ok(());
        }
        let raw_events = drain_events(
            &rx,
            root,
            &options,
            &known,
            &mut pending_changed,
            &mut pending_deleted,
        );
        if raw_events > 0 {
            event_log::log_file_watch_digest_queued(
                raw_events,
                pending_changed.len(),
                pending_deleted.len(),
            );
        }
        if pending_changed.is_empty() && pending_deleted.is_empty() {
            continue;
        }

        let changed = pending_changed.iter().cloned().collect::<Vec<_>>();
        let deleted = pending_deleted.iter().cloned().collect::<Vec<_>>();
        pending_changed.clear();
        pending_deleted.clear();

        let started = Instant::now();
        event_log::log_file_watch_digest_start(changed.len(), deleted.len());
        match manager.apply_default_changes(changed.clone(), deleted.clone()) {
            Ok(ReindexCheck::Unchanged) => {
                event_log::log_file_watch_digest_unchanged(started, changed.len(), deleted.len());
            }
            Ok(ReindexCheck::Reindexed(index)) => {
                apply_known_delta(root, &mut known, &changed, &deleted);
                let stats = index.stats();
                event_log::log_file_watch_digest_finish(
                    started,
                    changed.len(),
                    deleted.len(),
                    stats.files,
                    stats.chunks,
                    stats.symbols,
                    &stats.cache,
                );
                eprintln!(
                    "codebase-mcp live update ready in {:.3}s: {} changed, {} deleted, {} files, {} chunks, {} symbols",
                    started.elapsed().as_secs_f32(),
                    changed.len(),
                    deleted.len(),
                    stats.files,
                    stats.chunks,
                    stats.symbols
                );
            }
            Err(err) => {
                event_log::log_file_watch_digest_failure(
                    started,
                    changed.len(),
                    deleted.len(),
                    &err.to_string(),
                );
                eprintln!("codebase-mcp polling reindex failed: {err:#}");
                pending_changed.extend(changed);
                pending_deleted.extend(deleted);
            }
        }
    }
}

fn display_watch_roots(roots: &[PathBuf]) -> String {
    roots
        .iter()
        .map(|root| root.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn display_extensions(extensions: &HashSet<String>) -> String {
    let mut items = extensions
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>();
    items.sort();
    items.join(", ")
}

fn known_from_index(index: &crate::indexer::Codebase) -> BTreeSet<String> {
    index.files.keys().cloned().collect()
}

fn drain_events(
    rx: &Receiver<notify::Result<Event>>,
    root: &Path,
    options: &crate::indexer::IndexOptions,
    known: &BTreeSet<String>,
    pending_changed: &mut BTreeSet<String>,
    pending_deleted: &mut BTreeSet<String>,
) -> usize {
    let mut count = 0;
    while let Ok(event) = rx.try_recv() {
        count += 1;
        match event {
            Ok(event) => collect_event_paths(
                root,
                options,
                known,
                &event,
                pending_changed,
                pending_deleted,
            ),
            Err(err) => {
                event_log::log_file_watch_error("watcher_event", &err.to_string());
                eprintln!("codebase-mcp watcher event error: {err:#}");
            }
        }
    }
    count
}

fn collect_event_paths(
    root: &Path,
    options: &crate::indexer::IndexOptions,
    known: &BTreeSet<String>,
    event: &Event,
    pending_changed: &mut BTreeSet<String>,
    pending_deleted: &mut BTreeSet<String>,
) {
    for path in &event.paths {
        let absolute = if path.is_absolute() {
            path.clone()
        } else {
            root.join(path)
        };
        collect_one_event_path(
            root,
            options,
            known,
            &absolute,
            &event.kind,
            pending_changed,
            pending_deleted,
        );
    }
}

fn collect_one_event_path(
    root: &Path,
    options: &crate::indexer::IndexOptions,
    known: &BTreeSet<String>,
    absolute: &Path,
    kind: &EventKind,
    pending_changed: &mut BTreeSet<String>,
    pending_deleted: &mut BTreeSet<String>,
) {
    if is_remove_kind(kind) && absolute.extension().is_none() {
        mark_deleted_prefix(root, known, absolute, pending_changed, pending_deleted);
        return;
    }

    let Some(rel) = event_relative_path(root, absolute) else {
        return;
    };
    if !is_probably_source_path(root, absolute, options) {
        if known.contains(&rel) {
            pending_changed.remove(&rel);
            pending_deleted.insert(rel);
        }
        return;
    }

    if absolute.is_file() && !is_remove_kind(kind) {
        pending_deleted.remove(&rel);
        pending_changed.insert(rel);
    } else if known.contains(&rel) || is_remove_kind(kind) {
        pending_changed.remove(&rel);
        pending_deleted.insert(rel);
    }
}

fn is_remove_kind(kind: &EventKind) -> bool {
    matches!(kind, EventKind::Remove(_))
}

fn is_probably_source_path(
    root: &Path,
    absolute: &Path,
    options: &crate::indexer::IndexOptions,
) -> bool {
    is_indexed_source_file(root, absolute, options).unwrap_or(false)
}

fn mark_deleted_prefix(
    root: &Path,
    known: &BTreeSet<String>,
    absolute: &Path,
    pending_changed: &mut BTreeSet<String>,
    pending_deleted: &mut BTreeSet<String>,
) {
    let Some(rel) = event_relative_path(root, absolute) else {
        return;
    };
    let prefix = format!("{}/", rel.trim_end_matches('/'));
    for path in known.iter().filter(|path| path.starts_with(&prefix)) {
        pending_changed.remove(path);
        pending_deleted.insert(path.clone());
    }
}

fn event_relative_path(root: &Path, path: &Path) -> Option<String> {
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(normalize_rel_path(relative.to_string_lossy().as_ref()));
    }
    let root_text = normalize_path_text(root);
    let path_text = normalize_path_text(path);
    let root_cmp = comparable_path_text(&root_text);
    let path_cmp = comparable_path_text(&path_text);
    let root_cmp = root_cmp.trim_end_matches('/');
    let root_text = root_text.trim_end_matches('/');
    if path_cmp == root_cmp {
        return Some(String::new());
    }
    path_cmp
        .strip_prefix(&format!("{root_cmp}/"))
        .map(|_| normalize_rel_path(&path_text[root_text.len() + 1..]))
}

fn normalize_path_text(path: &Path) -> String {
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

fn apply_known_delta(
    root: &Path,
    known: &mut BTreeSet<String>,
    changed: &[String],
    deleted: &[String],
) {
    for path in deleted {
        known.remove(path);
    }
    for path in changed {
        let absolute = root.join(path);
        let Ok(metadata) = fs::metadata(&absolute) else {
            known.remove(path);
            continue;
        };
        if !metadata.is_file() {
            known.remove(path);
            continue;
        }
        known.insert(path.clone());
    }
}
