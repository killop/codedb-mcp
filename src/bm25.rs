use crate::tokens::tokenize;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const POSTING_BYTES: usize = 8;
const POSTING_RECORD_BYTES: usize = 12;
const SPILL_RECORD_THRESHOLD: usize = 1_000_000;

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
    #[cfg(test)]
    pub fn new(documents: impl IntoIterator<Item = Vec<String>>) -> Self {
        let mut builder = Bm25Builder::new();
        for document in documents {
            builder.add_document(document);
        }
        builder.finish()
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
        if self.postings.is_empty()
            && self
                .postings_path
                .as_ref()
                .is_some_and(|current| current == path)
        {
            return Ok(());
        }
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

#[derive(Debug, Default)]
pub struct Bm25Builder {
    doc_lens: Vec<u32>,
    total_len: usize,
    term_ids: HashMap<String, usize>,
    postings_by_term_id: Vec<Vec<Posting>>,
}

impl Bm25Builder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_document(&mut self, document: impl IntoIterator<Item = String>) {
        let doc_id = self.doc_lens.len() as u32;
        let mut doc_len = 0usize;
        let mut term_freqs: HashMap<usize, u32> = HashMap::new();
        for token in document {
            doc_len += 1;
            let next_id = self.term_ids.len();
            let term_id = match self.term_ids.entry(token) {
                std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    self.postings_by_term_id.push(Vec::new());
                    *entry.insert(next_id)
                }
            };
            *term_freqs.entry(term_id).or_default() += 1;
        }
        self.total_len += doc_len;
        self.doc_lens.push(doc_len as u32);
        for (term_id, term_freq) in term_freqs {
            if let Some(postings) = self.postings_by_term_id.get_mut(term_id) {
                postings.push(Posting { doc_id, term_freq });
            }
        }
    }

    pub fn finish(self) -> Bm25Index {
        let doc_count = self.doc_lens.len();
        let avg_doc_len = avg_doc_len(self.total_len, doc_count);
        let mut terms = self.term_ids.into_iter().collect::<Vec<_>>();
        terms.sort_by(|a, b| a.0.cmp(&b.0));
        let posting_count = self.postings_by_term_id.iter().map(Vec::len).sum();
        let mut postings_by_term_id = self.postings_by_term_id;
        let mut ranges = HashMap::with_capacity(terms.len());
        let mut postings = Vec::with_capacity(posting_count);
        for (term, term_id) in terms {
            let start = postings.len() as u32;
            let Some(values) = postings_by_term_id.get_mut(term_id) else {
                continue;
            };
            values.sort_by_key(|posting| posting.doc_id);
            let len = values.len() as u32;
            postings.append(values);
            ranges.insert(term, PostingRange { start, len });
        }
        Bm25Index {
            doc_lens: self.doc_lens,
            ranges,
            postings,
            postings_path: None,
            avg_doc_len,
            doc_count,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct PostingRecord {
    term_id: u32,
    doc_id: u32,
    term_freq: u32,
}

impl Ord for PostingRecord {
    fn cmp(&self, other: &Self) -> Ordering {
        self.term_id
            .cmp(&other.term_id)
            .then_with(|| self.doc_id.cmp(&other.doc_id))
    }
}

impl PartialOrd for PostingRecord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
pub struct SpillingBm25Builder {
    doc_lens: Vec<u32>,
    total_len: usize,
    term_ids: HashMap<String, u32>,
    records: Vec<PostingRecord>,
    spill_dir: PathBuf,
    run_paths: Vec<PathBuf>,
    run_seq: usize,
}

impl SpillingBm25Builder {
    pub fn new(spill_dir: PathBuf) -> Result<Self> {
        if spill_dir.exists() {
            std::fs::remove_dir_all(&spill_dir).with_context(|| {
                format!("failed to clear BM25 spill dir {}", spill_dir.display())
            })?;
        }
        std::fs::create_dir_all(&spill_dir)
            .with_context(|| format!("failed to create BM25 spill dir {}", spill_dir.display()))?;
        Ok(Self {
            doc_lens: Vec::new(),
            total_len: 0,
            term_ids: HashMap::new(),
            records: Vec::with_capacity(SPILL_RECORD_THRESHOLD.min(64 * 1024)),
            spill_dir,
            run_paths: Vec::new(),
            run_seq: 0,
        })
    }

    pub fn add_document(&mut self, document: impl IntoIterator<Item = String>) -> Result<()> {
        let doc_id = self.doc_lens.len() as u32;
        let mut doc_len = 0usize;
        let mut term_freqs: HashMap<u32, u32> = HashMap::new();
        for token in document {
            doc_len += 1;
            let next_id = self.term_ids.len() as u32;
            let term_id = match self.term_ids.entry(token) {
                std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                std::collections::hash_map::Entry::Vacant(entry) => *entry.insert(next_id),
            };
            *term_freqs.entry(term_id).or_default() += 1;
        }
        self.total_len += doc_len;
        self.doc_lens.push(doc_len as u32);
        for (term_id, term_freq) in term_freqs {
            self.records.push(PostingRecord {
                term_id,
                doc_id,
                term_freq,
            });
        }
        if self.records.len() >= SPILL_RECORD_THRESHOLD {
            self.flush_run()?;
        }
        Ok(())
    }

    pub fn finish_to_postings_file(mut self, path: &Path) -> Result<Bm25Index> {
        self.flush_run()?;
        let doc_count = self.doc_lens.len();
        let avg_doc_len = avg_doc_len(self.total_len, doc_count);
        let mut terms = vec![String::new(); self.term_ids.len()];
        for (term, term_id) in self.term_ids {
            if let Some(slot) = terms.get_mut(term_id as usize) {
                *slot = term;
            }
        }

        let file = File::create(path)
            .with_context(|| format!("failed to create BM25 postings {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        let mut readers = self
            .run_paths
            .iter()
            .map(RunReader::open)
            .collect::<Result<Vec<_>>>()?;
        let mut heap = std::collections::BinaryHeap::new();
        for (run_idx, reader) in readers.iter_mut().enumerate() {
            if let Some(record) = reader.next_record()? {
                heap.push(HeapItem { record, run_idx });
            }
        }

        let mut ranges = HashMap::with_capacity(terms.len());
        let mut current_term_id: Option<u32> = None;
        let mut current_start = 0u32;
        let mut current_len = 0u32;
        let mut postings_written = 0u32;
        while let Some(item) = heap.pop() {
            let record = item.record;
            if current_term_id.is_some_and(|term_id| term_id != record.term_id) {
                insert_spilled_range(
                    &mut ranges,
                    &mut terms,
                    current_term_id.expect("current term id"),
                    current_start,
                    current_len,
                );
                current_start = postings_written;
                current_len = 0;
            } else if current_term_id.is_none() {
                current_start = postings_written;
            }
            current_term_id = Some(record.term_id);
            writer.write_all(&record.doc_id.to_le_bytes())?;
            writer.write_all(&record.term_freq.to_le_bytes())?;
            postings_written = postings_written.saturating_add(1);
            current_len = current_len.saturating_add(1);
            if let Some(next) = readers[item.run_idx].next_record()? {
                heap.push(HeapItem {
                    record: next,
                    run_idx: item.run_idx,
                });
            }
        }
        if let Some(term_id) = current_term_id {
            insert_spilled_range(&mut ranges, &mut terms, term_id, current_start, current_len);
        }
        writer.flush()?;
        for path in &self.run_paths {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(&self.spill_dir);
        Ok(Bm25Index {
            doc_lens: self.doc_lens,
            ranges,
            postings: Vec::new(),
            postings_path: Some(path.to_path_buf()),
            avg_doc_len,
            doc_count,
        })
    }

    fn flush_run(&mut self) -> Result<()> {
        if self.records.is_empty() {
            return Ok(());
        }
        self.records.sort_unstable();
        let path = self.spill_dir.join(format!("run-{}.bin", self.run_seq));
        self.run_seq += 1;
        let file = File::create(&path)
            .with_context(|| format!("failed to create BM25 spill run {}", path.display()))?;
        let mut writer = BufWriter::new(file);
        for record in &self.records {
            writer.write_all(&record.term_id.to_le_bytes())?;
            writer.write_all(&record.doc_id.to_le_bytes())?;
            writer.write_all(&record.term_freq.to_le_bytes())?;
        }
        writer.flush()?;
        self.records.clear();
        self.run_paths.push(path);
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
struct HeapItem {
    record: PostingRecord,
    run_idx: usize,
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .record
            .cmp(&self.record)
            .then_with(|| other.run_idx.cmp(&self.run_idx))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct RunReader {
    reader: BufReader<File>,
}

impl RunReader {
    fn open(path: &PathBuf) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("failed to open BM25 spill run {}", path.display()))?;
        Ok(Self {
            reader: BufReader::new(file),
        })
    }

    fn next_record(&mut self) -> Result<Option<PostingRecord>> {
        let mut bytes = [0u8; POSTING_RECORD_BYTES];
        match self.reader.read_exact(&mut bytes) {
            Ok(()) => Ok(Some(PostingRecord {
                term_id: u32::from_le_bytes(bytes[0..4].try_into().expect("term id bytes")),
                doc_id: u32::from_le_bytes(bytes[4..8].try_into().expect("doc id bytes")),
                term_freq: u32::from_le_bytes(bytes[8..12].try_into().expect("term freq bytes")),
            })),
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => Ok(None),
            Err(err) => Err(err).context("failed to read BM25 spill record"),
        }
    }
}

fn insert_spilled_range(
    ranges: &mut HashMap<String, PostingRange>,
    terms: &mut [String],
    term_id: u32,
    start: u32,
    len: u32,
) {
    if len == 0 {
        return;
    }
    let Some(term) = terms.get_mut(term_id as usize) else {
        return;
    };
    if term.is_empty() {
        return;
    }
    ranges.insert(std::mem::take(term), PostingRange { start, len });
}

fn avg_doc_len(total_len: usize, doc_count: usize) -> f32 {
    if doc_count == 0 {
        0.0
    } else {
        total_len as f32 / doc_count as f32
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
