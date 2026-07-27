# htab.c, htab.h

> [spec:samurai:def:htab.delhtab-fn]
> void delhtab(struct hashtable *h, void del(void *))

> [spec:samurai:sem:htab.delhtab-fn]
> If h is null, do nothing. Otherwise, if del is non-null, visit the slot
> arrays in increasing index order from 0 through h.cap - 1. For every slot
> whose key.str is non-null, call del once with that slot's value, including
> when the value itself is null. After all callbacks have returned, release
> the key array, the value array, and finally the table object.
>
> The table owns only those three allocations. It never frees the bytes
> referenced by a key, and it does not itself destroy stored values; omitting
> del leaves value lifetime to the caller. A callback runs while the table and
> both arrays are still valid, but must not rely on them after this function
> returns.

> [spec:samurai:def:htab.getle32-fn]
> static inline uint_least32_t getle32(const void *p)

> [spec:samurai:sem:htab.getle32-fn]
> Read exactly four bytes beginning at p, without requiring any particular
> alignment or host byte order. Interpret the first byte as bits 0 through 7,
> the second as bits 8 through 15, the third as bits 16 through 23, and the
> fourth as bits 24 through 31, then return their bitwise union as an unsigned
> 32-bit value. The input must make all four bytes readable; the function
> neither writes memory nor reports an error.

> [spec:samurai:def:htab.getle64-fn]
> static inline uint_least64_t getle64(const void *p)

> [spec:samurai:sem:htab.getle64-fn]
> Read exactly eight bytes beginning at p, without requiring any particular
> alignment or host byte order. Interpret byte j as the unsigned value shifted
> left by 8*j bits, for j from 0 through 7, and return the bitwise union as an
> unsigned 64-bit value. The input must make all eight bytes readable; the
> function does not mutate memory or signal failure.

> [spec:samurai:def:htab.hashtable]
> struct hashtable {
>   size_t len, cap;
>   struct hashtablekey *keys;
>   void **vals;
> }

> [spec:samurai:def:htab.hashtablekey]
> struct hashtablekey {
>   uint64_t hash;
>   const char *str;
>   size_t len;
> }

> [spec:samurai:def:htab.htabget-fn]
> void * htabget(struct hashtable *h, struct hashtablekey *k)

> [spec:samurai:sem:htab.htabget-fn]
> Locate the probe position for k with keyindex. If that position is vacant
> (its stored key.str is null), return null. Otherwise the position contains
> the equal key, so return its stored value unchanged. The lookup creates no
> entry and does not mutate the table.
>
> Consequently, a null result is ambiguous: it denotes either no key or a
> present key whose value has not been assigned (or was assigned null). The
> table must have a nonzero power-of-two capacity, and a missing-key lookup
> needs at least one vacant slot; probing a completely full table for an absent
> key does not terminate in the source algorithm.

> [spec:samurai:def:htab.htabkey-fn]
> void htabkey(struct hashtablekey *k, const char *s, size_t n)

> [spec:samurai:sem:htab.htabkey-fn]
> Set k.str to s and k.len to n without copying, terminating, or normalizing
> the byte sequence. Compute k.hash as rapidhashv1 over exactly the n bytes
> beginning at s, then store that result in k.hash.
>
> This is a shallow, borrowed key descriptor. If it is used for a lookup or
> inserted into a table, its backing bytes must remain readable and unchanged
> for the duration of every later comparison; inserting it does not transfer
> ownership of either the descriptor or the byte storage.

> [spec:samurai:def:htab.htabput-fn]
> void ** htabput(struct hashtable *h, struct hashtablekey *k)

> [spec:samurai:sem:htab.htabput-fn]
> Before looking up k, grow the table when integer-dividing cap by two yields a
> value strictly less than len. Growing doubles cap, allocates fresh key and
> value arrays, marks every new key slot vacant, and then scans the old slots
> in increasing index order. Each occupied old slot is reinserted according to
> its existing hash into the new array, copying its key descriptor and value
> pointer unchanged. len is not changed during rehashing, no destructor is
> called, and the old arrays are released only after all occupied slots have
> been moved. Allocation failure is fatal through the allocating helpers.
>
> Next, find k's probe position. If it is vacant, shallow-copy k into the key
> slot, initialize its value slot to null, and increment len. If an equal key
> is already present, preserve both its original stored descriptor and its
> current value. Return a mutable reference to the selected value slot in
> either case so the caller can install or replace the value.
>
> The call may resize even for a duplicate key, so every previously returned
> value-slot address becomes invalid after a growth. Keys and values are
> otherwise borrowed: the table copies neither key bytes nor pointed-to values,
> and the caller is responsible for their lifetime. The probe rules require a
> usable nonzero power-of-two capacity and a terminating vacant slot.

> [spec:samurai:def:htab.keyequal-fn]
> static bool keyequal(struct hashtablekey *k1, struct hashtablekey *k2)

> [spec:samurai:sem:htab.keyequal-fn]
> First compare the two 64-bit hashes and then the two lengths. If either
> differs, return false without examining key bytes. Only when both match,
> compare exactly len bytes at the two stored string pointers and return true
> exactly when every byte matches. NUL termination and pointer identity play no
> part in equality, and the function does not modify either descriptor.

> [spec:samurai:def:htab.keyindex-fn]
> static size_t keyindex(struct hashtable *h, struct hashtablekey *k)

> [spec:samurai:sem:htab.keyindex-fn]
> Let mask be h.cap - 1 and start at k.hash bitwise-anded with mask. Treat a
> slot as occupied exactly when its key.str is non-null. While the current slot
> is occupied and its key is not equal to k, advance to (index + 1)
> bitwise-anded with mask. Return the first vacant slot or the first equal-key
> slot.
>
> This is linear probing with wraparound and no tombstones or deletion state.
> It relies on cap being a nonzero power of two. If every slot is occupied and
> no equal key exists, the loop wraps forever rather than returning an error.

> [spec:samurai:def:htab.mix-fn]
> static inline uint64_t mix(uint64_t a, uint64_t b)

> [spec:samurai:sem:htab.mix-fn]
> Copy a and b into local words, pass those locals to mum, and return the
> bitwise exclusive-or of the resulting low and high halves. Equivalently,
> form the full unsigned 128-bit product of the two inputs and return its low
> 64 bits XORed with its high 64 bits. The caller's input values are not
> mutated.

> [spec:samurai:def:htab.mkhtab-fn]
> struct hashtable * mkhtab(size_t cap)

> [spec:samurai:sem:htab.mkhtab-fn]
> Require cap to be a usable nonzero power of two: the source asserts the
> bit-test cap & (cap - 1) is zero before allocating, although that raw test
> also admits zero and zero gives no valid probe space. Allocate the table
> object plus separate arrays of cap key descriptors and cap value pointers.
> Set len to zero, retain cap, and mark every key slot vacant by setting only
> its str field to null. The remaining fields of vacant keys and all value
> slots have no defined contents until insertion.
>
> Allocation uses fatal allocating helpers, so allocation or size-overflow
> failure terminates rather than returning null. The returned table owns its
> arrays and must later be passed to delhtab; it owns neither future key bytes
> nor stored values.

> [spec:samurai:def:htab.mum-fn]
> static inline void mum(uint64_t *a, uint64_t *b)

> [spec:samurai:sem:htab.mum-fn]
> Read the two input words before writing either output, form their complete
> unsigned 64-by-64-bit product, store the low 64 bits through a, and store the
> high 64 bits through b. The result is independent of whether a native
> 128-bit integer or four 32-bit partial products implement it. If a and b
> alias, the second store overwrites the first, leaving the high half in the
> shared location.

> [spec:samurai:def:htab.rapidhashv1-fn]
> uint64_t rapidhashv1(const void *ptr, size_t len)

> [spec:samurai:sem:htab.rapidhashv1-fn]
> Hash the exact len input bytes with unsigned 64-bit operations. Let the
> three constants S0, S1, and S2 be, respectively,
> 0x2d358dccaa6c78a5, 0x8bb84b93962eacc9, and
> 0x4b33a62ed433d4a3. Let LE32 and LE64 mean the explicit little-endian loads
> defined above, and let MIX and MUM have the meanings of mix and mum. All
> exclusive-ors and shifts below operate on 64-bit words; additions are not
> used by this hash.
>
> Start p at the first input byte and end at p + len. Set seed0 to
> 0xbdd89aa982704029, then replace it with seed0 XOR
> MIX(seed0 XOR S0, S1) XOR len. Derive seed1 and seed2 as follows:
>
> 1. For len equal to zero, set seed1 and seed2 to zero.
> 2. For len from one through three, set seed1 to
>    byte[p] shifted left 56, OR byte[p + (len > 1 ? 1 : 0)] shifted left 32,
>    OR the final input byte; set seed2 to zero.
> 3. For len from four through sixteen, first set seed1 to
>    LE32(p) shifted left 32 OR LE32(end - 4). If len is at least eight,
>    advance p by four and retreat end by four. Then set seed2 by that same
>    LE32(p) shifted left 32 OR LE32(end - 4) expression.
> 4. For len greater than sixteen, initialize seed1 and seed2 from seed0. If
>    the original len is greater than 48, process at least one 48-byte group:
>    for i = 0, 1, 2 in order, replace seed[i] with
>    MIX(LE64(p) XOR S[i], LE64(p + 8) XOR seed[i]) and advance p by 16.
>    Repeat whole 48-byte groups while end - p is at least 48, then replace
>    seed0 with seed0 XOR seed1 XOR seed2. If more than sixteen bytes remain,
>    first XOR seed0 with S1, then repeatedly replace seed0 with
>    MIX(LE64(p) XOR S2, LE64(p + 8) XOR seed0), advancing p by 16 after each
>    group, until at most sixteen bytes remain. Finally set seed1 to
>    LE64(end - 16) and seed2 to LE64(end - 8).
>
> Finish every branch by XORing seed1 with S1 and seed2 with seed0, applying
> MUM to seed1 and seed2 in place, and returning
> MIX(seed1 XOR S0 XOR len, seed2 XOR S1). The routine allocates nothing and
> does not mutate the input; callers must supply len readable bytes.
