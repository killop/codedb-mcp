use crate::indexer::Codebase;
use crate::tokens::raw_identifiers;
use anyhow::Result;
use std::collections::HashMap;

pub fn hybrid_ranked_chunks(
    index: &Codebase,
    query: &str,
    top_k: usize,
    selector: Option<&[usize]>,
) -> Result<Vec<(usize, f32)>> {
    ranked_chunks(index, query, top_k, selector)
}

pub fn lexical_ranked_chunks(
    index: &Codebase,
    query: &str,
    top_k: usize,
    selector: Option<&[usize]>,
) -> Result<Vec<(usize, f32)>> {
    ranked_chunks(index, query, top_k, selector)
}

fn ranked_chunks(
    index: &Codebase,
    query: &str,
    top_k: usize,
    selector: Option<&[usize]>,
) -> Result<Vec<(usize, f32)>> {
    if query.trim().is_empty() || top_k == 0 || index.chunks.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_count = top_k.saturating_mul(5).clamp(20, 400);
    let mut scores = index.ranked_bm25_chunks(query, candidate_count, selector)?;
    if scores.is_empty() {
        return Ok(Vec::new());
    }

    apply_file_coherence(index, &mut scores);
    scores.sort_by(|(left_idx, left_score), (right_idx, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| {
                index
                    .chunk_file_path(&index.chunks[*left_idx])
                    .cmp(index.chunk_file_path(&index.chunks[*right_idx]))
            })
            .then_with(|| {
                index.chunks[*left_idx]
                    .start_line
                    .cmp(&index.chunks[*right_idx].start_line)
            })
    });
    scores.truncate(top_k.min(scores.len()));
    Ok(scores)
}

pub fn is_symbol_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.split_whitespace().nth(1).is_some() {
        return false;
    }
    let identifiers = raw_identifiers(trimmed);
    identifiers.len() == 1
        && (trimmed.contains("::")
            || trimmed.contains('.')
            || trimmed.contains('_')
            || trimmed.chars().any(|ch| ch.is_ascii_uppercase()))
}

fn apply_file_coherence(index: &Codebase, scores: &mut [(usize, f32)]) {
    if scores.is_empty() {
        return;
    }
    let max_score = scores
        .iter()
        .map(|(_, score)| *score)
        .fold(0.0f32, f32::max);
    if max_score <= 0.0 {
        return;
    }
    let mut file_sums = HashMap::<u32, f32>::new();
    let mut best_by_file = HashMap::<u32, (usize, f32)>::new();
    for (idx, score) in scores.iter() {
        let file_id = index.chunks[*idx].file_id;
        *file_sums.entry(file_id).or_default() += *score;
        match best_by_file.get(&file_id) {
            Some((_, current)) if current >= score => {}
            _ => {
                best_by_file.insert(file_id, (*idx, *score));
            }
        }
    }
    let max_file_sum = file_sums.values().copied().fold(0.0f32, f32::max);
    if max_file_sum <= 0.0 {
        return;
    }
    for (idx, score) in scores.iter_mut() {
        let file_id = index.chunks[*idx].file_id;
        if best_by_file
            .get(&file_id)
            .is_some_and(|(best_idx, _)| best_idx == idx)
        {
            *score += max_score * 0.2 * file_sums[&file_id] / max_file_sum;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_shape_routes_exact_symbols() {
        assert!(is_symbol_query("TryOpenExpeditionPanel"));
        assert!(!is_symbol_query("session"));
        assert!(!is_symbol_query("how session state is restored"));
    }
}
