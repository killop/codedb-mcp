use crate::indexer::Codebase;
use crate::tokens::{split_identifier, tokenize};
use crate::types::{Chunk, ChunkSearchHit};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const RRF_K: f32 = 60.0;

pub fn hybrid_search(
    index: &Codebase,
    query: &str,
    top_k: usize,
    selector: Option<&[usize]>,
) -> Result<Vec<ChunkSearchHit>> {
    if query.trim().is_empty() || index.chunks.is_empty() {
        return Ok(Vec::new());
    }

    let candidate_count = (top_k.max(1) * 5).min(index.chunks.len()).max(top_k);
    let bm25 = index
        .bm25
        .query(query, candidate_count, selector)
        .into_iter()
        .collect::<HashMap<usize, f32>>();
    let query_vec = index.model.encode_one(query);
    let semantic_files = index
        .vectors
        .query(&query_vec, candidate_count, None)?
        .into_iter()
        .collect::<HashMap<usize, f32>>();
    let semantic = semantic_chunk_scores(index, query, &semantic_files, top_k, selector);

    let semantic_rrf = rrf_scores(&semantic);
    let bm25_rrf = rrf_scores(&bm25);
    let mut candidates = HashSet::new();
    candidates.extend(semantic_rrf.keys().copied());
    candidates.extend(bm25_rrf.keys().copied());

    let alpha = if is_symbol_query(query) { 0.3 } else { 0.5 };
    let mut scores = HashMap::new();
    for idx in candidates {
        let score = alpha * semantic_rrf.get(&idx).copied().unwrap_or(0.0)
            + (1.0 - alpha) * bm25_rrf.get(&idx).copied().unwrap_or(0.0);
        scores.insert(idx, score);
    }

    boost_multi_chunk_files(index, &mut scores);
    let allowed = selector.map(|items| items.iter().copied().collect::<HashSet<_>>());
    apply_query_boost(index, query, &mut scores, allowed.as_ref());
    let mut ranked = scores.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|(a_idx, a_score), (b_idx, b_score)| {
        let a_score = *a_score * path_penalty(&index.chunks[*a_idx].file_path);
        let b_score = *b_score * path_penalty(&index.chunks[*b_idx].file_path);
        b_score
            .total_cmp(&a_score)
            .then_with(|| {
                index.chunks[*a_idx]
                    .file_path
                    .cmp(&index.chunks[*b_idx].file_path)
            })
            .then_with(|| {
                index.chunks[*a_idx]
                    .start_line
                    .cmp(&index.chunks[*b_idx].start_line)
            })
    });
    ranked.truncate(top_k.min(ranked.len()));
    Ok(ranked
        .into_iter()
        .map(|(idx, score)| ChunkSearchHit {
            chunk: index.chunks[idx].clone(),
            score,
            source: "hybrid",
        })
        .collect())
}

pub fn is_symbol_query(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.contains(' ') {
        return false;
    }
    trimmed.contains("::")
        || trimmed.contains('.')
        || trimmed.contains('_')
        || trimmed.chars().next().is_some_and(|c| c == '_')
        || trimmed.chars().any(|c| c.is_ascii_uppercase())
}

fn rrf_scores(scores: &HashMap<usize, f32>) -> HashMap<usize, f32> {
    let mut ranked = scores.iter().collect::<Vec<_>>();
    ranked.sort_by(|(a_idx, a_score), (b_idx, b_score)| {
        b_score.total_cmp(a_score).then_with(|| a_idx.cmp(b_idx))
    });
    ranked
        .into_iter()
        .enumerate()
        .map(|(rank, (&idx, _))| (idx, 1.0 / (RRF_K + rank as f32 + 1.0)))
        .collect()
}

fn semantic_chunk_scores(
    index: &Codebase,
    query: &str,
    file_scores: &HashMap<usize, f32>,
    top_k: usize,
    selector: Option<&[usize]>,
) -> HashMap<usize, f32> {
    if file_scores.is_empty() {
        return HashMap::new();
    }

    let allowed = selector.map(|items| items.iter().copied().collect::<HashSet<_>>());
    let mut ranked_files = file_scores.iter().collect::<Vec<_>>();
    ranked_files.sort_by(|(a_idx, a_score), (b_idx, b_score)| {
        b_score.total_cmp(a_score).then_with(|| a_idx.cmp(b_idx))
    });

    let mut scores = HashMap::new();
    let query_tokens = tokenize(query)
        .into_iter()
        .filter(|token| token.len() > 2 && !STOPWORDS.contains(&token.as_str()))
        .collect::<HashSet<_>>();
    let per_file = top_k.clamp(1, 4);
    let max_files = (top_k * 4).clamp(8, 80).min(ranked_files.len());

    for (&file_idx, &file_score) in ranked_files.into_iter().take(max_files) {
        let Some(unit) = index.semantic_units.get(file_idx) else {
            continue;
        };
        let Some(chunk_indices) = index.chunk_indices_by_file.get(&unit.file_path) else {
            continue;
        };
        let mut chunks = chunk_indices
            .iter()
            .copied()
            .filter(|idx| allowed.as_ref().is_none_or(|allowed| allowed.contains(idx)))
            .map(|idx| {
                let local = semantic_local_chunk_score(&index.chunks[idx], &query_tokens, query);
                (idx, local)
            })
            .collect::<Vec<_>>();
        chunks.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (rank, (idx, local_score)) in chunks.into_iter().take(per_file).enumerate() {
            let rank_penalty = 1.0 / (rank as f32 + 1.0);
            let score = file_score * (1.0 + local_score) * rank_penalty;
            scores
                .entry(idx)
                .and_modify(|existing| {
                    if *existing < score {
                        *existing = score;
                    }
                })
                .or_insert(score);
        }
    }
    scores
}

fn semantic_local_chunk_score(chunk: &Chunk, query_tokens: &HashSet<String>, query: &str) -> f32 {
    let mut score = 0.0f32;
    if !query_tokens.is_empty() {
        let chunk_tokens = tokenize(&chunk.content).into_iter().collect::<HashSet<_>>();
        let overlap = query_tokens
            .iter()
            .filter(|token| chunk_tokens.contains(*token))
            .count();
        score += overlap as f32 / query_tokens.len() as f32;
    }
    if is_symbol_query(query) {
        let symbol = query
            .rsplit(['.', ':', '\\'])
            .next()
            .unwrap_or(query)
            .trim();
        if chunk_defines_symbol(chunk, symbol) {
            score += 2.0;
        } else if chunk.content.contains(symbol) {
            score += 0.5;
        }
    }
    score
}

fn boost_multi_chunk_files(index: &Codebase, scores: &mut HashMap<usize, f32>) {
    if scores.is_empty() {
        return;
    }
    let max_score = scores.values().copied().fold(0.0f32, f32::max);
    if max_score == 0.0 {
        return;
    }
    let mut file_sum: HashMap<&str, f32> = HashMap::new();
    let mut best_chunk: HashMap<&str, usize> = HashMap::new();
    for (&idx, &score) in scores.iter() {
        let path = index.chunks[idx].file_path.as_str();
        *file_sum.entry(path).or_default() += score;
        match best_chunk.get(path) {
            Some(&current) if scores[&current] >= score => {}
            _ => {
                best_chunk.insert(path, idx);
            }
        }
    }
    let max_file_sum = file_sum.values().copied().fold(0.0f32, f32::max).max(1e-6);
    let boost_unit = max_score * 0.2;
    for (path, idx) in best_chunk {
        if let Some(score) = scores.get_mut(&idx) {
            *score += boost_unit * file_sum[path] / max_file_sum;
        }
    }
}

fn apply_query_boost(
    index: &Codebase,
    query: &str,
    scores: &mut HashMap<usize, f32>,
    allowed: Option<&HashSet<usize>>,
) {
    if scores.is_empty() {
        return;
    }
    let max_score = scores.values().copied().fold(0.0f32, f32::max);
    if is_symbol_query(query) {
        let symbol = query
            .rsplit(['.', ':', '\\'])
            .next()
            .unwrap_or(query)
            .trim();
        boost_symbol_definitions(index, scores, symbol, max_score * 3.0, allowed);
    } else {
        boost_path_keyword_matches(index, scores, query, max_score);
        for symbol in embedded_symbols(query) {
            boost_symbol_definitions(index, scores, &symbol, max_score * 1.5, allowed);
        }
    }
}

fn boost_symbol_definitions(
    index: &Codebase,
    scores: &mut HashMap<usize, f32>,
    symbol: &str,
    boost_unit: f32,
    allowed: Option<&HashSet<usize>>,
) {
    let symbol_lower = symbol.to_ascii_lowercase();
    let mut boosted_non_candidates = Vec::new();

    if let Some(indices) = index.symbol_definition_chunks.get(&symbol_lower) {
        for &idx in indices {
            if allowed.is_some_and(|allowed| !allowed.contains(&idx)) {
                continue;
            }
            let multiplier = symbol_definition_multiplier(index, idx, &symbol_lower);
            if let Some(score) = scores.get_mut(&idx) {
                *score += boost_unit * multiplier;
            } else if multiplier > 1.0 {
                boosted_non_candidates.push((idx, boost_unit * multiplier));
            }
        }
    } else {
        for (&idx, score) in scores.iter_mut() {
            if allowed.is_some_and(|allowed| !allowed.contains(&idx)) {
                continue;
            }
            if chunk_defines_symbol(&index.chunks[idx], symbol) {
                *score += boost_unit * symbol_definition_multiplier(index, idx, &symbol_lower);
            }
        }
    }

    for (idx, score) in boosted_non_candidates {
        scores.insert(idx, score);
    }
}

fn symbol_definition_multiplier(index: &Codebase, idx: usize, symbol_lower: &str) -> f32 {
    let stem = Path::new(&index.chunks[idx].file_path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if stem_matches(&stem, symbol_lower) {
        1.5
    } else {
        1.0
    }
}

fn boost_path_keyword_matches(
    index: &Codebase,
    scores: &mut HashMap<usize, f32>,
    query: &str,
    max_score: f32,
) {
    let keywords = tokenize(query)
        .into_iter()
        .filter(|word| word.len() > 2 && !STOPWORDS.contains(&word.as_str()))
        .collect::<HashSet<_>>();
    if keywords.is_empty() {
        return;
    }
    for (&idx, score) in scores.iter_mut() {
        let path = Path::new(&index.chunks[idx].file_path);
        let mut parts = HashSet::new();
        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
            parts.extend(split_identifier(stem));
        }
        if let Some(parent) = path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
        {
            parts.extend(split_identifier(parent));
        }
        let matches = keywords
            .iter()
            .filter(|keyword| parts.iter().any(|part| prefix_overlap(keyword, part)))
            .count();
        if matches > 0 {
            *score += max_score * (matches as f32 / keywords.len() as f32);
        }
    }
}

fn embedded_symbols(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|part| {
            part.len() > 3
                && part.chars().any(|c| c.is_ascii_uppercase())
                && part.chars().any(|c| c.is_ascii_lowercase())
        })
        .map(str::to_string)
        .collect()
}

fn chunk_defines_symbol(chunk: &Chunk, symbol: &str) -> bool {
    let needles = [
        format!("class {symbol}"),
        format!("interface {symbol}"),
        format!("struct {symbol}"),
        format!("enum {symbol}"),
        format!("record {symbol}"),
        format!(" {symbol}("),
    ];
    needles.iter().any(|needle| chunk.content.contains(needle))
}

fn stem_matches(stem: &str, name: &str) -> bool {
    let stem_norm = stem.replace('_', "");
    stem == name
        || stem_norm == name
        || stem.trim_end_matches('s') == name
        || stem_norm.trim_end_matches('s') == name
}

fn prefix_overlap(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    let (shorter, longer) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };
    shorter.len() >= 3 && longer.starts_with(shorter)
}

fn path_penalty(path: &str) -> f32 {
    let lower = path.to_ascii_lowercase();
    if lower.contains("/test")
        || lower.contains("tests/")
        || lower.ends_with("test.cs")
        || lower.ends_with("test.java")
    {
        return 0.75;
    }
    if lower.contains("/examples/") || lower.contains("/samples/") {
        return 0.85;
    }
    if lower.contains("/compat/") || lower.contains("/legacy/") {
        return 0.9;
    }
    1.0
}

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "do", "does", "for", "from", "has", "have",
    "how", "if", "in", "is", "it", "not", "of", "on", "or", "the", "to", "was", "what", "when",
    "where", "which", "who", "why", "with",
];
