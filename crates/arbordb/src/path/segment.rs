use smallvec::SmallVec;
use smol_str::SmolStr;

/// The inline capacity of a path's segment buffer. The hot read/write paths build
/// short paths — a field appended to an anchor — so a small inline buffer keeps
/// `child_name` (and the whole path it clones) off the heap; deeper paths spill.
/// Kept deliberately small: [`VPath`](super::VPath) is embedded in
/// [`AdbError`](crate::AdbError) and thus in every `AdbResult`, so a fat inline
/// buffer would bloat every result.
const INLINE_SEGMENTS: usize = 2;

/// A path's segment storage: inline up to [`INLINE_SEGMENTS`], heap beyond.
pub(crate) type Segments = SmallVec<[Segment; INLINE_SEGMENTS]>;

/// One component of a [`VPath`](super::VPath).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Segment {
    /// An object field name, e.g. `h` in `a/h`. Held as a [`SmolStr`]: field names
    /// are short and immutable, so this keeps a name inline (no heap allocation)
    /// on the hot path where every child navigation appends one to a path.
    Name(SmolStr),
    /// A zero-based list index, e.g. `5` in `a/t[5]`.
    Index(u64),
}
