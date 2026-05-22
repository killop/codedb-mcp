use crate::types::Symbol;
use std::collections::BTreeSet;
use tree_sitter::{Language, Node, Parser};

#[derive(Debug, Clone, Default)]
pub struct ParsedSource {
    pub namespace: Option<String>,
    pub imports: Vec<String>,
    pub symbols: Vec<Symbol>,
}

pub fn analyze_source(language: &str, content: &str) -> ParsedSource {
    let Some(grammar) = grammar(language) else {
        return ParsedSource::default();
    };
    let mut parser = Parser::new();
    if parser.set_language(&grammar).is_err() {
        return ParsedSource::default();
    }
    let Some(tree) = parser.parse(content, None) else {
        return ParsedSource::default();
    };

    let lines = content.lines().collect::<Vec<_>>();
    let mut context = ParseContext {
        language,
        content,
        lines: &lines,
        namespace: None,
        imports: BTreeSet::new(),
        symbols: Vec::new(),
    };
    visit(tree.root_node(), &mut context);
    normalize_symbol_ranges(&mut context.symbols, lines.len().max(1));

    ParsedSource {
        namespace: context.namespace,
        imports: context.imports.into_iter().collect(),
        symbols: context.symbols,
    }
}

fn grammar(language: &str) -> Option<Language> {
    match language {
        "csharp" => Some(tree_sitter_c_sharp::LANGUAGE.into()),
        "java" => Some(tree_sitter_java::LANGUAGE.into()),
        "python" => Some(tree_sitter_python::LANGUAGE.into()),
        "javascript" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "typescript" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        "c" => Some(tree_sitter_c::LANGUAGE.into()),
        "cpp" => Some(tree_sitter_cpp::LANGUAGE.into()),
        _ => None,
    }
}

struct ParseContext<'a> {
    language: &'a str,
    content: &'a str,
    lines: &'a [&'a str],
    namespace: Option<String>,
    imports: BTreeSet<String>,
    symbols: Vec<Symbol>,
}

fn visit(node: Node<'_>, context: &mut ParseContext<'_>) {
    collect_namespace_or_import(node, context);
    let skip_children = if let Some(symbol) = symbol_from_node(node, context) {
        let skip_children = skip_symbol_children(&symbol.kind);
        context.symbols.push(symbol);
        skip_children
    } else {
        false
    };
    if skip_children {
        return;
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, context);
    }
}

fn skip_symbol_children(kind: &str) -> bool {
    matches!(
        kind,
        "method" | "constructor" | "function" | "property" | "field" | "delegate" | "event"
    )
}

fn collect_namespace_or_import(node: Node<'_>, context: &mut ParseContext<'_>) {
    let kind = node.kind();
    match context.language {
        "csharp" => match kind {
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                set_namespace_from_name_field(node, context);
            }
            "using_directive" => {
                if let Some(import) = extract_csharp_using(node_text(node, context.content)) {
                    context.imports.insert(import);
                }
            }
            _ => {}
        },
        "java" => match kind {
            "package_declaration" => {
                if context.namespace.is_none() {
                    context.namespace = extract_java_package(node_text(node, context.content));
                }
            }
            "import_declaration" => {
                if let Some(import) = extract_java_import(node_text(node, context.content)) {
                    context.imports.insert(import);
                }
            }
            _ => {}
        },
        "python" => {
            if matches!(
                kind,
                "import_statement" | "import_from_statement" | "future_import_statement"
            ) {
                for import in extract_python_imports(node_text(node, context.content)) {
                    context.imports.insert(import);
                }
            }
        }
        "javascript" | "jsx" | "typescript" | "tsx" => {
            if matches!(kind, "import_statement" | "export_statement")
                && let Some(module) = extract_quoted_module(node_text(node, context.content))
            {
                context.imports.insert(module);
            }
        }
        "c" | "cpp" => {
            if kind == "preproc_include"
                && let Some(include) = extract_include(node_text(node, context.content))
            {
                context.imports.insert(include);
            }
        }
        _ => {}
    }
}

fn set_namespace_from_name_field(node: Node<'_>, context: &mut ParseContext<'_>) {
    if context.namespace.is_some() {
        return;
    }
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    context.namespace = node_text(name_node, context.content).and_then(clean_qualified_name);
}

fn symbol_from_node(node: Node<'_>, context: &ParseContext<'_>) -> Option<Symbol> {
    let kind = symbol_kind(context.language, node.kind())?;
    let name_node = symbol_name_node(node)?;
    let name = node_text(name_node, context.content).and_then(clean_symbol_name)?;
    Some(Symbol {
        name,
        kind: kind.to_string(),
        line_start: node.start_position().row + 1,
        line_end: node.end_position().row + 1,
        detail: detail_for_node(node, context.lines),
    })
}

fn symbol_kind(language: &str, node_kind: &str) -> Option<&'static str> {
    match language {
        "csharp" => match node_kind {
            "class_declaration" => Some("class"),
            "interface_declaration" => Some("interface"),
            "struct_declaration" => Some("struct"),
            "enum_declaration" => Some("enum"),
            "record_declaration" => Some("record"),
            "method_declaration" => Some("method"),
            "constructor_declaration" => Some("constructor"),
            "property_declaration" => Some("property"),
            "field_declaration" => Some("field"),
            "delegate_declaration" => Some("delegate"),
            "event_declaration" | "event_field_declaration" => Some("event"),
            _ => None,
        },
        "java" => match node_kind {
            "class_declaration" => Some("class"),
            "interface_declaration" => Some("interface"),
            "enum_declaration" => Some("enum"),
            "record_declaration" => Some("record"),
            "annotation_type_declaration" => Some("annotation"),
            "method_declaration" => Some("method"),
            "constructor_declaration" => Some("constructor"),
            "field_declaration" => Some("field"),
            _ => None,
        },
        "python" => match node_kind {
            "class_definition" => Some("class"),
            "function_definition" => Some("function"),
            "type_alias_statement" => Some("type_alias"),
            _ => None,
        },
        "javascript" | "jsx" => match node_kind {
            "class_declaration" => Some("class"),
            "function_declaration" | "generator_function_declaration" => Some("function"),
            "method_definition" => Some("method"),
            _ => None,
        },
        "typescript" | "tsx" => match node_kind {
            "class_declaration" | "abstract_class_declaration" => Some("class"),
            "interface_declaration" => Some("interface"),
            "type_alias_declaration" => Some("type_alias"),
            "enum_declaration" => Some("enum"),
            "function_declaration" | "generator_function_declaration" | "function_signature" => {
                Some("function")
            }
            "method_definition" | "method_signature" => Some("method"),
            _ => None,
        },
        "c" => match node_kind {
            "function_definition" => Some("function"),
            "struct_specifier" => Some("struct"),
            "union_specifier" => Some("union"),
            "enum_specifier" => Some("enum"),
            _ => None,
        },
        "cpp" => match node_kind {
            "function_definition" => Some("function"),
            "class_specifier" => Some("class"),
            "struct_specifier" => Some("struct"),
            "union_specifier" => Some("union"),
            "enum_specifier" => Some("enum"),
            _ => None,
        },
        _ => None,
    }
}

fn symbol_name_node(node: Node<'_>) -> Option<Node<'_>> {
    if matches!(node.kind(), "field_declaration" | "lexical_declaration") {
        return first_variable_declarator_name(node);
    }
    if node.kind() == "function_definition" {
        if let Some(declarator) = node.child_by_field_name("declarator") {
            return find_identifier_like(declarator);
        }
    }
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("declarator"))
        .and_then(find_identifier_like)
}

fn first_variable_declarator_name(node: Node<'_>) -> Option<Node<'_>> {
    if matches!(node.kind(), "variable_declarator" | "variable_declaration") {
        return node
            .child_by_field_name("name")
            .or_else(|| node.child_by_field_name("declarator"))
            .and_then(find_identifier_like);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = first_variable_declarator_name(child) {
            return Some(found);
        }
    }
    None
}

fn find_identifier_like(node: Node<'_>) -> Option<Node<'_>> {
    if is_identifier_like(node.kind()) {
        return Some(node);
    }
    if let Some(name) = node.child_by_field_name("name")
        && let Some(found) = find_identifier_like(name)
    {
        return Some(found);
    }
    if let Some(declarator) = node.child_by_field_name("declarator")
        && let Some(found) = find_identifier_like(declarator)
    {
        return Some(found);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if let Some(found) = find_identifier_like(child) {
            return Some(found);
        }
    }
    None
}

fn is_identifier_like(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "field_identifier"
            | "property_identifier"
            | "shorthand_property_identifier"
            | "statement_identifier"
            | "destructor_name"
            | "operator_name"
    )
}

fn normalize_symbol_ranges(symbols: &mut Vec<Symbol>, line_count: usize) {
    symbols.sort_by(|a, b| {
        a.line_start
            .cmp(&b.line_start)
            .then_with(|| a.line_end.cmp(&b.line_end))
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
    });
    symbols.dedup_by(|a, b| a.name == b.name && a.kind == b.kind && a.line_start == b.line_start);
    for idx in 0..symbols.len() {
        let start = symbols[idx].line_start;
        let mut end = symbols[idx].line_end.min(line_count).max(start);
        if let Some(next) = symbols.get(idx + 1) {
            end = if next.line_start > start {
                end.min(next.line_start - 1).max(start)
            } else {
                start
            };
        }
        symbols[idx].line_end = end;
    }
}

fn node_text(node: Node<'_>, content: &str) -> Option<String> {
    node.utf8_text(content.as_bytes())
        .ok()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn detail_for_node(node: Node<'_>, lines: &[&str]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let start = node.start_position().row.min(lines.len() - 1);
    let end = node.end_position().row.min(lines.len() - 1);
    for line in &lines[start..=end] {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    lines[start].trim().to_string()
}

fn extract_csharp_using(text: Option<String>) -> Option<String> {
    let text = text?;
    let mut value = strip_prefix_word(&text, "global").unwrap_or(text.as_str());
    value = strip_prefix_word(value, "using")?;
    value = strip_prefix_word(value, "static").unwrap_or(value);
    if value.contains('=') {
        return None;
    }
    clean_qualified_name(value.trim_end_matches(';'))
}

fn extract_java_package(text: Option<String>) -> Option<String> {
    let text = text?;
    let value = strip_prefix_word(&text, "package")?;
    clean_qualified_name(value.trim_end_matches(';'))
}

fn extract_java_import(text: Option<String>) -> Option<String> {
    let text = text?;
    let value = strip_prefix_word(&text, "import")?;
    let value = strip_prefix_word(value, "static").unwrap_or(value);
    clean_qualified_name(value.trim_end_matches(';'))
}

fn extract_python_imports(text: Option<String>) -> Vec<String> {
    let Some(text) = text else {
        return Vec::new();
    };
    let trimmed = text.trim();
    if let Some(rest) = strip_prefix_word(trimmed, "import") {
        return split_import_items(rest);
    }
    if let Some(rest) = strip_prefix_word(trimmed, "from") {
        let module = rest
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches('.');
        if !module.is_empty() {
            return vec![module.to_string()];
        }
    }
    Vec::new()
}

fn split_import_items(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter_map(|item| {
            let name = item
                .trim()
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches('.');
            (!name.is_empty()).then(|| name.to_string())
        })
        .collect()
}

fn extract_quoted_module(text: Option<String>) -> Option<String> {
    let text = text?;
    for quote in ['"', '\''] {
        let Some(start) = text.find(quote) else {
            continue;
        };
        let rest = &text[start + quote.len_utf8()..];
        if let Some(end) = rest.find(quote) {
            let module = rest[..end].trim();
            if !module.is_empty() {
                return Some(module.to_string());
            }
        }
    }
    None
}

fn extract_include(text: Option<String>) -> Option<String> {
    let text = text?;
    if let Some(start) = text.find('<')
        && let Some(end) = text[start + 1..].find('>')
    {
        return Some(text[start + 1..start + 1 + end].trim().to_string());
    }
    extract_quoted_module(Some(text))
}

fn strip_prefix_word<'a>(value: &'a str, word: &str) -> Option<&'a str> {
    let trimmed = value.trim_start();
    let rest = trimmed.strip_prefix(word)?;
    if rest
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(rest.trim_start())
}

fn clean_qualified_name(value: impl AsRef<str>) -> Option<String> {
    let normalized = value
        .as_ref()
        .replace("::", ".")
        .replace(char::is_whitespace, "")
        .trim_matches(';')
        .trim_matches('{')
        .trim_matches('}')
        .trim_matches('.')
        .trim_start_matches("global.")
        .to_string();
    (!normalized.is_empty()).then_some(normalized)
}

fn clean_symbol_name(value: impl AsRef<str>) -> Option<String> {
    let mut text = value.as_ref().trim();
    if text.is_empty() {
        return None;
    }
    if let Some((_, suffix)) = text.rsplit_once("::") {
        text = suffix;
    }
    if let Some((_, suffix)) = text.rsplit_once('.') {
        text = suffix;
    }
    let text = text
        .split(['<', '(', '[', ':', ';', '=', ','])
        .next()
        .unwrap_or_default()
        .trim();
    let name = text
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$' | '~'))
        .collect::<String>();
    (!name.is_empty()).then_some(name)
}
