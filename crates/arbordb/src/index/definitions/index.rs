use super::{Direction, IndexColumn, misc::read_string};
use crate::{
    codec::{self, Reader},
    data::Scalar,
    error::{AdbError, AdbResult},
    index::ordered,
    path::VPath,
};

/// A secondary index definition.
///
/// `pattern` is a path pattern selecting which nodes are indexed *entities*: a
/// slash-separated access path where `*` matches any single child and every other
/// segment matches literally (e.g. `users/*` indexes every direct child of
/// `users`; the empty string `""` indexes the table root). `columns` form the
/// sort key, each a [`VPath`] relative to a matched entity, in priority order. A
/// `unique` index rejects a second entity that produces the same column tuple.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IndexDef {
    /// The index name (unique per table).
    name:    String,
    /// The path pattern selecting indexed entities.
    pattern: String,
    /// The sort-key columns, in priority order.
    columns: Vec<IndexColumn>,
    /// Whether the column tuple must be unique across entities.
    unique:  bool,
}

impl IndexDef {
    /// Assembles a definition from its `name`, entity `pattern`, sort `columns`,
    /// and uniqueness flag.
    pub fn new(name: String, pattern: String, columns: Vec<IndexColumn>, unique: bool) -> Self {
        Self {
            name,
            pattern,
            columns,
            unique,
        }
    }

    /// Appends the registry encoding of this definition to `buf`.
    pub(crate) fn encode(&self, buf: &mut Vec<u8>) {
        codec::put_bytes(buf, self.name.as_bytes());
        codec::put_bytes(buf, self.pattern.as_bytes());
        buf.push(u8::from(self.unique));
        codec::put_u32(buf, self.columns.len() as u32);

        for column in &self.columns {
            codec::put_bytes(buf, column.path().to_string().as_bytes());
            buf.push(column.direction().to_byte());
        }
    }

    /// Reads only the **name** of an encoded definition, advancing `r` past the
    /// whole record. The registry's presence check needs the name alone, so this
    /// skips the pattern, uniqueness flag and columns rather than materializing an
    /// [`IndexDef`] (notably, it never parses a [`VPath`] per column). Returns a
    /// borrow into `r`'s buffer — no allocation.
    pub(crate) fn decode_name<'a>(r: &mut Reader<'a>) -> AdbResult<&'a str> {
        let name = std::str::from_utf8(r.bytes()?)
            .map_err(|_| AdbError::Corrupt("invalid utf-8 in index definition".into()))?;

        let _pattern = r.bytes()?;
        let _unique = r.u8()?;

        let count = r.u32()?;
        for _ in 0..count {
            let _path = r.bytes()?;
            let _direction = r.u8()?;
        }

        Ok(name)
    }

    /// Decodes a definition written by [`IndexDef::encode`].
    pub(crate) fn decode(r: &mut Reader<'_>) -> AdbResult<Self> {
        let name = read_string(r)?;
        let pattern = read_string(r)?;
        let unique = r.u8()? != 0;
        let count = r.u32()?;

        let mut columns = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let path = VPath::parse(&read_string(r)?)?;
            let direction = Direction::from_byte(r.u8()?)?;
            columns.push(IndexColumn::new(path, direction));
        }

        Ok(IndexDef {
            name,
            pattern,
            columns,
            unique,
        })
    }

    /// Encodes `values` (one per column, in column order) into the order-preserving
    /// byte prefix an index key starts with, applying each column's direction.
    /// Shared by write-time maintenance (values read from an entity) and queries
    /// (values supplied by the caller).
    pub(crate) fn encode_columns(&self, values: &[Scalar]) -> Vec<u8> {
        let mut cols = Vec::new();
        for (value, column) in values.iter().zip(&self.columns) {
            ordered::encode_scalar(&mut cols, value, column.direction() == Direction::Desc);
        }

        cols
    }

    /// The index name (unique per table).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The path pattern selecting indexed entities.
    pub fn pattern(&self) -> &str {
        &self.pattern
    }

    /// The sort-key columns, in priority order.
    pub fn columns(&self) -> &Vec<IndexColumn> {
        &self.columns
    }

    /// Whether the column tuple must be unique across entities.
    pub fn unique(&self) -> bool {
        self.unique
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trips() {
        let def = IndexDef::new(
            String::from("by_city_age"),
            String::from("users/*"),
            vec![
                IndexColumn::asc(VPath::root().child_name("city")),
                IndexColumn::desc(VPath::root().child_name("age")),
            ],
            true,
        );

        let mut buf = Vec::new();
        def.encode(&mut buf);

        assert_eq!(IndexDef::decode(&mut Reader::new(&buf)).unwrap(), def);

        // `decode_name` reads only the name, without materializing the columns.
        assert_eq!(IndexDef::decode_name(&mut Reader::new(&buf)).unwrap(), "by_city_age");
    }
}
