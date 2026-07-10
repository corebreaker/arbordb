//! One column entry inside `columns(...)`: a field name plus an optional direction.

use syn::{
    parse::{Parse, ParseStream},
    Error,
    Ident,
    Result as SynResult,
};

/// A column of an `index(...)`: a field identifier and its `asc` (default) / `desc`.
pub(crate) struct ColumnSpec {
    /// The field this column indexes.
    field:      Ident,
    /// Whether the column sorts descending (`desc`).
    descending: bool,
}

impl ColumnSpec {
    /// The field this column indexes.
    pub(crate) fn field(&self) -> &Ident {
        &self.field
    }

    /// Whether the column sorts descending.
    pub(crate) fn descending(&self) -> bool {
        self.descending
    }
}

impl Parse for ColumnSpec {
    fn parse(input: ParseStream) -> SynResult<Self> {
        let field = input.parse::<Ident>()?;

        let descending = if input.peek(Ident) {
            let direction = input.parse::<Ident>()?;

            match direction.to_string().as_str() {
                "asc" => false,
                "desc" => true,
                _ => return Err(Error::new(direction.span(), "expected `asc` or `desc`")),
            }
        } else {
            false
        };

        Ok(ColumnSpec {
            field,
            descending,
        })
    }
}
