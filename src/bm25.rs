use crate::tokens::tokenize;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const POSTING_BYTES: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Posting {
    doc_id: u32,
    term_freq: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct PostingRange {
    start: u32,
    len: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Bm25Index {
    doc_lens: Vec<u32>,
    ranges: HashMap<String, PostingRange>,
    #[serde(skip)]
    postings: Vec<Posting>,
    #[serde(skip)]
    postings_path: Option<PathBuf>,
    avg_doc_len: f32,
    doc_count: usize,
}

impl Bm25Index {
    pub fn new(documents: impl IntoIterator<Item = Vec<String>>) -> Self {
        let mut postings_by_term: HashMap<String, Vec<Posting>> = HashMap::new();
        let mut doc_lens = Vec::new();
        let mut total_len = 0usize;
        for (doc_id, doc) in documents.into_iter().enumerate() {
            total_len += doc.len();
            doc_lens.push(doc.len() as u32);
            let mut term_freqs: HashMap<String, u32> = HashMap::new();
            for token in doc {
                *term_freqs.entry(token).or_default() += 1;
            }
            for (token, term_freq) in term_freqs {
                postings_by_term.entry(token).or_default().push(Posting {
                    doc_id: doc_id as u32,
                    term_freq,
                });
            }
        }
        let doc_count = doc_lens.len();
        let avg_doc_len = if doc_count == 0 {
            0.0
        } else {
            total_len as f32 / doc_count as f32
        };
        let mut keys = postings_by_term.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        let posting_count = postings_by_term.values().map(Vec::len).sum();
        let mut ranges = HashMap::with_capacity(keys.len());
        let mut postings = Vec::with_capacity(posting_count);
        for key in keys {
            let start = postings.len() as u32;
            if let Some(mut values) = postings_by_term.remove(&key) {
                values.sort_by_key(|posting| posting.doc_id);
                let len = values.len() as u32;
                postings.extend(values);
                ranges.insert(key, PostingRange { start, len });
            }
        }
        Self {
            doc_lens,
            ranges,
            postings,
            postings_path: None,
            avg_doc_len,
            doc_count,
        }
    }

    pub fn query(
        &self,
        query: &str,
        top_k: usize,
        selector: Option<&[usize]>,
    ) -> Result<Vec<(usize, f32)>> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() || self.doc_count == 0 {
            return Ok(Vec::new());
        }

        let mut scores = vec![0.0f32; self.doc_count];
        let mut touched = Vec::new();
        for token in query_tokens {
            let Some(range) = self.ranges.get(&token) else {
                continue;
            };
            let start = range.start as usize;
            let end = start + range.len as usize;
            let postings = self.read_postings(start, end)?;
            let df = postings.len() as f32;
            let idf = ((self.doc_count as f32 - df + 0.5) / (df + 0.5) + 1.0).ln();
            for posting in &postings {
                let doc_id = posting.doc_id as usize;
                if selector.is_some_and(|indices| indices.binary_search(&doc_id).is_err()) {
                    continue;
                }
                let score = self.term_score(doc_id, posting.term_freq, idf);
                if score <= 0.0 {
                    continue;
                }
                if scores[doc_id] == 0.0 {
                    touched.push(doc_id);
                }
                scores[doc_id] += score;
            }
        }

        let mut ranked = touched
            .into_iter()
            .map(|idx| (idx, scores[idx]))
            .filter(|(_, score)| *score > 0.0)
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(top_k.min(ranked.len()));
        Ok(ranked)
    }

    fn term_score(&self, doc_id: usize, term_freq: u32, idf: f32) -> f32 {
        let k1 = 1.5f32;
        let b = 0.75f32;
        let freq = term_freq as f32;
        let doc_len = self.doc_lens.get(doc_id).copied().unwrap_or_default() as f32;
        let denom = freq + k1 * (1.0 - b + b * doc_len / self.avg_doc_len.max(1.0));
        idf * (freq * (k1 + 1.0)) / denom
    }

    pub fn write_postings(&self, path: &Path) -> Result<()> {
        let file = File::create(path)
            .with_context(|| format!("failed to create BM25 postings {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        for posting in &self.postings {
            writer.write_all(&posting.doc_id.to_le_bytes())?;
            writer.write_all(&posting.term_freq.to_le_bytes())?;
        }
        writer.flush()?;
        Ok(())
    }

    pub fn use_postings_file(&mut self, path: PathBuf) {
        self.postings.clear();
        self.postings.shrink_to_fit();
        self.postings_path = Some(path);
    }

    fn read_postings(&self, start: usize, end: usize) -> Result<Vec<Posting>> {
        if let Some(postings) = self.postings.get(start..end) {
            return Ok(postings.to_vec());
        }
        let Some(path) = &self.postings_path else {
            return Ok(Vec::new());
        };
        let count = end.saturating_sub(start);
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut file = File::open(path)
            .with_context(|| format!("failed to open BM25 postings {}", path.display()))?;
        file.seek(SeekFrom::Start((start * POSTING_BYTES) as u64))?;
        let mut bytes = vec![0u8; count * POSTING_BYTES];
        file.read_exact(&mut bytes)?;
        let mut postings = Vec::with_capacity(count);
        for item in bytes.chunks_exact(POSTING_BYTES) {
            postings.push(Posting {
                doc_id: u32::from_le_bytes(item[0..4].try_into().expect("posting doc id bytes")),
                term_freq: u32::from_le_bytes(
                    item[4..8].try_into().expect("posting term frequency bytes"),
                ),
            });
        }
        Ok(postings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_ranks_matching_documents_and_respects_selector() {
        let index = Bm25Index::new(vec![
            vec!["alpha".to_string(), "beta".to_string()],
            vec!["alpha".to_string(), "alpha".to_string()],
            vec!["gamma".to_string()],
        ]);

        let hits = index.query("alpha", 10, None).unwrap();
        assert_eq!(hits[0].0, 1);
        assert_eq!(hits.len(), 2);

        let filtered = index.query("alpha", 10, Some(&[0])).unwrap();
        assert_eq!(filtered, vec![(0, filtered[0].1)]);
    }
}
