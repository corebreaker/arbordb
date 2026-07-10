//! Scalar leaf accessors: [`Leaf`] (read) and [`LeafMut`] (read/write).

use super::{
    refs::{AIdentifiable, AMut, ARef},
    AValue,
};

use crate::{
    access::{Reader, Writer},
    error::{AdbError, AdbResult},
    path::VPath,
};

use std::{marker::PhantomData, sync::Arc};

/// A read accessor for a single scalar leaf of type `T`.
pub struct Leaf<'t, T: AValue> {
    /// The cursor this accessor reads through.
    reader:  Arc<dyn Reader + 't>,
    /// The intra-value path this leaf is anchored at.
    base:    VPath,
    /// Binds the scalar type `T` without storing one.
    _marker: PhantomData<fn() -> T>,
}

impl<'t, T: AValue> Leaf<'t, T> {
    /// Reads and decodes the scalar; errors if nothing is stored there.
    pub fn get(&self) -> AdbResult<T> {
        match self.reader.scalar_at(&self.base)? {
            Some(scalar) => T::from_scalar_owned(scalar),
            None => Err(AdbError::PathNotFound(self.base.clone())),
        }
    }
}

impl<T: AValue> AIdentifiable for Leaf<'_, T> {
    fn path(&self) -> &VPath {
        &self.base
    }
}

impl<'t, T: AValue> ARef<'t> for Leaf<'t, T> {
    fn open(reader: Arc<dyn Reader + 't>, base: VPath) -> Self {
        Self {
            reader,
            base,
            _marker: PhantomData,
        }
    }
}

/// A read/write accessor for a single scalar leaf of type `T`.
pub struct LeafMut<'t, T: AValue> {
    /// The cursor this accessor reads and writes through.
    writer:  Arc<dyn Writer + 't>,
    /// The intra-value path this leaf is anchored at.
    base:    VPath,
    /// Binds the scalar type `T` without storing one.
    _marker: PhantomData<fn() -> T>,
}

impl<'t, T: AValue> LeafMut<'t, T> {
    /// Reads and decodes the scalar (a writer can read its own state).
    pub fn get(&self) -> AdbResult<T> {
        match self.writer.scalar_at(&self.base)? {
            Some(scalar) => T::from_scalar_owned(scalar),
            None => Err(AdbError::PathNotFound(self.base.clone())),
        }
    }

    /// Sets the scalar.
    pub fn set(&self, value: &T) -> AdbResult<()> {
        self.writer.put_scalar(&self.base, value.to_scalar())
    }
}

impl<T: AValue> AIdentifiable for LeafMut<'_, T> {
    fn path(&self) -> &VPath {
        &self.base
    }
}

impl<'t, T: AValue> AMut<'t> for LeafMut<'t, T> {
    fn open(writer: Arc<dyn Writer + 't>, base: VPath) -> Self {
        Self {
            writer,
            base,
            _marker: PhantomData,
        }
    }
}
