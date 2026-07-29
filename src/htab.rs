//! Literal translation of `htab.c`'s open-addressed hash table.

// [spec:samurai:def:htab.getle32-fn]
// [spec:samurai:sem:htab.getle32-fn]
fn getle32(bytes: &[u8]) -> u32 {
    u32::from(bytes[0])
        | (u32::from(bytes[1]) << 8)
        | (u32::from(bytes[2]) << 16)
        | (u32::from(bytes[3]) << 24)
}

// [spec:samurai:def:htab.getle64-fn]
// [spec:samurai:sem:htab.getle64-fn]
fn getle64(bytes: &[u8]) -> u64 {
    u64::from(bytes[0])
        | (u64::from(bytes[1]) << 8)
        | (u64::from(bytes[2]) << 16)
        | (u64::from(bytes[3]) << 24)
        | (u64::from(bytes[4]) << 32)
        | (u64::from(bytes[5]) << 40)
        | (u64::from(bytes[6]) << 48)
        | (u64::from(bytes[7]) << 56)
}

// [spec:samurai:def:htab.mum-fn]
// [spec:samurai:sem:htab.mum-fn]
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

struct SegmentedBytes<'a> {
    parts: &'a [&'a [u8]],
    len: usize,
}

impl<'a> SegmentedBytes<'a> {
    fn new(parts: &'a [&'a [u8]]) -> Self {
        Self {
            parts,
            len: parts.iter().map(|part| part.len()).sum(),
        }
    }

    fn byte(&self, mut index: usize) -> u8 {
        for part in self.parts {
            if index < part.len() {
                return part[index];
            }
            index -= part.len();
        }
        unreachable!("segmented byte index is in bounds")
    }

    fn contiguous(&self, mut index: usize, len: usize) -> Option<&[u8]> {
        for part in self.parts {
            if index < part.len() {
                return part.get(index..index + len);
            }
            index -= part.len();
        }
        None
    }

    fn read_u32(&self, index: usize) -> u32 {
        if let Some(bytes) = self.contiguous(index, 4) {
            return getle32(bytes);
        }
        u32::from(self.byte(index))
            | (u32::from(self.byte(index + 1)) << 8)
            | (u32::from(self.byte(index + 2)) << 16)
            | (u32::from(self.byte(index + 3)) << 24)
    }

    fn read_u64(&self, index: usize) -> u64 {
        if let Some(bytes) = self.contiguous(index, 8) {
            return getle64(bytes);
        }
        u64::from(self.read_u32(index)) | (u64::from(self.read_u32(index + 4)) << 32)
    }
}

/// Hash a logical byte sequence without first concatenating its segments.
pub(crate) fn rapidhashv1_parts(parts: &[&[u8]]) -> u64 {
    const SECRET: [u64; 3] = [
        0x2d35_8dcc_aa6c_78a5,
        0x8bb8_4b93_962e_acc9,
        0x4b33_a62e_d433_d4a3,
    ];

    let bytes = SegmentedBytes::new(parts);
    let mut pos = 0usize;
    let mut end = bytes.len;
    let mut seed = [0u64; 3];
    seed[0] = 0xbdd8_9aa9_8270_4029
        ^ mix(0xbdd8_9aa9_8270_4029 ^ SECRET[0], SECRET[1])
        ^ bytes.len as u64;

    match bytes.len {
        0 => {}
        1..=3 => {
            seed[1] = (u64::from(bytes.byte(0)) << 56)
                | (u64::from(bytes.byte(usize::from(bytes.len > 1))) << 32)
                | u64::from(bytes.byte(end - 1));
        }
        4..=16 => {
            seed[1] = (u64::from(bytes.read_u32(pos)) << 32) | u64::from(bytes.read_u32(end - 4));
            if bytes.len >= 8 {
                pos += 4;
                end -= 4;
            }
            seed[2] = (u64::from(bytes.read_u32(pos)) << 32) | u64::from(bytes.read_u32(end - 4));
        }
        _ => {
            seed[1] = seed[0];
            seed[2] = seed[0];
            if bytes.len > 48 {
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
        }
    }

    seed[1] ^= SECRET[1];
    seed[2] ^= seed[0];
    let (low, high) = mum(seed[1], seed[2]);
    mix(low ^ SECRET[0] ^ bytes.len as u64, high ^ SECRET[1])
}

// [spec:samurai:def:htab.rapidhashv1-fn]
// [spec:samurai:sem:htab.rapidhashv1-fn]
#[cfg(test)]
pub(crate) fn rapidhashv1(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        0x5a6e_f770_74eb_c84b
    } else {
        rapidhashv1_parts(&[bytes])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_the_reference_empty_input() {
        assert_eq!(rapidhashv1(b""), 0x5a6e_f770_74eb_c84b);
    }

    #[test]
    fn segmented_hash_matches_contiguous_bytes_at_every_boundary() {
        let bytes = (0..=127).collect::<Vec<u8>>();
        for end in 0..=bytes.len() {
            let expected = rapidhashv1(&bytes[..end]);
            for split in 0..=end {
                assert_eq!(
                    rapidhashv1_parts(&[&bytes[..split], &bytes[split..end]]),
                    expected,
                    "end={end}, split={split}"
                );
            }
        }
    }
}
