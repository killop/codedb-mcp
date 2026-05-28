use anyhow::{Result, bail};
use std::collections::HashSet;

pub struct MinishVectorStore {
    vectors: Vec<f32>,
    len: usize,
    dim: usize,
}

impl MinishVectorStore {
    pub fn build(vectors: &[Vec<f32>], fallback_dim: usize) -> Result<Self> {
        let dim = vectors
            .iter()
            .find(|vector| !vector.is_empty())
            .map_or(fallback_dim, Vec::len)
            .max(1);
        let mut flat = Vec::with_capacity(vectors.len() * dim);
        for (id, vector) in vectors.iter().enumerate() {
            if vector.len() != dim {
                bail!(
                    "embedding dimension mismatch at vector {id}: expected {dim}, got {}",
                    vector.len()
                );
            }
            let norm = vector
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt()
                .max(1e-6);
            flat.extend(vector.iter().map(|value| value / norm));
        }
        Ok(Self {
            vectors: flat,
            len: vectors.len(),
            dim,
        })
    }

    pub fn query(
        &self,
        query: &[f32],
        top_k: usize,
        selector: Option<&[usize]>,
    ) -> Result<Vec<(usize, f32)>> {
        if self.len == 0 || top_k == 0 {
            return Ok(Vec::new());
        }
        if query.len() != self.dim {
            bail!(
                "embedding dimension mismatch: expected {}, got {}",
                self.dim,
                query.len()
            );
        }
        let query_norm = query
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt()
            .max(1e-6);
        let normalized_query = query
            .iter()
            .map(|value| value / query_norm)
            .collect::<Vec<_>>();
        let allowed = selector.map(|items| items.iter().copied().collect::<HashSet<_>>());
        let mut scores = Vec::new();
        for id in 0..self.len {
            if allowed
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(&id))
            {
                continue;
            }
            let start = id * self.dim;
            let end = start + self.dim;
            let dot = self.vectors[start..end]
                .iter()
                .zip(normalized_query.iter())
                .map(|(left, right)| left * right)
                .sum::<f32>();
            scores.push((id, dot));
        }
        scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scores.truncate(top_k.min(scores.len()));
        Ok(scores)
    }
}
