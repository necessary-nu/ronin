//! Literal translation of `htab.c`'s open-addressed hash table.

// [spec:samurai:def:htab.hashtablekey]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HashTableKey {
    pub hash: u64,
    pub bytes: Vec<u8>,
}

// [spec:samurai:def:htab.hashtable]
pub struct HashTable<V> {
    pub len: usize,
    pub cap: usize,
    slots: Vec<Option<(HashTableKey, Option<V>)>>,
}

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

// [spec:samurai:def:htab.rapidhashv1-fn]
// [spec:samurai:sem:htab.rapidhashv1-fn]
pub fn rapidhashv1(bytes: &[u8]) -> u64 {
    const SECRET: [u64; 3] = [
        0x2d35_8dcc_aa6c_78a5,
        0x8bb8_4b93_962e_acc9,
        0x4b33_a62e_d433_d4a3,
    ];

    let mut pos = 0usize;
    let mut end = bytes.len();
    let mut seed = [0u64; 3];
    seed[0] = 0xbdd8_9aa9_8270_4029
        ^ mix(0xbdd8_9aa9_8270_4029 ^ SECRET[0], SECRET[1])
        ^ bytes.len() as u64;

    match bytes.len() {
        0 => {}
        1..=3 => {
            seed[1] = (u64::from(bytes[0]) << 56)
                | (u64::from(bytes[usize::from(bytes.len() > 1)]) << 32)
                | u64::from(bytes[end - 1]);
        }
        4..=16 => {
            seed[1] =
                (u64::from(getle32(&bytes[pos..])) << 32) | u64::from(getle32(&bytes[end - 4..]));
            if bytes.len() >= 8 {
                pos += 4;
                end -= 4;
            }
            seed[2] =
                (u64::from(getle32(&bytes[pos..])) << 32) | u64::from(getle32(&bytes[end - 4..]));
        }
        _ => {
            seed[1] = seed[0];
            seed[2] = seed[0];
            if bytes.len() > 48 {
                while end - pos >= 48 {
                    for i in 0..3 {
                        seed[i] = mix(
                            getle64(&bytes[pos..]) ^ SECRET[i],
                            getle64(&bytes[pos + 8..]) ^ seed[i],
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
                        getle64(&bytes[pos..]) ^ SECRET[2],
                        getle64(&bytes[pos + 8..]) ^ seed[0],
                    );
                    pos += 16;
                }
            }
            seed[1] = getle64(&bytes[end - 16..]);
            seed[2] = getle64(&bytes[end - 8..]);
        }
    }

    seed[1] ^= SECRET[1];
    seed[2] ^= seed[0];
    let (low, high) = mum(seed[1], seed[2]);
    mix(low ^ SECRET[0] ^ bytes.len() as u64, high ^ SECRET[1])
}

// [spec:samurai:def:htab.htabkey-fn]
// [spec:samurai:sem:htab.htabkey-fn]
pub fn htabkey(bytes: &[u8]) -> HashTableKey {
    HashTableKey {
        hash: rapidhashv1(bytes),
        bytes: bytes.to_vec(),
    }
}

// [spec:samurai:def:htab.mkhtab-fn]
// [spec:samurai:sem:htab.mkhtab-fn]
pub fn mkhtab<V>(cap: usize) -> HashTable<V> {
    assert!(cap != 0 && cap.is_power_of_two());
    HashTable {
        len: 0,
        cap,
        slots: std::iter::repeat_with(|| None).take(cap).collect(),
    }
}

impl<V> HashTable<V> {
    // [spec:samurai:def:htab.keyequal-fn]
    // [spec:samurai:sem:htab.keyequal-fn]
    fn keyequal(left: &HashTableKey, right: &HashTableKey) -> bool {
        left.hash == right.hash && left.bytes == right.bytes
    }

    // [spec:samurai:def:htab.keyindex-fn]
    // [spec:samurai:sem:htab.keyindex-fn]
    fn keyindex(&self, key: &HashTableKey) -> usize {
        let mask = self.cap - 1;
        let mut index = key.hash as usize & mask;
        while let Some((stored, _)) = &self.slots[index] {
            if Self::keyequal(stored, key) {
                break;
            }
            index = (index + 1) & mask;
        }
        index
    }

    fn grow(&mut self) {
        let old = std::mem::replace(
            &mut self.slots,
            std::iter::repeat_with(|| None).take(self.cap * 2).collect(),
        );
        self.cap *= 2;
        for (key, value) in old.into_iter().flatten() {
            let index = self.keyindex(&key);
            self.slots[index] = Some((key, value));
        }
    }

    // [spec:samurai:def:htab.htabput-fn]
    // [spec:samurai:sem:htab.htabput-fn]
    pub fn htabput(&mut self, key: HashTableKey) -> &mut Option<V> {
        if self.cap / 2 < self.len {
            self.grow();
        }
        let index = self.keyindex(&key);
        if self.slots[index].is_none() {
            self.slots[index] = Some((key, None));
            self.len += 1;
        }
        &mut self.slots[index].as_mut().expect("inserted slot").1
    }

    // [spec:samurai:def:htab.htabget-fn]
    // [spec:samurai:sem:htab.htabget-fn]
    pub fn htabget(&self, key: &HashTableKey) -> Option<&V> {
        let index = self.keyindex(key);
        self.slots[index]
            .as_ref()
            .and_then(|(_, value)| value.as_ref())
    }

    // [spec:samurai:def:htab.delhtab-fn]
    // [spec:samurai:sem:htab.delhtab-fn]
    pub fn delhtab(self, mut delete: impl FnMut(V)) {
        for (_, value) in self.slots.into_iter().flatten() {
            if let Some(value) = value {
                delete(value);
            }
        }
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
    fn keeps_values_through_growth() {
        let mut table = mkhtab(2);
        for value in 0..4u8 {
            let key = htabkey(&[value]);
            *table.htabput(key) = Some(value);
        }
        assert_eq!(table.htabget(&htabkey(&[3])), Some(&3));
    }
}
