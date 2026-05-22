use crate::types::{Chunk, Scope, Symbol};
use regex::{Regex, RegexBuilder};
use std::sync::OnceLock;

const DESIRED_CHUNK_CHARS: usize = 1500;

fn line_comment_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*(//|/\*|\*|\*/)").expect("valid comment regex"))
}

pub fn chunk_source(
    content: &str,
    file_path: &str,
    language: &str,
    symbols: &[Symbol],
) -> Vec<Chunk> {
    if content.trim().is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Vec::new();
    }

    let mut ranges = Vec::<(usize, usize)>::new();
    if symbols.is_empty() {
        ranges.extend(line_chunks(&lines, 1, lines.len()));
    } else {
        let mut cursor = 1usize;
        for symbol in symbols {
            if cursor < symbol.line_start {
                ranges.extend(line_chunks(&lines, cursor, symbol.line_start - 1));
            }
            ranges.extend(line_chunks(
                &lines,
                symbol.line_start,
                symbol.line_end.min(lines.len()),
            ));
            cursor = symbol.line_end.saturating_add(1);
        }
        if cursor <= lines.len() {
            ranges.extend(line_chunks(&lines, cursor, lines.len()));
        }
    }

    merge_adjacent_ranges(&lines, ranges)
        .into_iter()
        .filter_map(|(start, end)| {
            let content = lines[start - 1..end].join("\n");
            if content.trim().is_empty() {
                None
            } else {
                Some(Chunk {
                    id: 0,
                    file_path: file_path.to_string(),
                    start_line: start,
                    end_line: end,
                    language: language.to_string(),
                    content,
                })
            }
        })
        .collect()
}

pub fn scope_for_line(symbols: &[Symbol], line: usize) -> Option<Scope> {
    symbols
        .iter()
        .filter(|sym| sym.line_start <= line && line <= sym.line_end)
        .max_by_key(|sym| sym.line_start)
        .map(|sym| Scope {
            name: sym.name.clone(),
            kind: sym.kind.clone(),
            start: sym.line_start,
            end: sym.line_end,
        })
}

pub fn is_comment_or_blank(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || line_comment_re().is_match(trimmed)
}

pub fn regex_case_insensitive(pattern: &str) -> Result<Regex, regex::Error> {
    RegexBuilder::new(pattern).case_insensitive(true).build()
}

fn line_chunks(lines: &[&str], start: usize, end: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut chunk_start = start;
    let mut chars = 0usize;
    for line_no in start..=end {
        chars += lines[line_no - 1].len() + 1;
        if chars >= DESIRED_CHUNK_CHARS && line_no >= chunk_start {
            ranges.push((chunk_start, line_no));
            chunk_start = line_no + 1;
            chars = 0;
        }
    }
    if chunk_start <= end {
        ranges.push((chunk_start, end));
    }
    ranges
}

fn merge_adjacent_ranges(lines: &[&str], ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    let mut merged = Vec::new();
    let mut current: Option<(usize, usize, usize)> = None;

    for (start, end) in ranges {
        let chars = range_chars(lines, start, end);
        match current {
            Some((cur_start, cur_end, cur_chars))
                if start <= cur_end + 1 && cur_chars + chars <= DESIRED_CHUNK_CHARS =>
            {
                current = Some((cur_start, end, cur_chars + chars));
            }
            Some((cur_start, cur_end, _)) => {
                merged.push((cur_start, cur_end));
                current = Some((start, end, chars));
            }
            None => current = Some((start, end, chars)),
        }
    }

    if let Some((start, end, _)) = current {
        merged.push((start, end));
    }
    merged
}

fn range_chars(lines: &[&str], start: usize, end: usize) -> usize {
    lines[start - 1..end]
        .iter()
        .map(|line| line.len() + 1)
        .sum()
}
