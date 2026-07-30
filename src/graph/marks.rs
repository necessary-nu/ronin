//! Generation-stamped traversal marks.
//!
//! Graph scans run once per target, per restat completion, and per dyndep
//! reload. Allocating and zeroing a graph-sized array for each is the dominant
//! cost on large graphs, so these buffers live for the build and reset by
//! bumping a counter.

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(super) enum VisitState {
    #[default]
    New,
    Active,
    Done,
}

/// Per-index marks cleared in constant time by bumping a generation.
///
/// Traversals run once per target, per restat completion, and per dyndep
/// reload, so allocating and zeroing a graph-sized array each time is the
/// dominant cost on large graphs. Stamping each slot with the generation that
/// wrote it makes a reset a single counter increment. Stamps stay one byte
/// wide, so the buffers cost no more than the per-call arrays they replace and
/// a real clear is needed only when the counter wraps.
#[derive(Default)]
pub(crate) struct MarkSet {
    stamps: Vec<u8>,
    generation: u8,
}

impl MarkSet {
    pub(crate) fn begin(&mut self, len: usize) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamps.fill(0);
            self.generation = 1;
        }
        if self.stamps.len() < len {
            self.stamps.resize(len, 0);
        }
    }

    /// Mark `index`, reporting whether it was already marked this generation.
    pub(crate) fn replace(&mut self, index: usize) -> bool {
        let seen = self.stamps[index] == self.generation;
        self.stamps[index] = self.generation;
        seen
    }
}

impl VisitState {
    const fn from_bits(bits: u8) -> Self {
        match bits {
            1 => Self::Active,
            2 => Self::Done,
            _ => Self::New,
        }
    }

    const fn bits(self) -> u8 {
        match self {
            Self::New => 0,
            Self::Active => 1,
            Self::Done => 2,
        }
    }
}

/// Tri-state visit marks with the same constant-time reset as [`MarkSet`].
///
/// Each byte packs the writing generation above the two state bits, so the
/// buffer matches the width of the plain state array it replaces.
#[derive(Default)]
pub(super) struct VisitMarks {
    stamps: Vec<u8>,
    generation: u8,
}

impl VisitMarks {
    const MAX_GENERATION: u8 = u8::MAX >> 2;

    pub(super) fn begin(&mut self, len: usize) {
        self.generation += 1;
        if self.generation > Self::MAX_GENERATION {
            self.stamps.fill(0);
            self.generation = 1;
        }
        if self.stamps.len() < len {
            self.stamps.resize(len, 0);
        }
    }

    pub(super) fn get(&self, index: usize) -> VisitState {
        let stamp = self.stamps[index];
        if stamp >> 2 == self.generation {
            VisitState::from_bits(stamp & 0b11)
        } else {
            VisitState::New
        }
    }

    pub(super) fn set(&mut self, index: usize, state: VisitState) {
        self.stamps[index] = (self.generation << 2) | state.bits();
    }
}
