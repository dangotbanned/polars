use std::hash::{Hash, Hasher};

use crate::nulls::IsNull;

// Hash combine from c++' boost lib.
#[inline(always)]
pub fn _boost_hash_combine(l: u64, r: u64) -> u64 {
    l ^ r.wrapping_add(0x9e3779b9u64.wrapping_add(l << 6).wrapping_add(r >> 2))
}

#[inline(always)]
pub const fn folded_multiply(a: u64, b: u64) -> u64 {
    let full = (a as u128).wrapping_mul(b as u128);
    (full as u64) ^ ((full >> 64) as u64)
}

/// Contains a byte slice and a precomputed hash for that string.
/// During rehashes, we will rehash the hash instead of the string, that makes
/// rehashing cheap and allows cache coherent small hash tables.
#[derive(Eq, Copy, Clone, Debug)]
pub struct BytesHash<'a> {
    payload: Option<&'a [u8]>,
    pub(super) hash: u64,
}

impl<'a> BytesHash<'a> {
    #[inline]
    pub fn new(s: Option<&'a [u8]>, hash: u64) -> Self {
        Self { payload: s, hash }
    }
}

impl<'a> IsNull for BytesHash<'a> {
    const HAS_NULLS: bool = true;
    type Inner = BytesHash<'a>;

    #[inline(always)]
    fn is_null(&self) -> bool {
        self.payload.is_none()
    }

    fn unwrap_inner(self) -> Self::Inner {
        assert!(self.payload.is_some());
        self
    }
}

impl Hash for BytesHash<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash)
    }
}

impl PartialEq for BytesHash<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        (self.hash == other.hash) && (self.payload == other.payload)
    }
}

#[inline(always)]
pub fn hash_to_partition(h: u64, n_partitions: usize) -> usize {
    // Assuming h is a 64-bit random number, we note that
    // h / 2^64 is almost a uniform random number in [0, 1), and thus
    // floor(h * n_partitions / 2^64) is almost a uniform random integer in
    // [0, n_partitions). Despite being written with u128 multiplication this
    // compiles to a single mul / mulhi instruction on x86-x64/aarch64.
    ((h as u128 * n_partitions as u128) >> 64) as usize
}

#[derive(Clone)]
pub struct HashPartitioner {
    num_partitions: usize,
    seed: u64,
}

impl HashPartitioner {
    /// Creates a new hash partitioner with the given number of partitions and
    /// seed.
    #[inline]
    pub fn new(num_partitions: usize, mut seed: u64) -> Self {
        assert!(num_partitions > 0);
        // Make sure seeds bits are properly randomized, and is odd.
        const ARBITRARY1: u64 = 0x85921e81c41226a0;
        const ARBITRARY2: u64 = 0x3bc1d0faba166294;
        const ARBITRARY3: u64 = 0xfbde893e21a73756;
        seed = folded_multiply(seed ^ ARBITRARY1, ARBITRARY2);
        seed = folded_multiply(seed, ARBITRARY3);
        seed |= 1;
        Self {
            seed,
            num_partitions,
        }
    }

    /// Converts a hash to a partition. It is guaranteed that the output is
    /// in the range [0, n_partitions), and that independent HashPartitioners
    /// that we initialized with the same num_partitions and seed return the same
    /// partition.
    #[inline(always)]
    pub fn hash_to_partition(&self, hash: u64) -> usize {
        // Assuming r is a 64-bit random number, we note that
        // r / 2^64 is almost a uniform random number in [0, 1), and thus
        // floor(r * n_partitions / 2^64) is almost a uniform random integer in
        // [0, n_partitions). Despite being written with u128 multiplication this
        // compiles to a single mul / mulhi instruction on x86-x64/aarch64.
        let shuffled = hash.wrapping_mul(self.seed);
        ((shuffled as u128 * self.num_partitions as u128) >> 64) as usize
    }

    /// The partition nulls are put into.
    #[inline(always)]
    pub fn null_partition(&self) -> usize {
        0
    }

    #[inline(always)]
    pub fn num_partitions(&self) -> usize {
        self.num_partitions
    }
}

// FIXME: use Hasher interface and support a random state.
pub trait DirtyHash {
    // A quick and dirty hash. Only the top bits of the hash are decent, such as
    // used in hash_to_partition.
    fn dirty_hash(&self) -> u64;
}

// Multiplication by a 'random' odd number gives a universal hash function in
// the top bits.
const RANDOM_ODD: u64 = 0x55fbfd6bfc5458e9;

macro_rules! impl_hash_partition_as_u64 {
    ($T: ty) => {
        impl DirtyHash for $T {
            fn dirty_hash(&self) -> u64 {
                (*self as u64).wrapping_mul(RANDOM_ODD)
            }
        }
    };
}

impl_hash_partition_as_u64!(u8);
impl_hash_partition_as_u64!(u16);
impl_hash_partition_as_u64!(u32);
impl_hash_partition_as_u64!(u64);
impl_hash_partition_as_u64!(i8);
impl_hash_partition_as_u64!(i16);
impl_hash_partition_as_u64!(i32);
impl_hash_partition_as_u64!(i64);

impl DirtyHash for i128 {
    fn dirty_hash(&self) -> u64 {
        (*self as u64)
            .wrapping_mul(RANDOM_ODD)
            .wrapping_add((*self >> 64) as u64)
    }
}

impl DirtyHash for BytesHash<'_> {
    fn dirty_hash(&self) -> u64 {
        self.hash
    }
}

impl<T: DirtyHash + ?Sized> DirtyHash for &T {
    fn dirty_hash(&self) -> u64 {
        (*self).dirty_hash()
    }
}

// FIXME: we should probably encourage explicit null handling, but for now we'll
// allow directly getting a partition from a nullable value.
impl<T: DirtyHash> DirtyHash for Option<T> {
    fn dirty_hash(&self) -> u64 {
        self.as_ref().map(|s| s.dirty_hash()).unwrap_or(0)
    }
}
