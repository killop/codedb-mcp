use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const WORD_HIT_BYTES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SymbolKind {
    Class,
    Interface,
    Struct,
    Enum,
    Record,
    Method,
    Constructor,
    Property,
    Field,
    Function,
    Module,
    Trait,
    Impl,
    TypeAlias,
    Const,
    Static,
    Macro,
    Variable,
    Symbol,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Class => "class",
            Self::Interface => "interface",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Record => "record",
            Self::Method => "method",
            Self::Constructor => "constructor",
            Self::Property => "property",
            Self::Field => "field",
            Self::Function => "function",
            Self::Module => "module",
            Self::Trait => "trait",
            Self::Impl => "impl",
            Self::TypeAlias => "type_alias",
            Self::Const => "const",
            Self::Static => "static",
            Self::Macro => "macro",
            Self::Variable => "variable",
            Self::Symbol => "symbol",
        }
    }
}

impl From<&str> for SymbolKind {
    fn from(value: &str) -> Self {
        match value {
            "class" => Self::Class,
            "interface" => Self::Interface,
            "struct" => Self::Struct,
            "enum" => Self::Enum,
            "record" => Self::Record,
            "method" => Self::Method,
            "constructor" => Self::Constructor,
            "property" => Self::Property,
            "field" => Self::Field,
            "function" => Self::Function,
            "module" => Self::Module,
            "trait" => Self::Trait,
            "impl" => Self::Impl,
            "type_alias" => Self::TypeAlias,
            "const" => Self::Const,
            "static" => Self::Static,
            "macro" => Self::Macro,
            "variable" => Self::Variable,
            _ => Self::Symbol,
        }
    }
}

impl PartialEq<&str> for SymbolKind {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for SymbolKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for SymbolKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from(value.as_str()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageId {
    CSharp,
    Java,
    Rust,
    Python,
    Lua,
    JavaScript,
    Jsx,
    TypeScript,
    Tsx,
    C,
    Cpp,
    Unknown,
}

impl LanguageId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CSharp => "csharp",
            Self::Java => "java",
            Self::Rust => "rust",
            Self::Python => "python",
            Self::Lua => "lua",
            Self::JavaScript => "javascript",
            Self::Jsx => "jsx",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Unknown => "unknown",
        }
    }
}

impl From<&str> for LanguageId {
    fn from(value: &str) -> Self {
        match value {
            "csharp" => Self::CSharp,
            "java" => Self::Java,
            "rust" => Self::Rust,
            "python" => Self::Python,
            "lua" => Self::Lua,
            "javascript" => Self::JavaScript,
            "jsx" => Self::Jsx,
            "typescript" => Self::TypeScript,
            "tsx" => Self::Tsx,
            "c" => Self::C,
            "cpp" => Self::Cpp,
            _ => Self::Unknown,
        }
    }
}

impl PartialEq<&str> for LanguageId {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Display for LanguageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for LanguageId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LanguageId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from(value.as_str()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line_start: usize,
    pub line_end: usize,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub language: LanguageId,
    pub line_count: usize,
    pub byte_size: usize,
    pub modified_unix_ms: i128,
    pub content_hash: String,
    pub namespace: Option<String>,
    pub imports: Vec<String>,
    pub symbols: Vec<Symbol>,
    #[serde(skip)]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: usize,
    #[serde(default)]
    pub file_id: u32,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub language: LanguageId,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticUnit {
    pub id: usize,
    pub file_path: String,
    pub text: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WordHit {
    pub file_id: u32,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct WordHitRange {
    pub start: u32,
    pub len: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WordIndex {
    ranges: HashMap<String, WordHitRange>,
    #[serde(skip)]
    hits: Vec<WordHit>,
    #[serde(skip)]
    hits_path: Option<PathBuf>,
}

impl WordIndex {
    pub fn from_map(mut map: HashMap<String, Vec<WordHit>>) -> Self {
        let mut keys = map.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        let mut ranges = HashMap::with_capacity(keys.len());
        let total_hits = map.values().map(Vec::len).sum();
        let mut hits = Vec::with_capacity(total_hits);
        for key in keys {
            let start = hits.len() as u32;
            if let Some(mut values) = map.remove(&key) {
                values.sort_by(|a, b| a.file_id.cmp(&b.file_id).then_with(|| a.line.cmp(&b.line)));
                let len = values.len() as u32;
                hits.extend(values);
                ranges.insert(key, WordHitRange { start, len });
            }
        }
        Self {
            ranges,
            hits,
            hits_path: None,
        }
    }

    pub fn hits(&self, word: &str) -> Result<Vec<WordHit>> {
        let Some(range) = self.ranges.get(word) else {
            return Ok(Vec::new());
        };
        let start = range.start as usize;
        let end = start + range.len as usize;
        if let Some(hits) = self.hits.get(start..end) {
            return Ok(hits.to_vec());
        }
        self.read_hits(start, end)
    }

    pub fn write_hits(&self, path: &Path) -> Result<()> {
        let file = File::create(path)
            .with_context(|| format!("failed to create word hits {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        for hit in &self.hits {
            writer.write_all(&hit.file_id.to_le_bytes())?;
            writer.write_all(&hit.line.to_le_bytes())?;
        }
        writer.flush()?;
        Ok(())
    }

    pub fn use_hits_file(&mut self, path: PathBuf) {
        self.hits.clear();
        self.hits.shrink_to_fit();
        self.hits_path = Some(path);
    }

    fn read_hits(&self, start: usize, end: usize) -> Result<Vec<WordHit>> {
        let Some(path) = &self.hits_path else {
            return Ok(Vec::new());
        };
        let count = end.saturating_sub(start);
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut file = File::open(path)
            .with_context(|| format!("failed to open word hits {}", path.display()))?;
        file.seek(SeekFrom::Start((start * WORD_HIT_BYTES) as u64))?;
        let mut bytes = vec![0u8; count * WORD_HIT_BYTES];
        file.read_exact(&mut bytes)?;
        let mut hits = Vec::with_capacity(count);
        for item in bytes.chunks_exact(WORD_HIT_BYTES) {
            hits.push(WordHit {
                file_id: u32::from_le_bytes(item[0..4].try_into().expect("word hit file id bytes")),
                line: u32::from_le_bytes(item[4..8].try_into().expect("word hit line bytes")),
            });
        }
        Ok(hits)
    }
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub name: String,
    pub kind: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: String,
    pub line: usize,
    pub text: String,
    pub scope: Option<Scope>,
}

#[derive(Debug, Clone)]
pub struct ChunkSearchHit {
    pub chunk: Chunk,
    pub score: f32,
    pub source: &'static str,
}
