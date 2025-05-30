#![allow(unsafe_op_in_unsafe_fn)]
use std::fmt::{Debug, Formatter};

use polars_error::{PolarsResult, polars_ensure};

use crate::nulls::IsNull;

#[cfg(not(feature = "bigidx"))]
pub type IdxSize = u32;
#[cfg(feature = "bigidx")]
pub type IdxSize = u64;

#[cfg(not(feature = "bigidx"))]
pub type NonZeroIdxSize = std::num::NonZeroU32;
#[cfg(feature = "bigidx")]
pub type NonZeroIdxSize = std::num::NonZeroU64;

#[cfg(not(feature = "bigidx"))]
pub type AtomicIdxSize = std::sync::atomic::AtomicU32;
#[cfg(feature = "bigidx")]
pub type AtomicIdxSize = std::sync::atomic::AtomicU64;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct NullableIdxSize {
    pub inner: IdxSize,
}

impl PartialEq<Self> for NullableIdxSize {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl Eq for NullableIdxSize {}

unsafe impl bytemuck::Zeroable for NullableIdxSize {}
unsafe impl bytemuck::AnyBitPattern for NullableIdxSize {}
unsafe impl bytemuck::NoUninit for NullableIdxSize {}

impl Debug for NullableIdxSize {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.inner)
    }
}

impl NullableIdxSize {
    #[inline(always)]
    pub fn is_null_idx(&self) -> bool {
        // The left/right join maintain_order algorithms depend on the special value for sorting
        self.inner == IdxSize::MAX
    }

    #[inline(always)]
    pub const fn null() -> Self {
        Self {
            inner: IdxSize::MAX,
        }
    }

    #[inline(always)]
    pub fn idx(&self) -> IdxSize {
        self.inner
    }

    #[inline(always)]
    pub fn to_opt(&self) -> Option<IdxSize> {
        if self.is_null_idx() {
            None
        } else {
            Some(self.idx())
        }
    }
}

impl From<IdxSize> for NullableIdxSize {
    #[inline(always)]
    fn from(value: IdxSize) -> Self {
        Self { inner: value }
    }
}

pub trait Bounded {
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub trait NullCount {
    fn null_count(&self) -> usize {
        0
    }
}

impl<T: NullCount> NullCount for &T {
    fn null_count(&self) -> usize {
        (*self).null_count()
    }
}

impl<T> Bounded for &[T] {
    fn len(&self) -> usize {
        <[T]>::len(self)
    }
}

impl<T> NullCount for &[T] {
    fn null_count(&self) -> usize {
        0
    }
}

pub trait Indexable {
    type Item: IsNull;

    fn get(&self, i: usize) -> Self::Item;

    /// # Safety
    /// Doesn't do any bound checks.
    unsafe fn get_unchecked(&self, i: usize) -> Self::Item;
}

impl<T: Copy + IsNull> Indexable for &[T] {
    type Item = T;

    fn get(&self, i: usize) -> Self::Item {
        self[i]
    }

    /// # Safety
    /// Doesn't do any bound checks.
    unsafe fn get_unchecked(&self, i: usize) -> Self::Item {
        *<[T]>::get_unchecked(self, i)
    }
}

pub fn check_bounds(idx: &[IdxSize], len: IdxSize) -> PolarsResult<()> {
    // We iterate in large uninterrupted chunks to help auto-vectorization.
    let Some(max_idx) = idx.iter().copied().max() else {
        return Ok(());
    };

    polars_ensure!(max_idx < len, OutOfBounds: "indices are out of bounds");
    Ok(())
}

pub trait ToIdx {
    fn to_idx(self, len: u64) -> IdxSize;
}

macro_rules! impl_to_idx {
    ($ty:ty) => {
        impl ToIdx for $ty {
            #[inline]
            fn to_idx(self, _len: u64) -> IdxSize {
                self as IdxSize
            }
        }
    };
    ($ty:ty, $ity:ty) => {
        impl ToIdx for $ty {
            #[inline]
            fn to_idx(self, len: u64) -> IdxSize {
                let idx = self as $ity;
                if idx < 0 {
                    (idx + len as $ity) as IdxSize
                } else {
                    idx as IdxSize
                }
            }
        }
    };
}

impl_to_idx!(u8);
impl_to_idx!(u16);
impl_to_idx!(u32);
impl_to_idx!(u64);
impl_to_idx!(i8, i16);
impl_to_idx!(i16, i32);
impl_to_idx!(i32, i64);
impl_to_idx!(i64, i64);

// Allows for 2^24 (~16M) chunks
// Leaves 2^40 (~1T) rows per chunk
const DEFAULT_CHUNK_BITS: u64 = 24;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ChunkId<const CHUNK_BITS: u64 = DEFAULT_CHUNK_BITS> {
    swizzled: u64,
}

impl<const CHUNK_BITS: u64> Debug for ChunkId<CHUNK_BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if self.is_null() {
            write!(f, "NULL")
        } else {
            let (chunk, row) = self.extract();
            write!(f, "({chunk}, {row})")
        }
    }
}

impl<const CHUNK_BITS: u64> ChunkId<CHUNK_BITS> {
    #[inline(always)]
    pub const fn null() -> Self {
        Self { swizzled: u64::MAX }
    }

    #[inline(always)]
    pub fn is_null(&self) -> bool {
        self.swizzled == u64::MAX
    }

    #[inline(always)]
    #[allow(clippy::unnecessary_cast)]
    pub fn store(chunk: IdxSize, row: IdxSize) -> Self {
        debug_assert!(chunk < !(u64::MAX << CHUNK_BITS) as IdxSize);
        let swizzled = ((row as u64) << CHUNK_BITS) | chunk as u64;

        Self { swizzled }
    }

    #[inline(always)]
    #[allow(clippy::unnecessary_cast)]
    pub fn extract(self) -> (IdxSize, IdxSize) {
        let row = (self.swizzled >> CHUNK_BITS) as IdxSize;
        let mask = (1u64 << CHUNK_BITS) - 1;
        let chunk = (self.swizzled & mask) as IdxSize;
        (chunk, row)
    }

    #[inline(always)]
    pub fn inner_mut(&mut self) -> &mut u64 {
        &mut self.swizzled
    }

    pub fn from_inner(inner: u64) -> Self {
        Self { swizzled: inner }
    }

    pub fn into_inner(self) -> u64 {
        self.swizzled
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_chunk_idx() {
        let chunk = 213908;
        let row = 813457;

        let ci: ChunkId = ChunkId::store(chunk, row);
        let (c, r) = ci.extract();

        assert_eq!(c, chunk);
        assert_eq!(r, row);
    }
}
