use crate::tools::ProjectManager;
use anyhow::{Context, Result};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(1500);

pub fn start_project_watcher(manager: Arc<ProjectManager>) -> Result<JoinHandle<()>> {
    let root = manager
        .default_root()
        .canonicalize()
        .context("failed to resolve watcher root")?;
    let extensions = manager
        .extensions()
        .into_iter()
        .map(|ext| ext.trim_start_matches('.').to_ascii_lowercase())
        .collect::<HashSet<_>>();

    let handle = thread::Builder::new()
        .name("codebase-mcp-watch".to_string())
        .spawn(move || {
            if let Err(err) = watch_loop(manager, root, extensions) {
                eprintln!("codebase-mcp watcher stopped: {err:#}");
            }
        })
        .context("failed to spawn watcher thread")?;
    Ok(handle)
}

fn watch_loop(
    manager: Arc<ProjectManager>,
    root: std::path::PathBuf,
    extensions: HashSet<String>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.send(result);
        },
        Config::default(),
    )?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch {}", root.display()))?;

    eprintln!(
        "codebase-mcp watching {} for: {}",
        root.display(),
        display_extensions(&extensions)
    );

    let mut pending = false;
    let mut last_event = Instant::now();

    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Ok(event)) => {
                if should_reindex(&event, &extensions) {
                    pending = true;
                    last_event = Instant::now();
                }
            }
            Ok(Err(err)) => eprintln!("codebase-mcp watcher event error: {err}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if pending && last_event.elapsed() >= DEBOUNCE {
                    pending = false;
                    let started = Instant::now();
                    match manager.default_has_content_changes() {
                        Ok(false) => {
                            eprintln!("codebase-mcp reindex skipped: content hash unchanged");
                            continue;
                        }
                        Ok(true) => {}
                        Err(err) => {
                            eprintln!("codebase-mcp change check failed, reindexing: {err:#}");
                        }
                    }
                    eprintln!("codebase-mcp reindex started after file change");
                    match manager.reindex_default() {
                        Ok(index) => {
                            let stats = index.stats();
                            eprintln!(
                                "codebase-mcp reindex ready in {:.1}s: {} files, {} chunks, {} symbols",
                                started.elapsed().as_secs_f32(),
                                stats.files,
                                stats.chunks,
                                stats.symbols
                            );
                        }
                        Err(err) => {
                            eprintln!("codebase-mcp reindex failed: {err:#}");
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Ok(())
}

fn should_reindex(event: &Event, extensions: &HashSet<String>) -> bool {
    event
        .paths
        .iter()
        .any(|path| path_has_watched_extension(path, extensions))
}

fn path_has_watched_extension(path: &Path, extensions: &HashSet<String>) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| extensions.contains(&ext.to_ascii_lowercase()))
        .unwrap_or(false)
}

fn display_extensions(extensions: &HashSet<String>) -> String {
    let mut items = extensions
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>();
    items.sort();
    items.join(", ")
}
