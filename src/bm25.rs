use crate::tokens::tokenize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Posting {
    doc_id: usize,
    term_freq: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Bm25Index {
    doc_lens: Vec<u32>,
    postings: HashMap<String, Vec<Posting>>,
    avg_doc_len: f32,
    doc_count: usize,
}

impl Bm25Index {
    pub fn new(documents: Vec<Vec<String>>) -> Self {
        let mut postings: HashMap<String, Vec<Posting>> = HashMap::new();
        let mut doc_lens = Vec::with_capacity(documents.len());
        let mut total_len = 0usize;
        for (doc_id, doc) in documents.iter().enumerate() {
            total_len += doc.len();
            doc_lens.push(doc.len() as u32);
            let mut term_freqs: HashMap<&str, u32> = HashMap::new();
            for token in doc {
                *term_freqs.entry(token.as_str()).or_default() += 1;
            }
            for (token, term_freq) in term_freqs {
                postings
                    .entry(token.to_string())
                    .or_default()
                    .push(Posting { doc_id, term_freq });
            }
        }
        let avg_doc_len = if documents.is_empty() {
            0.0
        } else {
            total_len as f32 / documents.len() as f32
        };
        Self {
            doc_lens,
            postings,
            avg_doc_len,
            doc_count: documents.len(),
        }
    }

    pub fn query(
        &self,
        query: &str,
        top_k: usize,
        selector: Option<&[usize]>,
    ) -> Vec<(usize, f32)> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() || self.doc_count == 0 {
            return Vec::new();
        }

        let mut scores = vec![0.0f32; self.doc_count];
        let mut touched = Vec::new();
        for token in query_tokens {
            let Some(postings) = self.postings.get(&token) else {
                continue;
            };
            let df = postings.len() as f32;
            let idf = ((self.doc_count as f32 - df + 0.5) / (df + 0.5) + 1.0).ln();
            for posting in postings {
                if selector.is_some_and(|indices| indices.binary_search(&posting.doc_id).is_err()) {
                    continue;
                }
                let score = self.term_score(posting.doc_id, posting.term_freq, idf);
                if score <= 0.0 {
                    continue;
                }
                if scores[posting.doc_id] == 0.0 {
                    touched.push(posting.doc_id);
                }
                scores[posting.doc_id] += score;
            }
        }

        let mut ranked = touched
            .into_iter()
            .map(|idx| (idx, scores[idx]))
            .filter(|(_, score)| *score > 0.0)
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(top_k.min(ranked.len()));
        ranked
    }

    fn term_score(&self, doc_id: usize, term_freq: u32, idf: f32) -> f32 {
        let k1 = 1.5f32;
        let b = 0.75f32;
        let freq = term_freq as f32;
        let doc_len = self.doc_lens.get(doc_id).copied().unwrap_or_default() as f32;
        let denom = freq + k1 * (1.0 - b + b * doc_len / self.avg_doc_len.max(1.0));
        idf * (freq * (k1 + 1.0)) / denom
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

        let hits = index.query("alpha", 10, None);
        assert_eq!(hits[0].0, 1);
        assert_eq!(hits.len(), 2);

        let filtered = index.query("alpha", 10, Some(&[0]));
        assert_eq!(filtered, vec![(0, filtered[0].1)]);
    }
}
