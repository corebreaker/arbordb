//! `Vec<T>` as `AData`: a list vnode with one `T` vnode per element.

use super::{
    refs::{AIdentifiable, AMut, ARef},
    AData,
    NodeEncoder,
};

use crate::{
    access::{Reader, Writer},
    error::AdbResult,
    path::VPath,
};

use std::{marker::PhantomData, sync::Arc};

impl<T: AData> AData for Vec<T> {
    type Mut<'t> = SeqMut<'t, T>;
    type Ref<'t> = Seq<'t, T>;

    fn store<W: Writer>(&self, writer: &W, at: &VPath) -> AdbResult<()> {
        writer.ensure_container(at, true)?;
        for (index, item) in self.iter().enumerate() {
            item.store(writer, &at.child_index(index as u64))?;
        }

        Ok(())
    }

    fn encode_node(&self, enc: &mut NodeEncoder) -> AdbResult<u32> {
        // Children first: emit each element, collecting its offset, then the list vnode.
        let mut offsets = Vec::with_capacity(self.len());
        for item in self {
            offsets.push(item.encode_node(enc)?);
        }

        Ok(enc.list(&offsets))
    }

    fn load<R: Reader>(reader: &R, at: &VPath) -> AdbResult<Self> {
        let len = reader.len_at(at)?;

        let mut out = Vec::with_capacity(len);
        for index in 0..len {
            out.push(T::load(reader, &at.child_index(index as u64))?);
        }

        Ok(out)
    }
}

/// A read accessor for a list of `T`.
pub struct Seq<'t, T: AData> {
    /// The cursor this accessor reads through.
    reader:  Arc<dyn Reader + 't>,
    /// The intra-value path of the list.
    base:    VPath,
    /// Binds the element type `T` without storing one.
    _marker: PhantomData<fn() -> T>,
}

impl<'t, T: AData> Seq<'t, T> {
    /// The number of elements.
    pub fn len(&self) -> AdbResult<usize> {
        self.reader.len_at(&self.base)
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> AdbResult<bool> {
        Ok(self.len()? == 0)
    }

    /// A read accessor for the element at `index`, or `None` if out of range.
    pub fn get(&self, index: usize) -> AdbResult<Option<T::Ref<'t>>> {
        if index >= self.len()? {
            return Ok(None);
        }

        let element = <T::Ref<'t> as ARef<'t>>::open(Arc::clone(&self.reader), self.base.child_index(index as u64));

        Ok(Some(element))
    }
}

impl<T: AData> AIdentifiable for Seq<'_, T> {
    fn path(&self) -> &VPath {
        &self.base
    }
}

impl<'t, T: AData> ARef<'t> for Seq<'t, T> {
    fn open(reader: Arc<dyn Reader + 't>, base: VPath) -> Self {
        Self {
            reader,
            base,
            _marker: PhantomData,
        }
    }
}

/// A read/write accessor for a list of `T`.
pub struct SeqMut<'t, T: AData> {
    /// The cursor this accessor reads and writes through.
    writer:  Arc<dyn Writer + 't>,
    /// The intra-value path of the list.
    base:    VPath,
    /// Binds the element type `T` without storing one.
    _marker: PhantomData<fn() -> T>,
}

impl<'t, T: AData> SeqMut<'t, T> {
    /// The number of elements.
    pub fn len(&self) -> AdbResult<usize> {
        self.writer.len_at(&self.base)
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> AdbResult<bool> {
        Ok(self.len()? == 0)
    }

    /// A write accessor for the element at `index`, or `None` if out of range.
    pub fn get(&self, index: usize) -> AdbResult<Option<T::Mut<'t>>> {
        if index >= self.len()? {
            return Ok(None);
        }

        let element = <T::Mut<'t> as AMut<'t>>::open(Arc::clone(&self.writer), self.base.child_index(index as u64));

        Ok(Some(element))
    }

    /// Appends `value` to the end of the list.
    pub fn push(&self, value: &T) -> AdbResult<()> {
        let index = self.len()? as u64;

        value.store(&self.writer, &self.base.child_index(index))
    }
}

impl<T: AData> AIdentifiable for SeqMut<'_, T> {
    fn path(&self) -> &VPath {
        &self.base
    }
}

impl<'t, T: AData> AMut<'t> for SeqMut<'t, T> {
    fn open(writer: Arc<dyn Writer + 't>, base: VPath) -> Self {
        Self {
            writer,
            base,
            _marker: PhantomData,
        }
    }
}
