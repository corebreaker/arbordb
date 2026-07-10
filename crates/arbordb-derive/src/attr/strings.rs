//! Small string-casing helpers shared by the `rename` / `rename_all` rules.

/// Lower- or upper-cases `variant`, inserting `sep` before each interior capital
/// (`FooBar` -> `foo_bar` / `FOO-BAR`).
pub(super) fn split_pascal(variant: &str, sep: char, screaming: bool) -> String {
    let mut out = String::new();

    for (i, ch) in variant.chars().enumerate() {
        if i > 0 && ch.is_ascii_uppercase() {
            out.push(sep);
        }

        out.push(if screaming {
            ch.to_ascii_uppercase()
        } else {
            ch.to_ascii_lowercase()
        });
    }

    out
}

/// Upper-cases the first character of `word`, leaving the rest unchanged.
pub(super) fn capitalize(word: &str) -> String {
    let mut chars = word.chars();

    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Lower-cases the first character of `s`, leaving the rest unchanged.
pub(super) fn lower_first(s: &str) -> String {
    let mut chars = s.chars();

    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}
