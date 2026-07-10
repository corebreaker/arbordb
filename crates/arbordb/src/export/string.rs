//! Shared double-quoted string rendering. JSON strings and YAML double-quoted
//! scalars share an escaping scheme — the JSON escapes are a valid subset of
//! YAML's — so both writers quote through here.

/// Appends `s` to `out` as a double-quoted, escaped string.
pub(super) fn quote(out: &mut String, s: &str) {
    out.push('"');

    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }

    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::quote;

    fn quoted(s: &str) -> String {
        let mut out = String::new();
        quote(&mut out, s);

        out
    }

    #[test]
    fn escapes_the_json_subset() {
        assert_eq!(quoted("a\"b"), "\"a\\\"b\"");
        assert_eq!(quoted("a\\b"), "\"a\\\\b\"");
        assert_eq!(quoted("a\nb"), "\"a\\nb\"");
        assert_eq!(quoted("a\rb"), "\"a\\rb\"");
        assert_eq!(quoted("a\tb"), "\"a\\tb\"");
        assert_eq!(quoted("a\u{08}b"), "\"a\\bb\"");
        assert_eq!(quoted("a\u{0C}b"), "\"a\\fb\"");
    }

    #[test]
    fn other_control_characters_become_unicode_escapes() {
        assert_eq!(quoted("\u{01}"), "\"\\u0001\"");
        assert_eq!(quoted("plain"), "\"plain\"");
    }
}
