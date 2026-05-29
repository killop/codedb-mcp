use crate::tree_sitter_lang::ParsedSource;
use crate::types::{Chunk, Scope, Symbol};

pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "cs" => Some("csharp"),
        "java" => Some("java"),
        "rs" => Some("rust"),
        "py" | "pyw" => Some("python"),
        "lua" => Some("lua"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("jsx"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "c" => Some("c"),
        "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => Some("cpp"),
        _ => None,
    }
}

pub fn analyze_source(language: &str, content: &str) -> ParsedSource {
    crate::tree_sitter_lang::analyze_source(language, content)
}

#[cfg(test)]
pub fn analyze_symbols(language: &str, content: &str) -> Vec<Symbol> {
    analyze_source(language, content).symbols
}

#[cfg(test)]
pub fn parse_namespace(language: &str, content: &str) -> Option<String> {
    analyze_source(language, content).namespace
}

#[cfg(test)]
pub fn parse_imports(language: &str, content: &str) -> Vec<String> {
    analyze_source(language, content).imports
}

#[cfg(test)]
pub fn chunk_source(
    language: &str,
    content: &str,
    file_path: &str,
    symbols: &[Symbol],
) -> Vec<Chunk> {
    crate::source::chunk_source(content, file_path, language, symbols)
}

pub fn chunk_source_metadata(
    language: &str,
    content: &str,
    file_path: &str,
    symbols: &[Symbol],
) -> Vec<Chunk> {
    crate::source::chunk_source_metadata(content, file_path, language, symbols)
}

pub fn scope_for_line(symbols: &[Symbol], line: usize) -> Option<Scope> {
    crate::source::scope_for_line(symbols, line)
}

pub fn is_comment_or_blank(line: &str) -> bool {
    crate::source::is_comment_or_blank(line)
}

pub fn regex_case_insensitive(pattern: &str) -> Result<regex::Regex, regex::Error> {
    crate::source::regex_case_insensitive(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csharp_tree_sitter_extracts_outline_imports_and_namespace() {
        let parsed = analyze_source(
            "csharp",
            r#"
using Game.Core;

namespace Game.App;

public class PlayerController
{
    public int Health { get; set; }

    public void Move()
    {
    }
}
"#,
        );

        assert_eq!(parsed.namespace.as_deref(), Some("Game.App"));
        assert!(parsed.imports.contains(&"Game.Core".to_string()));
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "class" && s.name == "PlayerController")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "property" && s.name == "Health")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "method" && s.name == "Move")
        );
    }

    #[test]
    fn java_tree_sitter_extracts_outline_imports_and_package() {
        let parsed = analyze_source(
            "java",
            r#"
package com.acme.app;

import com.acme.core.UserService;

public class App {
    private UserService service;

    public void run() {
    }
}
"#,
        );

        assert_eq!(parsed.namespace.as_deref(), Some("com.acme.app"));
        assert!(
            parsed
                .imports
                .contains(&"com.acme.core.UserService".to_string())
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "class" && s.name == "App")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "method" && s.name == "run")
        );
    }

    #[test]
    fn rust_tree_sitter_extracts_outline_and_imports() {
        let parsed = analyze_source(
            "rust",
            r#"
use std::collections::HashMap;
use crate::core::{Engine, Runner};

pub mod guide {
    pub struct GuideManager;
    pub enum GuideType {
        Main,
        Side,
    }

    pub trait Runnable {
        fn run(&self);
    }

    impl GuideManager {
        pub fn new() -> Self {
            Self
        }
    }

    pub fn start() {}
    pub type GuideId = u64;
    pub const DEFAULT_GUIDE: GuideId = 1;
    pub static ENABLED: bool = true;
    macro_rules! guide_macro {
        () => {};
    }
}
"#,
        );

        assert!(
            parsed
                .imports
                .contains(&"std.collections.HashMap".to_string())
        );
        assert!(parsed.imports.contains(&"crate.core.Engine".to_string()));
        assert!(parsed.imports.contains(&"crate.core.Runner".to_string()));
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "module" && s.name == "guide")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "struct" && s.name == "GuideManager")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "enum" && s.name == "GuideType")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "trait" && s.name == "Runnable")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "function" && s.name == "start")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "type_alias" && s.name == "GuideId")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "const" && s.name == "DEFAULT_GUIDE")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "static" && s.name == "ENABLED")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "macro" && s.name == "guide_macro")
        );
    }

    #[test]
    fn python_tree_sitter_extracts_outline_and_imports() {
        let parsed = analyze_source(
            "python",
            r#"
import os, sys as system
from app.services import user_service

class UserController:
    def handle(self):
        pass
"#,
        );

        assert!(parsed.imports.contains(&"os".to_string()));
        assert!(parsed.imports.contains(&"sys".to_string()));
        assert!(parsed.imports.contains(&"app.services".to_string()));
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "class" && s.name == "UserController")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "function" && s.name == "handle")
        );
    }

    #[test]
    fn lua_tree_sitter_extracts_outline_and_requires() {
        let parsed = analyze_source(
            "lua",
            r#"
local M = {}
local player = require("game.player")
local audio = require 'game.audio'

function M:start()
end

local function build_player()
end

M.stop = function()
end

return {
    run = function()
    end
}
"#,
        );

        assert!(parsed.imports.contains(&"game.player".to_string()));
        assert!(parsed.imports.contains(&"game.audio".to_string()));
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "method" && s.name == "start")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "function" && s.name == "build_player")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "function" && s.name == "stop")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "function" && s.name == "run")
        );
    }

    #[test]
    fn typescript_tree_sitter_extracts_outline_and_imports() {
        let parsed = analyze_source(
            "typescript",
            r#"
import { Client } from "./client";

export interface Service {
    run(): void;
}

export class GameService implements Service {
    run(): void {}
}
"#,
        );

        assert!(parsed.imports.contains(&"./client".to_string()));
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "interface" && s.name == "Service")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "class" && s.name == "GameService")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "method" && s.name == "run")
        );
    }

    #[test]
    fn cpp_tree_sitter_extracts_outline_and_includes() {
        let parsed = analyze_source(
            "cpp",
            r#"
#include <vector>

class Worker {
public:
    void run();
};

int main() {
    return 0;
}
"#,
        );

        assert!(parsed.imports.contains(&"vector".to_string()));
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "class" && s.name == "Worker")
        );
        assert!(
            parsed
                .symbols
                .iter()
                .any(|s| s.kind == "function" && s.name == "main")
        );
    }
}
