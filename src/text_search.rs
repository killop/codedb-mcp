use crate::types::FileEntry;
use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

pub const TEXT_INDEX_VERSION: u32 = 1;
const MAX_TRIGRAM_FILE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TextPosting {
    pub file_id: u32,
    pub next_mask: u8,
    pub loc_mask: u8,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TextLookupEntry {
    pub trigram: u32,
    pub offset: u32,
    pub len: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextSearchIndex {
    pub version: u32,
    pub source_hash: String,
    pub file_count: usize,
    pub lookup: Vec<TextLookupEntry>,
    pub postings: Vec<TextPosting>,
    pub skipped_file_ids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, Default)]
struct PostingMask {
    next_mask: u8,
    loc_mask: u8,
}

impl TextSearchIndex {
    pub fn build(
        root: &Path,
        files: &BTreeMap<String, FileEntry>,
        file_paths: &[String],
        source_hash: String,
    ) -> Result<Self> {
        let maps = file_paths
            .par_iter()
            .enumerate()
            .fold(
                HashMap::<u32, Vec<TextPosting>>::new,
                |mut local_index, (file_id, rel_path)| {
                    let Some(file) = files.get(rel_path) else {
                        return local_index;
                    };
                    if file.byte_size > MAX_TRIGRAM_FILE_BYTES {
                        return local_index;
                    }
                    let Ok(bytes) = fs::read(root.join(rel_path)) else {
                        return local_index;
                    };
                    let trigrams = extract_file_trigrams(&bytes);
                    for (trigram, mask) in trigrams {
                        local_index.entry(trigram).or_default().push(TextPosting {
                            file_id: file_id as u32,
                            next_mask: mask.next_mask,
                            loc_mask: mask.loc_mask,
                        });
                    }
                    local_index
                },
            )
            .reduce(
                HashMap::<u32, Vec<TextPosting>>::new,
                |mut left, right| {
                    for (trigram, mut postings) in right {
                        left.entry(trigram).or_default().append(&mut postings);
                    }
                    left
                },
            );

        let mut keys = maps.keys().copied().collect::<Vec<_>>();
        keys.sort_unstable();
        let mut lookup = Vec::with_capacity(keys.len());
        let total_postings = maps.values().map(Vec::len).sum();
        let mut postings = Vec::with_capacity(total_postings);
        for trigram in keys {
            let start = postings.len() as u32;
            let Some(mut list) = maps.get(&trigram).cloned() else {
                continue;
            };
            list.sort_unstable_by_key(|posting| posting.file_id);
            list.dedup_by(|a, b| {
                if a.file_id == b.file_id {
                    b.next_mask |= a.next_mask;
                    b.loc_mask |= a.loc_mask;
                    true
                } else {
                    false
                }
            });
            let len = list.len() as u32;
            postings.extend(list);
            lookup.push(TextLookupEntry {
                trigram,
                offset: start,
                len,
            });
        }

        let skipped_file_ids = file_paths
            .iter()
            .enumerate()
            .filter_map(|(file_id, rel_path)| {
                files
                    .get(rel_path)
                    .is_some_and(|file| file.byte_size > MAX_TRIGRAM_FILE_BYTES)
                    .then_some(file_id as u32)
            })
            .collect();

        Ok(Self {
            version: TEXT_INDEX_VERSION,
            source_hash,
            file_count: file_paths.len(),
            lookup,
            postings,
            skipped_file_ids,
        })
    }

    pub fn validate(&self, source_hash: &str, file_count: usize) -> bool {
        self.version == TEXT_INDEX_VERSION
            && self.source_hash == source_hash
            && self.file_count == file_count
    }

    pub fn candidate_file_ids(&self, query: &str) -> Option<Vec<u32>> {
        let trigrams = query_trigrams(query);
        self.candidate_file_ids_from_trigrams(&trigrams)
    }

    pub fn regex_candidate_file_ids(&self, pattern: &str) -> Option<Vec<u32>> {
        let trigrams = regex_literal_trigrams(pattern);
        self.candidate_file_ids_from_trigrams(&trigrams)
    }

    fn candidate_file_ids_from_trigrams(&self, trigrams: &[u32]) -> Option<Vec<u32>> {
        if trigrams.is_empty() {
            return None;
        }
        let mut posting_lists = Vec::with_capacity(trigrams.len());
        for trigram in trigrams {
            let postings = self.postings_for(*trigram)?;
            if postings.is_empty() {
                return Some(Vec::new());
            }
            posting_lists.push((*trigram, postings));
        }
        posting_lists.sort_unstable_by_key(|(_, postings)| postings.len());

        let mut result = posting_lists[0]
            .1
            .iter()
            .map(|posting| posting.file_id)
            .collect::<Vec<_>>();
        for (_, postings) in posting_lists.iter().skip(1) {
            intersect_sorted_ids(&mut result, postings);
            if result.is_empty() {
                return Some(result);
            }
        }

        Some(result)
    }

    fn postings_for(&self, trigram: u32) -> Option<&[TextPosting]> {
        let idx = self
            .lookup
            .binary_search_by_key(&trigram, |entry| entry.trigram)
            .ok()?;
        let entry = self.lookup[idx];
        let start = entry.offset as usize;
        let end = start + entry.len as usize;
        self.postings.get(start..end)
    }

}

pub fn source_hash(file_paths: &[String], files: &BTreeMap<String, FileEntry>) -> String {
    let mut hasher = blake3::Hasher::new();
    for path in file_paths {
        hasher.update(path.as_bytes());
        hasher.update(&[0]);
        if let Some(file) = files.get(path) {
            hasher.update(file.content_hash.as_bytes());
            hasher.update(&file.byte_size.to_le_bytes());
        }
        hasher.update(&[0xff]);
    }
    hasher.finalize().to_hex().to_string()
}

pub fn read_text_search_index(
    path: &Path,
    source_hash: &str,
    file_count: usize,
) -> Result<Option<TextSearchIndex>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read text search index {}", path.display()))?;
    let index: TextSearchIndex = bincode::deserialize(&bytes)
        .with_context(|| format!("failed to decode text search index {}", path.display()))?;
    if index.validate(source_hash, file_count) {
        Ok(Some(index))
    } else {
        Ok(None)
    }
}

pub fn write_text_search_index(path: &Path, index: &TextSearchIndex) -> Result<()> {
    let bytes = bincode::serialize(index).context("failed to encode text search index")?;
    let tmp_path = path.with_extension("bin.tmp");
    fs::write(&tmp_path, bytes)
        .with_context(|| format!("failed to write text search index {}", tmp_path.display()))?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp_path, path)
        .with_context(|| format!("failed to install text search index {}", path.display()))?;
    Ok(())
}

fn extract_file_trigrams(bytes: &[u8]) -> HashMap<u32, PostingMask> {
    let mut trigrams = HashMap::with_capacity((bytes.len() / 4).clamp(64, 65_536));
    if bytes.len() < 3 {
        return trigrams;
    }
    for idx in 0..bytes.len() - 2 {
        let a = bytes[idx];
        let b = bytes[idx + 1];
        let c = bytes[idx + 2];
        if is_ascii_whitespace(a) && is_ascii_whitespace(b) && is_ascii_whitespace(c) {
            continue;
        }
        let entry = trigrams.entry(pack_trigram(a, b, c)).or_default();
        entry.loc_mask |= 1u8 << (idx % 8);
        if idx + 3 < bytes.len() {
            entry.next_mask |= 1u8 << (normalize_byte(bytes[idx + 3]) % 8);
        }
    }
    trigrams
}

fn query_trigrams(query: &str) -> Vec<u32> {
    unique_trigrams(query.as_bytes())
}

fn regex_literal_trigrams(pattern: &str) -> Vec<u32> {
    if pattern.contains('|')
        || pattern.contains('?')
        || pattern.contains('*')
        || pattern.contains('{')
        || pattern.contains('}')
    {
        return Vec::new();
    }
    let mut literals = Vec::new();
    let mut current = Vec::new();
    let mut escaped = false;
    for byte in pattern.bytes() {
        if escaped {
            if is_regex_literal_escape(byte) {
                current.push(byte);
            } else {
                push_current_literal(&mut literals, &mut current);
            }
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
            continue;
        }
        if is_regex_meta(byte) {
            push_current_literal(&mut literals, &mut current);
        } else {
            current.push(byte);
        }
    }
    push_current_literal(&mut literals, &mut current);

    let mut set = HashSet::new();
    for literal in literals {
        for trigram in unique_trigrams(&literal) {
            set.insert(trigram);
        }
    }
    let mut trigrams = set.into_iter().collect::<Vec<_>>();
    trigrams.sort_unstable();
    trigrams
}

fn unique_trigrams(bytes: &[u8]) -> Vec<u32> {
    if bytes.len() < 3 {
        return Vec::new();
    }
    let mut set = HashSet::with_capacity(bytes.len().saturating_sub(2));
    for idx in 0..bytes.len() - 2 {
        let a = bytes[idx];
        let b = bytes[idx + 1];
        let c = bytes[idx + 2];
        if is_ascii_whitespace(a) && is_ascii_whitespace(b) && is_ascii_whitespace(c) {
            continue;
        }
        set.insert(pack_trigram(a, b, c));
    }
    let mut trigrams = set.into_iter().collect::<Vec<_>>();
    trigrams.sort_unstable();
    trigrams
}

fn push_current_literal(literals: &mut Vec<Vec<u8>>, current: &mut Vec<u8>) {
    if current.len() >= 3 {
        literals.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn intersect_sorted_ids(result: &mut Vec<u32>, postings: &[TextPosting]) {
    let mut write = 0usize;
    let mut posting_idx = 0usize;
    let original_len = result.len();
    for read in 0..original_len {
        let id = result[read];
        while posting_idx < postings.len() && postings[posting_idx].file_id < id {
            posting_idx += 1;
        }
        if posting_idx < postings.len() && postings[posting_idx].file_id == id {
            result[write] = id;
            write += 1;
            posting_idx += 1;
        }
    }
    result.truncate(write);
}

fn pack_trigram(a: u8, b: u8, c: u8) -> u32 {
    pack_normalized(normalize_byte(a), normalize_byte(b), normalize_byte(c))
}

fn pack_normalized(a: u8, b: u8, c: u8) -> u32 {
    ((a as u32) << 16) | ((b as u32) << 8) | c as u32
}

fn normalize_byte(byte: u8) -> u8 {
    byte.to_ascii_lowercase()
}

fn is_ascii_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

fn is_regex_meta(byte: u8) -> bool {
    matches!(
        byte,
        b'.' | b'*'
            | b'+'
            | b'?'
            | b'('
            | b')'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'^'
            | b'$'
            | b'|'
    )
}

fn is_regex_literal_escape(byte: u8) -> bool {
    matches!(
        byte,
        b'\\' | b'.'
            | b'*'
            | b'+'
            | b'?'
            | b'('
            | b')'
            | b'['
            | b']'
            | b'{'
            | b'}'
            | b'^'
            | b'$'
            | b'|'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigram_candidates_intersect() {
        let mut files = BTreeMap::new();
        files.insert(
            "a.cs".to_string(),
            FileEntry {
                path: "a.cs".to_string(),
                language: crate::types::LanguageId::CSharp,
                line_count: 1,
                byte_size: 18,
                modified_unix_ms: 0,
                content_hash: "a".to_string(),
                namespace: None,
                imports: Vec::new(),
                symbols: Vec::new(),
                content: String::new(),
            },
        );
        files.insert(
            "b.cs".to_string(),
            FileEntry {
                path: "b.cs".to_string(),
                language: crate::types::LanguageId::CSharp,
                line_count: 1,
                byte_size: 18,
                modified_unix_ms: 0,
                content_hash: "b".to_string(),
                namespace: None,
                imports: Vec::new(),
                symbols: Vec::new(),
                content: String::new(),
            },
        );
        let temp = std::env::temp_dir().join(format!(
            "codebase-mcp-text-search-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("a.cs"), "class PoolManager {}").unwrap();
        fs::write(temp.join("b.cs"), "class InputManager {}").unwrap();
        let paths = vec!["a.cs".to_string(), "b.cs".to_string()];
        let index = TextSearchIndex::build(&temp, &files, &paths, "hash".to_string()).unwrap();
        let hits = index.candidate_file_ids("PoolManager").unwrap();
        assert_eq!(hits, vec![0]);
        let _ = fs::remove_dir_all(&temp);
    }
}
