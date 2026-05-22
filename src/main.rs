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
mod tokens;
mod tools;
mod tree_sitter_lang;
mod types;
mod vector_store;
mod watcher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::AppConfig;
use std::path::PathBuf;
use std::sync::Arc;
use tools::{ProjectManager, dispatch_tool};

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = AppConfig::load(&cli.config)?;
    let options = config.index_options();
    let watch_enabled = config.watch.enabled && !cli.no_watch;

    match cli.command {
        Some(Command::Mcp { path }) => {
            let manager = Arc::new(ProjectManager::new_lazy(path, options));
            mcp::serve(manager, watch_enabled)
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
            let manager = ProjectManager::new(cli.root, options)?;
            let args: serde_json::Value = serde_json::from_str(&arguments)?;
            let text = dispatch_tool(&manager, &name, &args);
            print!("{text}");
            Ok(())
        }
        None => {
            let manager = Arc::new(ProjectManager::new_lazy(cli.root, options));
            mcp::serve(manager, watch_enabled)
        }
    }
}
