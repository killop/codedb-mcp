use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub line_start: usize,
    pub line_end: usize,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub language: String,
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
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub language: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticUnit {
    pub id: usize,
    pub file_path: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordHit {
    pub path: String,
    pub line: usize,
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
