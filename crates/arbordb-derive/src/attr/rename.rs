//! The `rename_all` casing rules — the eight serde-compatible casings.
//!
//! Two entry points convert an identifier: [`RenameRule::apply_to_field`] assumes
//! a `snake_case` source (a Rust field), [`RenameRule::apply_to_variant`] a
//! `PascalCase` source (a Rust variant). Casing is ASCII-only; non-ASCII
//! characters pass through unchanged, and there is no acronym grouping
//! (`HTTPServer` snakes to `h_t_t_p_server`).

use super::strings::{capitalize, lower_first, split_pascal};
use syn::{Error, LitStr, Result as SynResult};

/// A casing rule set by `#[arbor(rename_all = "...")]` on a container and applied
/// to every field name / variant tag that has no explicit `rename`.
#[derive(Clone, Copy, Default)]
pub(crate) enum RenameRule {
    /// No transformation — the identifier is stored verbatim.
    #[default]
    None,
    /// `lowercase`
    Lower,
    /// `UPPERCASE`
    Upper,
    /// `PascalCase`
    Pascal,
    /// `camelCase`
    Camel,
    /// `snake_case`
    Snake,
    /// `SCREAMING_SNAKE_CASE`
    ScreamingSnake,
    /// `kebab-case`
    Kebab,
    /// `SCREAMING-KEBAB-CASE`
    ScreamingKebab,
}

impl RenameRule {
    /// Parses a `rename_all` string literal, erroring on an unknown casing.
    pub(crate) fn from_lit(lit: &LitStr) -> SynResult<Self> {
        let rule = match lit.value().as_str() {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            // no-coverage:start — attribute-validation error (invalid input never compiles)
            other => {
                return Err(Error::new(
                    lit.span(),
                    format!(
                        "unknown rename rule `{other}`; expected one of lowercase, UPPERCASE, \
                         PascalCase, camelCase, snake_case, SCREAMING_SNAKE_CASE, kebab-case, \
                         SCREAMING-KEBAB-CASE"
                    ),
                ));
            } // no-coverage:stop
        };

        Ok(rule)
    }

    /// Applies the rule to a struct field name (a Rust field is `snake_case`).
    pub(crate) fn apply_to_field(self, field: &str) -> String {
        match self {
            Self::None | Self::Lower | Self::Snake => field.to_owned(),
            Self::Upper | Self::ScreamingSnake => field.to_ascii_uppercase(),
            Self::Pascal => field.split('_').map(capitalize).collect(),
            Self::Camel => lower_first(&field.split('_').map(capitalize).collect::<String>()),
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => field.to_ascii_uppercase().replace('_', "-"),
        }
    }

    /// Applies the rule to an enum variant name (a Rust variant is `PascalCase`).
    pub(crate) fn apply_to_variant(self, variant: &str) -> String {
        match self {
            Self::None | Self::Pascal => variant.to_owned(),
            Self::Lower => variant.to_ascii_lowercase(),
            Self::Upper => variant.to_ascii_uppercase(),
            Self::Camel => lower_first(variant),
            Self::Snake => split_pascal(variant, '_', false),
            Self::ScreamingSnake => split_pascal(variant, '_', true),
            Self::Kebab => split_pascal(variant, '-', false),
            Self::ScreamingKebab => split_pascal(variant, '-', true),
        }
    }
}
