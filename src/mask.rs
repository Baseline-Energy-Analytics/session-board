//! Masking what looks like a credential. Commands go on screen and into the cache, never raw.

use regex::Regex;
use std::sync::LazyLock;

/// NAME=value where the name says key/token/secret/password, bearer and basic auth, URLs with a
/// password, and long opaque strings. Applied in order, each on the previous one's output.
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        (r"(?i)\b(bearer|basic|token)\s+[A-Za-z0-9._~+/=-]{8,}", "${1} …"),
        (
            r#"(?i)\b([A-Z0-9_.-]*(?:KEY|TOKEN|SECRET|PASSWORD|PASSWD|PASS|PWD|AUTH|CREDENTIALS?)[A-Z0-9_.-]*)(\s*[=:]\s*)("[^"]*"|'[^']*'|\S+)"#,
            "${1}${2}…",
        ),
        (r"(?i)(--?(?:password|passwd|token|api-key|apikey|secret|key)[= ])\S+", "${1}…"),
        (r"(://[^/\s:@]+:)[^@\s/]+@", "${1}…@"),
        (r"[A-Za-z0-9_+/=-]{32,}", "…"),
    ]
    .into_iter()
    .map(|(p, r)| (Regex::new(p).expect("mask pattern"), r))
    .collect()
});

pub fn mask_secrets(text: &str) -> String {
    PATTERNS
        .iter()
        .fold(text.to_string(), |acc, (re, repl)| re.replace_all(&acc, *repl).into_owned())
}

/// Whitespace runs collapsed to one space, ends trimmed: a command on one line.
pub fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first `n` characters (not bytes).
pub fn truncate_chars(text: &str, n: usize) -> &str {
    match text.char_indices().nth(n) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}
