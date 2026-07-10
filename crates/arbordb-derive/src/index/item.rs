//! One comma-separated entry inside `index(...)`: `name = "..."`, `columns(...)`, or `unique`.

use crate::index::column_spec::ColumnSpec;

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Error, Ident, LitStr, Token};

/// One item of an `index(...)` declaration.
pub(super) enum Item {
    /// `name = "..."`.
    Name(LitStr),
    /// `columns(a, b desc, …)`.
    Columns(Vec<ColumnSpec>),
    /// `unique`.
    Unique,
}

impl Parse for Item {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key = input.parse::<Ident>()?;

        match key.to_string().as_str() {
            "name" => {
                input.parse::<Token![=]>()?;

                Ok(Item::Name(input.parse()?))
            }
            "columns" => {
                let inner;
                syn::parenthesized!(inner in input);

                let columns = Punctuated::<ColumnSpec, Token![,]>::parse_terminated(&inner)?;

                Ok(Item::Columns(columns.into_iter().collect()))
            }
            "unique" => Ok(Item::Unique),
            _ => Err(Error::new(key.span(), "expected `name`, `columns`, or `unique`")),
        }
    }
}
