//! The parsed `#[arbor(index(...))]` declaration.

use crate::index::{column_spec::ColumnSpec, item::Item};
use syn::{parse::ParseStream, punctuated::Punctuated, Error, LitStr, Token, Result as SynResult};

/// A parsed `index(name = "...", columns(...), unique)` declaration.
pub(crate) struct IndexAttr {
    name:    LitStr,
    columns: Vec<ColumnSpec>,
    unique:  bool,
}

impl IndexAttr {
    /// Parses the body of an `index(...)` item — `input` is the parenthesized
    /// content (`name = "x", columns(a), unique`).
    pub(crate) fn from_body(input: ParseStream) -> SynResult<Self> {
        let span = input.span();
        let items = Punctuated::<Item, Token![,]>::parse_terminated(input)?;

        let mut name = None;
        let mut columns = None;
        let mut unique = false;

        for item in items {
            match item {
                Item::Name(value) => name = Some(value),
                Item::Columns(value) => columns = Some(value),
                Item::Unique => unique = true,
            }
        }

        let name = name.ok_or_else(|| Error::new(span, "index requires `name = \"...\"`"))?;
        let columns = columns.ok_or_else(|| Error::new(span, "index requires `columns(...)`"))?;

        if columns.is_empty() {
            return Err(Error::new(span, "index requires at least one column"));
        }

        Ok(IndexAttr {
            name,
            columns,
            unique,
        })
    }

    /// The index name.
    pub(crate) fn name(&self) -> &LitStr {
        &self.name
    }

    /// The index columns, in priority order.
    pub(crate) fn columns(&self) -> &Vec<ColumnSpec> {
        &self.columns
    }

    /// Whether the index is unique.
    pub(crate) fn unique(&self) -> bool {
        self.unique
    }
}
