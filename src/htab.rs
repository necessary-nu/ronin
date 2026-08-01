//! Literal translation of `htab.c`'s open-addressed hash table.

// [spec:samurai:def:htab.getle32-fn]
// [spec:samurai:sem:htab.getle32-fn]
fn getle32(bytes: &[u8], index: usize) -> u32 {
    u32::from_le_bytes(
        bytes[index..index + 4]
            .try_into()
            .expect("rapidhash reads four in-bounds bytes"),
    )
}

// [spec:samurai:def:htab.getle64-fn]
// [spec:samurai:sem:htab.getle64-fn]
fn getle64(bytes: &[u8], index: usize) -> u64 {
    u64::from_le_bytes(
        bytes[index..index + 8]
            .try_into()
            .expect("rapidhash reads eight in-bounds bytes"),
    )
}

// [spec:samurai:def:htab.mum-fn]
// [spec:samurai:sem:htab.mum-fn]
#[allow(
    clippy::cast_possible_truncation,
    reason = "each cast keeps exactly the half of the 128-bit product it names"
)]
fn mum(a: u64, b: u64) -> (u64, u64) {
    let product = u128::from(a) * u128::from(b);
    (product as u64, (product >> 64) as u64)
}

// [spec:samurai:def:htab.mix-fn]
// [spec:samurai:sem:htab.mix-fn]
fn mix(a: u64, b: u64) -> u64 {
    let (low, high) = mum(a, b);
    low ^ high
}

/// Word reads the hash needs, kept as required methods so the contiguous
/// implementation compiles to a bare load: a shared default body carrying the
/// segmented fallback is too large for the optimiser to inline, and every
/// four-byte read of a path would then pay a call.
pub(crate) trait RapidBytes {
    fn len(&self) -> usize;
    fn byte(&self, index: usize) -> u8;
    fn read_u32(&self, index: usize) -> u32;
    fn read_u64(&self, index: usize) -> u64;
}

impl RapidBytes for [u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn byte(&self, index: usize) -> u8 {
        self[index]
    }

    fn read_u32(&self, index: usize) -> u32 {
        getle32(self, index)
    }

    fn read_u64(&self, index: usize) -> u64 {
        getle64(self, index)
    }
}

/// The `len` bytes at `index` when one segment holds all of them.
fn segment<'a>(parts: &[&'a [u8]], mut index: usize, len: usize) -> Option<&'a [u8]> {
    for part in parts {
        if index < part.len() {
            return part.get(index..index.checked_add(len)?);
        }
        index -= part.len();
    }
    None
}

impl RapidBytes for [&[u8]] {
    fn len(&self) -> usize {
        self.iter().map(|part| part.len()).sum()
    }

    fn byte(&self, mut index: usize) -> u8 {
        for part in self {
            if index < part.len() {
                return part[index];
            }
            index -= part.len();
        }
        unreachable!("segmented byte index is in bounds")
    }

    fn read_u32(&self, index: usize) -> u32 {
        if let Some(bytes) = segment(self, index, 4) {
            return getle32(bytes, 0);
        }
        u32::from(self.byte(index))
            | (u32::from(self.byte(index + 1)) << 8)
            | (u32::from(self.byte(index + 2)) << 16)
            | (u32::from(self.byte(index + 3)) << 24)
    }

    fn read_u64(&self, index: usize) -> u64 {
        if let Some(bytes) = segment(self, index, 8) {
            return getle64(bytes, 0);
        }
        u64::from(self.read_u32(index)) | (u64::from(self.read_u32(index + 4)) << 32)
    }
}

const SECRET: [u64; 3] = [
    0x2d35_8dcc_aa6c_78a5,
    0x8bb8_4b93_962e_acc9,
    0x4b33_a62e_d433_d4a3,
];

/// Reduce the three seeds and the input length to the finished hash.
fn finish(seed: [u64; 3], len: usize) -> u64 {
    let (low, high) = mum(seed[1] ^ SECRET[1], seed[2] ^ seed[0]);
    mix(low ^ SECRET[0] ^ len as u64, high ^ SECRET[1])
}

/// Hash the inputs longer than sixteen bytes, whose seeds come from folding
/// the bytes in blocks rather than from the ends alone.
///
/// Held out of line, and kept there: everything sixteen bytes and under is a
/// leaf call that keeps its seeds in registers, and inlining these loops would
/// saddle it with a stack frame and six callee-saved registers it never reads.
#[inline(never)]
fn wide_hash(bytes: &(impl RapidBytes + ?Sized), len: usize, seed0: u64) -> u64 {
    let mut pos = 0usize;
    let end = len;
    let mut seed = [seed0; 3];
    if len > 48 {
        while end - pos >= 48 {
            for i in 0..3 {
                seed[i] = mix(
                    bytes.read_u64(pos) ^ SECRET[i],
                    bytes.read_u64(pos + 8) ^ seed[i],
                );
                pos += 16;
            }
        }
        seed[0] ^= seed[1] ^ seed[2];
    }
    if end - pos > 16 {
        seed[0] ^= SECRET[1];
        while end - pos > 16 {
            seed[0] = mix(
                bytes.read_u64(pos) ^ SECRET[2],
                bytes.read_u64(pos + 8) ^ seed[0],
            );
            pos += 16;
        }
    }
    seed[1] = bytes.read_u64(end - 16);
    seed[2] = bytes.read_u64(end - 8);
    finish(seed, len)
}

// [spec:samurai:def:htab.rapidhashv1-fn]
// [spec:samurai:sem:htab.rapidhashv1-fn]
/// Hash a logical byte sequence supplied contiguously or as segments.
pub(crate) fn rapidhashv1(bytes: &(impl RapidBytes + ?Sized)) -> u64 {
    let len = bytes.len();
    let mut pos = 0usize;
    let mut end = len;
    let seed0 =
        0xbdd8_9aa9_8270_4029 ^ mix(0xbdd8_9aa9_8270_4029 ^ SECRET[0], SECRET[1]) ^ len as u64;
    let mut seed = [seed0, 0, 0];

    match len {
        0 => {}
        1..=3 => {
            seed[1] = (u64::from(bytes.byte(0)) << 56)
                | (u64::from(bytes.byte(usize::from(len > 1))) << 32)
                | u64::from(bytes.byte(end - 1));
        }
        4..=16 => {
            seed[1] = (u64::from(bytes.read_u32(pos)) << 32) | u64::from(bytes.read_u32(end - 4));
            if len >= 8 {
                pos += 4;
                end -= 4;
            }
            seed[2] = (u64::from(bytes.read_u32(pos)) << 32) | u64::from(bytes.read_u32(end - 4));
        }
        _ => return wide_hash(bytes, len, seed0),
    }

    finish(seed, len)
}

/// One-shot rapidhash adapted to std's streaming [`Hasher`] contract.
///
/// Build manifests are trusted input — executing them runs arbitrary
/// commands — so path- and log-keyed maps follow Ninja and C samurai in
/// using a fixed-seed hash instead of `SipHash` denial-of-service hardening.
/// These maps do not expose iteration order as program semantics.
#[derive(Default)]
pub(crate) struct RapidHasher(u64);

type RapidBuildHasher = std::hash::BuildHasherDefault<RapidHasher>;
pub(crate) type RapidHashMap<K, V> = std::collections::HashMap<K, V, RapidBuildHasher>;
pub(crate) type RapidHashSet<K> = std::collections::HashSet<K, RapidBuildHasher>;

impl std::hash::Hasher for RapidHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0 = self.0.rotate_left(21) ^ rapidhashv1(bytes);
    }

    fn write_u8(&mut self, value: u8) {
        self.write_u64(u64::from(value));
    }

    fn write_u32(&mut self, value: u32) {
        self.write_u64(u64::from(value));
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = self.0.rotate_left(21) ^ value.wrapping_mul(SECRET[2]);
    }

    fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_the_reference_empty_input() {
        assert_eq!(rapidhashv1(b"".as_slice()), 0x5a6e_f770_74eb_c84b);
    }

    #[test]
    fn streaming_hasher_is_deterministic_and_write_sensitive() {
        use std::hash::Hasher as _;

        let hash = |writes: &[&[u8]]| {
            let mut hasher = RapidHasher::default();
            for write in writes {
                hasher.write(write);
            }
            hasher.finish()
        };
        assert_eq!(hash(&[b"out/main.o"]), hash(&[b"out/main.o"]));
        assert_ne!(hash(&[b"out/main.o"]), hash(&[b"out/main2.o"]));
        assert_ne!(hash(&[b"ab", b"c"]), hash(&[b"a", b"bc"]));

        let mut integers = RapidHasher::default();
        integers.write_usize(7);
        let mut other = RapidHasher::default();
        other.write_usize(8);
        assert_ne!(integers.finish(), other.finish());
    }

    #[test]
    fn rapid_map_preserves_borrowed_byte_lookup() {
        let mut paths = RapidHashMap::default();
        paths.insert(b"out/main.o".to_vec(), 7);

        assert_eq!(paths.get(b"out/main.o".as_slice()), Some(&7));
        assert_eq!(paths.get(b"out/missing.o".as_slice()), None);
    }

    #[test]
    fn segmented_hash_matches_contiguous_bytes_at_every_boundary() {
        let bytes = (0..=127).collect::<Vec<u8>>();
        for end in 0..=bytes.len() {
            let expected = rapidhashv1(&bytes[..end]);
            for split in 0..=end {
                assert_eq!(
                    rapidhashv1(&[&bytes[..split], &bytes[split..end]][..]),
                    expected,
                    "end={end}, split={split}"
                );
            }
        }
    }
}
