//! `Option<T>` as `AData`: `None` is stored as a present `Null` leaf, `Some(v)` as
//! `v`'s own shape.

use super::{
    refs::{AIdentifiable, AMut, ARef},
    AData,
    Scalar,
};
use crate::{
    access::{Reader, Writer},
    error::AdbResult,
    path::VPath,
};

use std::{marker::PhantomData, sync::Arc};

impl<T: AData> AData for Option<T> {
    type Mut<'t> = OptMut<'t, T>;
    type Ref<'t> = Opt<'t, T>;

    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
        match self {
            None => writer.put_scalar(at, Scalar::Null),
            Some(value) => value.store(writer, at),
        }
    }

    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
        if !reader.exists_at(at)? {
            return Ok(None);
        }

        match reader.scalar_at(at)? {
            Some(Scalar::Null) => Ok(None),
            _ => Ok(Some(T::load(reader, at)?)),
        }
    }
}

/// A read accessor for an optional `T`.
pub struct Opt<'t, T: AData> {
    reader:  Arc<dyn Reader + 't>,
    base:    VPath,
    _marker: PhantomData<fn() -> T>,
}

impl<'t, T: AData> Opt<'t, T> {
    /// Whether a value is present (not a null leaf).
    pub fn is_some(&self) -> AdbResult<bool> {
        if !self.reader.exists_at(&self.base)? {
            return Ok(false);
        }

        Ok(!matches!(self.reader.scalar_at(&self.base)?, Some(Scalar::Null)))
    }

    /// The inner read accessor if a value is present.
    pub fn get(&self) -> AdbResult<Option<T::Ref<'t>>> {
        if !self.is_some()? {
            return Ok(None);
        }

        let inner = <T::Ref<'t> as ARef<'t>>::open(Arc::clone(&self.reader), self.base.clone());

        Ok(Some(inner))
    }
}

impl<T: AData> AIdentifiable for Opt<'_, T> {
    fn path(&self) -> &VPath {
        &self.base
    }
}

impl<'t, T: AData> ARef<'t> for Opt<'t, T> {
    fn open(reader: Arc<dyn Reader + 't>, base: VPath) -> Self {
        Self {
            reader,
            base,
            _marker: PhantomData,
        }
    }
}

/// A read/write accessor for an optional `T`.
pub struct OptMut<'t, T: AData> {
    writer:  Arc<dyn Writer + 't>,
    base:    VPath,
    _marker: PhantomData<fn() -> T>,
}

impl<'t, T: AData> OptMut<'t, T> {
    /// Sets the option, clearing whatever was there first so no stale fields
    /// survive a shape change.
    pub fn set(&self, value: Option<&T>) -> AdbResult<()> {
        self.writer.remove(&self.base)?;

        match value {
            None => self.writer.put_scalar(&self.base, Scalar::Null),
            Some(value) => value.store(&self.writer, &self.base),
        }
    }

    /// Clears the option to `None` (a null leaf).
    pub fn clear(&self) -> AdbResult<()> {
        self.set(None)
    }
}

impl<T: AData> AIdentifiable for OptMut<'_, T> {
    fn path(&self) -> &VPath {
        &self.base
    }
}

impl<'t, T: AData> AMut<'t> for OptMut<'t, T> {
    fn open(writer: Arc<dyn Writer + 't>, base: VPath) -> Self {
        Self {
            writer,
            base,
            _marker: PhantomData,
        }
    }
}
