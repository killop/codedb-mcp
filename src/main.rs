mod bm25;
mod cache;
mod config;
mod embedding;
mod graph;
mod indexer;
mod language;
mod mcp;
mod search;
mod source;
mod text_search;
mod tokens;
mod tools;
mod tree_sitter_lang;
mod types;
mod vector_store;
mod watcher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::AppConfig;
use indexer::{IndexOptions, collect_source_paths, normalize_rel_path};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tools::{ProjectManager, ReindexCheck, dispatch_cached_cli_tool, dispatch_tool};

#[derive(Parser, Debug)]
#[command(
    name = "codebase-mcp",
    version,
    about = "Rust MCP server for codedb-compatible tree-sitter code search"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(global = true, long, short = 'C', default_value = ".")]
    root: PathBuf,

    #[arg(global = true, long, default_value = ".codedb-mcp/codedb-mcp.toml")]
    config: PathBuf,

    #[arg(global = true, long)]
    no_watch: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    Mcp {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    Index {
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    Search {
        query: String,
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(short = 'k', long, default_value_t = 10)]
        max_results: usize,
    },
    Tool {
        name: String,
        #[arg(default_value = "{}")]
        arguments: String,
    },
    #[command(hide = true)]
    BenchIncremental {
        #[arg(default_value = ".")]
        path: PathBuf,
        #[arg(long, default_value_t = 1000)]
        count: usize,
        #[arg(long, default_value = "Assets/CodedbMcpBench")]
        bench_dir: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load(&cli.config)?;
    let options = config.index_options();
    let watch_enabled = config.watch.enabled && !cli.no_watch;
    let watch_poll_interval = Duration::from_secs(config.watch.poll_interval_seconds.max(1));

    match cli.command {
        Some(Command::Mcp { path }) => {
            let manager = Arc::new(ProjectManager::new_lazy(path, options));
            mcp::serve(manager, watch_enabled, watch_poll_interval)
        }
        Some(Command::Index { path }) => {
            let manager = ProjectManager::new(path, options)?;
            let index = manager.get(None)?;
            let stats = index.stats();
            println!(
                "indexed {}: {} files, {} chunks, {} symbols",
                stats.root, stats.files, stats.chunks, stats.symbols
            );
            Ok(())
        }
        Some(Command::Search {
            query,
            path,
            max_results,
        }) => {
            let manager = ProjectManager::new(path, options)?;
            let text = dispatch_tool(
                &manager,
                "codedb_search",
                &serde_json::json!({
                    "query": query,
                    "max_results": max_results,
                }),
            );
            print!("{text}");
            Ok(())
        }
        Some(Command::Tool { name, arguments }) => {
            let args: serde_json::Value = serde_json::from_str(&arguments)?;
            if let Some(text) = dispatch_cached_cli_tool(&cli.root, &options, &name, &args)? {
                print!("{text}");
                return Ok(());
            }
            let manager = ProjectManager::new_lazy(cli.root, options);
            let text = dispatch_tool(&manager, &name, &args);
            print!("{text}");
            Ok(())
        }
        Some(Command::BenchIncremental {
            path,
            count,
            bench_dir,
        }) => bench_incremental(path, options, count, &bench_dir),
        None => {
            let manager = Arc::new(ProjectManager::new_lazy(cli.root, options));
            mcp::serve(manager, watch_enabled, watch_poll_interval)
        }
    }
}

fn bench_incremental(
    path: PathBuf,
    options: IndexOptions,
    count: usize,
    bench_dir: &str,
) -> Result<()> {
    let root = path.canonicalize()?;
    let bench_rel = normalize_rel_path(bench_dir).trim_matches('/').to_string();
    let bench_abs = root.join(&bench_rel);
    ensure_bench_dir_is_safe(&bench_rel)?;
    if bench_abs.exists() {
        let bench_canon = bench_abs.canonicalize()?;
        if !bench_canon.starts_with(&root) {
            anyhow::bail!(
                "benchmark directory is outside root: {}",
                bench_canon.display()
            );
        }
        fs::remove_dir_all(&bench_canon)?;
    }

    let manager = ProjectManager::new_lazy(root.clone(), options.clone());
    let load_start = Instant::now();
    let initial = manager.get(None)?;
    let load_ms = load_start.elapsed().as_secs_f64() * 1000.0;

    let scan_start = Instant::now();
    let source_count = collect_source_paths(&root, &options)?.len();
    let scan_ms = scan_start.elapsed().as_secs_f64() * 1000.0;

    fs::create_dir_all(&bench_abs)?;
    let paths = (0..count)
        .map(|idx| format!("{bench_rel}/CodedbBenchFile{idx:04}.cs"))
        .collect::<Vec<_>>();

    let write_start = Instant::now();
    for (idx, rel) in paths.iter().enumerate() {
        fs::write(root.join(rel), bench_cs_source(idx, 1))?;
    }
    let write_add_ms = write_start.elapsed().as_secs_f64() * 1000.0;
    let add_ms = time_apply(&manager, paths.clone(), Vec::new())?;

    let write_start = Instant::now();
    for (idx, rel) in paths.iter().enumerate() {
        fs::write(root.join(rel), bench_cs_source(idx, 2))?;
    }
    let write_modify_ms = write_start.elapsed().as_secs_f64() * 1000.0;
    let modify_ms = time_apply(&manager, paths.clone(), Vec::new())?;

    let write_start = Instant::now();
    for rel in &paths {
        let _ = fs::remove_file(root.join(rel));
    }
    let write_delete_ms = write_start.elapsed().as_secs_f64() * 1000.0;
    let delete_ms = time_apply(&manager, Vec::new(), paths.clone())?;
    let _ = fs::remove_dir_all(&bench_abs);

    let final_stats = manager.get(None)?.stats();
    println!(
        "{}",
        json!({
            "root": root.display().to_string(),
            "bench_dir": bench_rel,
            "count": count,
            "initial_files": initial.stats().files,
            "final_files": final_stats.files,
            "source_scan_files": source_count,
            "load_ms": round_ms(load_ms),
            "source_scan_ms": round_ms(scan_ms),
            "write_add_ms": round_ms(write_add_ms),
            "apply_add_ms": round_ms(add_ms),
            "write_modify_ms": round_ms(write_modify_ms),
            "apply_modify_ms": round_ms(modify_ms),
            "write_delete_ms": round_ms(write_delete_ms),
            "apply_delete_ms": round_ms(delete_ms),
        })
    );
    Ok(())
}

fn ensure_bench_dir_is_safe(bench_rel: &str) -> Result<()> {
    if bench_rel.is_empty()
        || bench_rel.starts_with("../")
        || bench_rel.contains("/../")
        || PathBuf::from(bench_rel).is_absolute()
    {
        anyhow::bail!("unsafe benchmark directory: {bench_rel}");
    }
    let lower = bench_rel.to_ascii_lowercase();
    if !lower.contains("codedbmcpbench") && !lower.contains("codedb-mcp-bench") {
        anyhow::bail!("benchmark directory must contain CodedbMcpBench");
    }
    Ok(())
}

fn bench_cs_source(idx: usize, version: usize) -> String {
    format!(
        "namespace CodedbMcpBench {{ public sealed class CodedbBenchFile{idx:04} {{ public int Version => {version}; public string Name => nameof(CodedbBenchFile{idx:04}); public void Tick() {{ }} }} }}\n"
    )
}

fn time_apply(manager: &ProjectManager, changed: Vec<String>, deleted: Vec<String>) -> Result<f64> {
    let started = Instant::now();
    match manager.apply_default_changes(changed, deleted)? {
        ReindexCheck::Unchanged | ReindexCheck::Reindexed(_) => {}
    }
    Ok(started.elapsed().as_secs_f64() * 1000.0)
}

fn round_ms(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}
