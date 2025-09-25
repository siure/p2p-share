use std::fmt::Write as _;

/// Generate a human-friendly pairing code like "brave-sky-otter".
/// Uses the `petname` crate wordlists. `words` is typically 3 or 4.
pub fn generate_code(words: usize) -> String {
    // petname::petname returns "word1-word2-...".
    // We also ensure lowercase and ASCII-safe separators.
    let w = words.clamp(2, 6);
    let s = petname::petname(w.try_into().unwrap(), "-");
    normalize_code(&s.unwrap())
}

/// Normalize a user-entered code:
/// - lowercase
/// - keep only [a-z0-9] and hyphens
/// - collapse all non-alnum into single hyphens
pub fn normalize_code(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_sep = false;
    for ch in input.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            in_sep = false;
        } else {
            if !in_sep && !out.is_empty() {
                out.push('-');
            }
            in_sep = true;
        }
    }
    // Trim leading/trailing hyphens
    while out.ends_with('-') {
        out.pop();
    }
    while out.starts_with('-') {
        out.remove(0);
    }
    // Avoid empty string
    if out.is_empty() {
        out.push_str("code-code-code");
    }
    out
}

/// Produce a compact, human-readable size string (for later use if needed).
#[allow(dead_code)]
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut idx = 0usize;
    while value >= 1024.0 && idx < UNITS.len() - 1 {
        value /= 1024.0;
        idx += 1;
    }
    let mut s = String::new();
    let _ = write!(&mut s, "{:.2} {}", value, UNITS[idx]);
    s
}