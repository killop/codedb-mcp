use anyhow::{Result, bail};
use std::collections::HashSet;
use vicinity::hnsw::HNSWIndex;

pub struct MinishVectorStore {
    index: HNSWIndex,
    len: usize,
    ef_search: usize,
}

impl MinishVectorStore {
    pub fn build(vectors: &[Vec<f32>], fallback_dim: usize) -> Result<Self> {
        let dim = vectors
            .iter()
            .find(|vector| !vector.is_empty())
            .map_or(fallback_dim, Vec::len)
            .max(1);
        let mut index = HNSWIndex::builder(dim)
            .m(8)
            .ef_construction(80)
            .ef_search(64)
            .auto_normalize(true)
            .build()?;

        for (id, vector) in vectors.iter().enumerate() {
            if vector.len() != dim {
                bail!(
                    "embedding dimension mismatch at vector {id}: expected {dim}, got {}",
                    vector.len()
                );
            }
            index.add_slice(id as u32, vector)?;
        }
        index.build()?;
        Ok(Self {
            index,
            len: vectors.len(),
            ef_search: 64,
        })
    }

    pub fn len(&self) -> usize {
        self.len
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

        let requested = match selector {
            Some(selector) => (top_k * 20)
                .max(top_k)
                .min(self.len)
                .max(selector.len().min(top_k)),
            None => top_k.min(self.len),
        };
        let mut results = self
            .index
            .search(query, requested, self.ef_search.max(requested))?;
        if let Some(selector) = selector {
            let allowed = selector.iter().copied().collect::<HashSet<_>>();
            results.retain(|(id, _)| allowed.contains(&(*id as usize)));
        }
        results.truncate(top_k);
        Ok(results
            .into_iter()
            .map(|(id, distance)| (id as usize, 1.0 - distance))
            .collect())
    }
}
