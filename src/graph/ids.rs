//! Encoding graph identifiers for channels that cannot carry their type.

use super::EdgeId;
impl EdgeId {
    /// Encode for an integer channel that cannot carry the type, such as a
    /// poll key. Zero stays free for the signal sentinel.
    pub(crate) const fn event_key(self) -> usize {
        self.0.get() as usize
    }

    /// Restore an identifier previously encoded by [`Self::event_key`].
    ///
    /// Unlike `from_index` this claims nothing about the arena — it only
    /// round-trips an identifier the caller already held. That is also why it
    /// is not a basis for unchecked indexing: a key from elsewhere would
    /// produce an identifier naming no slot.
    pub(crate) const fn from_event_key(key: usize) -> Option<Self> {
        if key > u32::MAX as usize {
            return None;
        }
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the bound above keeps the key within u32"
        )]
        match std::num::NonZeroU32::new(key as u32) {
            Some(raw) => Some(Self(raw)),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EdgeId;

    /// The poll-key encoding must round-trip, and must keep zero free.
    ///
    /// The supervisor multiplexes edge completions and signal wakeups over one
    /// integer key space, and distinguishes them by zero meaning "signal". An
    /// encoding that mapped some edge onto zero would make a finished command
    /// look like a signal; one that failed to round-trip would attribute
    /// output to the wrong edge.
    #[test]
    fn event_keys_round_trip_and_reserve_zero() {
        for index in [0, 1, 2, 4095, 1_000_000] {
            let edge = EdgeId::from_index(index);
            let key = edge.event_key();
            assert_ne!(key, 0, "index {index} must not encode to the signal key");
            assert_eq!(EdgeId::from_event_key(key), Some(edge));
        }
        assert_eq!(EdgeId::from_event_key(0), None);
        assert_eq!(EdgeId::from_event_key(u32::MAX as usize + 1), None);
    }
}
