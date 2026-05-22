use crate::tree_sitter_lang::ParsedSource;
use crate::types::{Chunk, Scope, Symbol};

pub fn language_for_extension(ext: &str) -> Option<&'static str> {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "cs" => Some("csharp"),
        "java" => Some("java"),
        "py" | "pyw" => Some("python"),
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

pub fn chunk_source(
    language: &str,
    content: &str,
    file_path: &str,
    symbols: &[Symbol],
) -> Vec<Chunk> {
    crate::source::chunk_source(content, file_path, language, symbols)
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
