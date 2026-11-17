/// Describes a count in words.
pub fn describe(count: usize) -> String {
    let label = if count == 1 { "item" } else { "items" };
    format!("{count} {label}")
}

/// Shortens `text` to at most `max` characters.
pub fn shorten(text: &str, max: usize) -> String {
    // Old code: clippy flags the needless `return` below, and nobody has touched it yet.
    return text.chars().take(max).collect();
}
