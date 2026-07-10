//! `BTreeMap<String, T>` as `AData`: an object node with one `T` node per entry.

use super::{
    refs::{AIdentifiable, AMut, ARef},
    AData,
};
use crate::{
    access::{Reader, Writer},
    error::AdbResult,
    path::VPath,
};

use std::{collections::BTreeMap, marker::PhantomData, sync::Arc};

impl<T: AData> AData for BTreeMap<String, T> {
    type Mut<'t> = MapMut<'t, T>;
    type Ref<'t> = Map<'t, T>;

    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
        writer.ensure_container(at, false)?;
        for (name, value) in self {
            value.store(writer, &at.child_name(name))?;
        }

        Ok(())
    }

    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
        let mut map = BTreeMap::new();
        for name in reader.keys_at(at)? {
            let value = T::load(reader, &at.child_name(&name))?;
            map.insert(name, value);
        }

        Ok(map)
    }
}

/// A read accessor for a `String`-keyed map of `T`.
pub struct Map<'t, T: AData> {
    reader:  Arc<dyn Reader + 't>,
    base:    VPath,
    _marker: PhantomData<fn() -> T>,
}

impl<'t, T: AData> Map<'t, T> {
    /// The entry names, in order.
    pub fn keys(&self) -> AdbResult<Vec<String>> {
        self.reader.keys_at(&self.base)
    }

    /// The number of entries.
    pub fn len(&self) -> AdbResult<usize> {
        Ok(self.keys()?.len())
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> AdbResult<bool> {
        Ok(self.len()? == 0)
    }

    /// Whether the map contains `name`.
    pub fn contains_key(&self, name: &str) -> AdbResult<bool> {
        self.reader.exists_at(&self.base.child_name(name))
    }

    /// A read accessor for the value under `name`, or `None` if absent.
    pub fn get(&self, name: &str) -> AdbResult<Option<T::Ref<'t>>> {
        if !self.contains_key(name)? {
            return Ok(None);
        }

        let value = <T::Ref<'t> as ARef<'t>>::open(Arc::clone(&self.reader), self.base.child_name(name));

        Ok(Some(value))
    }
}

impl<T: AData> AIdentifiable for Map<'_, T> {
    fn path(&self) -> &VPath {
        &self.base
    }
}

impl<'t, T: AData> ARef<'t> for Map<'t, T> {
    fn open(reader: Arc<dyn Reader + 't>, base: VPath) -> Self {
        Self {
            reader,
            base,
            _marker: PhantomData,
        }
    }
}

/// A read/write accessor for a `String`-keyed map of `T`.
pub struct MapMut<'t, T: AData> {
    writer:  Arc<dyn Writer + 't>,
    base:    VPath,
    _marker: PhantomData<fn() -> T>,
}

impl<'t, T: AData> MapMut<'t, T> {
    /// The entry names, in order.
    pub fn keys(&self) -> AdbResult<Vec<String>> {
        self.writer.keys_at(&self.base)
    }

    /// A write accessor for the value under `name`, or `None` if absent.
    pub fn get(&self, name: &str) -> AdbResult<Option<T::Mut<'t>>> {
        if !self.writer.exists_at(&self.base.child_name(name))? {
            return Ok(None);
        }

        let value = <T::Mut<'t> as AMut<'t>>::open(Arc::clone(&self.writer), self.base.child_name(name));

        Ok(Some(value))
    }

    /// Inserts (or replaces) the `name` entry, clearing any prior value first.
    pub fn insert(&self, name: &str, value: &T) -> AdbResult<()> {
        let at = self.base.child_name(name);
        self.writer.remove(&at)?;

        value.store(&self.writer, &at)
    }

    /// Removes the `name` entry, returning whether it was present.
    pub fn remove(&self, name: &str) -> AdbResult<bool> {
        self.writer.remove(&self.base.child_name(name))
    }
}

impl<T: AData> AIdentifiable for MapMut<'_, T> {
    fn path(&self) -> &VPath {
        &self.base
    }
}

impl<'t, T: AData> AMut<'t> for MapMut<'t, T> {
    fn open(writer: Arc<dyn Writer + 't>, base: VPath) -> Self {
        Self {
            writer,
            base,
            _marker: PhantomData,
        }
    }
}
