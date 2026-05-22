use regex::Regex;
use std::sync::OnceLock;

fn token_re() -> &'static Regex {
    static TOKEN_RE: OnceLock<Regex> = OnceLock::new();
    TOKEN_RE.get_or_init(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("valid token regex"))
}

pub fn raw_identifiers(text: &str) -> Vec<String> {
    token_re()
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

pub fn split_identifier(token: &str) -> Vec<String> {
    let lower = token.to_ascii_lowercase();
    let mut parts = Vec::new();

    if token.contains('_') {
        parts.extend(
            lower
                .split('_')
                .filter(|p| !p.is_empty())
                .map(str::to_string),
        );
    } else {
        let chars: Vec<char> = token.chars().collect();
        let mut start = 0;
        for i in 1..chars.len() {
            let prev = chars[i - 1];
            let cur = chars[i];
            let next = chars.get(i + 1).copied();
            let boundary = (prev.is_ascii_lowercase() && cur.is_ascii_uppercase())
                || (prev.is_ascii_uppercase()
                    && cur.is_ascii_uppercase()
                    && next.is_some_and(|n| n.is_ascii_lowercase()))
                || (!prev.is_ascii_digit() && cur.is_ascii_digit())
                || (prev.is_ascii_digit() && !cur.is_ascii_digit());
            if boundary {
                if start < i {
                    parts.push(
                        chars[start..i]
                            .iter()
                            .collect::<String>()
                            .to_ascii_lowercase(),
                    );
                }
                start = i;
            }
        }
        if start < chars.len() {
            parts.push(
                chars[start..]
                    .iter()
                    .collect::<String>()
                    .to_ascii_lowercase(),
            );
        }
    }

    if parts.len() >= 2 {
        let mut out = Vec::with_capacity(parts.len() + 1);
        out.push(lower);
        out.extend(parts);
        out
    } else {
        vec![lower]
    }
}

pub fn tokenize(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    for token in raw_identifiers(text) {
        result.extend(split_identifier(&token));
    }
    result
}

pub fn is_identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

pub fn has_whole_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(rel) = haystack[from..].find(needle) {
        let pos = from + rel;
        let before = haystack[..pos].chars().next_back();
        let after = haystack[pos + needle.len()..].chars().next();
        let before_ok = before.is_none_or(|c| !is_identifier_char(c));
        let after_ok = after.is_none_or(|c| !is_identifier_char(c));
        if before_ok && after_ok {
            return true;
        }
        from = pos + 1;
    }
    false
}
