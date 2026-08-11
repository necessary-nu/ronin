//! Interned binding names and the small maps keyed by them.
//!
//! Every variable lookup used to walk a `BTreeMap<String, _>`, comparing bytes
//! through pointer-chasing nodes, and evaluation performs several lookups per
//! edge plus one per scope in the parent chain. Interning turns each of those
//! into an integer comparison over a contiguous table.

use crate::htab::RapidHashMap;
use crate::util::{BStr, BString, arena_id};

arena_id!(VarId);

/// Names that evaluation and the build rules refer to by fixed identity.
///
/// `Names::default` interns these first and in this order, so the constants
/// below are valid for every graph and the hot comparisons are constants.
const RESERVED: [&[u8]; 14] = [
    b"in",
    b"in_newline",
    b"out",
    b"command",
    b"depfile",
    b"deps",
    b"description",
    b"dyndep",
    b"generator",
    b"msvc_deps_prefix",
    b"pool",
    b"restat",
    b"rspfile",
    b"rspfile_content",
];

pub(crate) struct Names {
    ids: RapidHashMap<BString, VarId>,
    names: Vec<BString>,
}

impl Names {
    pub(crate) const IN: VarId = VarId::from_index(0);
    pub(crate) const IN_NEWLINE: VarId = VarId::from_index(1);
    pub(crate) const OUT: VarId = VarId::from_index(2);
    pub(crate) const COMMAND: VarId = VarId::from_index(3);
    pub(crate) const DEPFILE: VarId = VarId::from_index(4);
    pub(crate) const DEPS: VarId = VarId::from_index(5);
    pub(crate) const DESCRIPTION: VarId = VarId::from_index(6);
    pub(crate) const DYNDEP: VarId = VarId::from_index(7);
    pub(crate) const GENERATOR: VarId = VarId::from_index(8);
    pub(crate) const MSVC_DEPS_PREFIX: VarId = VarId::from_index(9);
    pub(crate) const POOL: VarId = VarId::from_index(10);
    pub(crate) const RESTAT: VarId = VarId::from_index(11);
    pub(crate) const RSPFILE: VarId = VarId::from_index(12);
    pub(crate) const RSPFILE_CONTENT: VarId = VarId::from_index(13);

    pub(crate) fn intern(&mut self, name: &BStr) -> VarId {
        if let Some(id) = self.ids.get(name) {
            return *id;
        }
        let id = VarId::from_index(self.names.len());
        let name = name.to_owned();
        self.names.push(name.clone());
        self.ids.insert(name, id);
        id
    }

    /// Resolve an already-interned name.
    ///
    /// A name that was never interned cannot be bound anywhere, so `None` is
    /// the correct answer for a lookup rather than a reason to intern.
    pub(crate) fn lookup(&self, name: &BStr) -> Option<VarId> {
        self.ids.get(name).copied()
    }

    pub(crate) fn name(&self, id: VarId) -> &BStr {
        self.names[id.index()].as_ref()
    }
}

impl Default for Names {
    fn default() -> Self {
        let mut names = Self {
            ids: RapidHashMap::default(),
            names: Vec::new(),
        };
        for reserved in RESERVED {
            names.intern(BStr::new(reserved));
        }
        names
    }
}

/// A small name-keyed table held in sorted order.
///
/// Binding tables hold a handful of entries for an edge or rule and at most a
/// few hundred for a manifest's root scope, so one contiguous run beats a tree
/// with a node allocation per entry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Bindings<V> {
    entries: Vec<(VarId, V)>,
}

impl<V> Bindings<V> {
    pub(crate) fn get(&self, name: VarId) -> Option<&V> {
        self.entries
            .binary_search_by_key(&name, |(key, _)| *key)
            .ok()
            .map(|index| &self.entries[index].1)
    }

    pub(crate) fn insert(&mut self, name: VarId, value: V) {
        match self.entries.binary_search_by_key(&name, |(key, _)| *key) {
            Ok(index) => self.entries[index].1 = value,
            Err(index) => self.entries.insert(index, (name, value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_names_keep_their_documented_identities() {
        let names = Names::default();
        assert_eq!(names.lookup(BStr::new("in")), Some(Names::IN));
        assert_eq!(names.lookup(BStr::new("command")), Some(Names::COMMAND));
        assert_eq!(
            names.lookup(BStr::new("rspfile_content")),
            Some(Names::RSPFILE_CONTENT)
        );
        assert_eq!(names.name(Names::OUT), "out");
        assert_eq!(names.lookup(BStr::new("never_bound")), None);
    }

    #[test]
    fn interning_is_stable_and_bindings_round_trip() {
        let mut names = Names::default();
        let first = names.intern(BStr::new("cflags"));
        assert_eq!(names.intern(BStr::new("cflags")), first);
        assert_eq!(names.lookup(BStr::new("cflags")), Some(first));
        assert_eq!(names.name(first), "cflags");

        let mut bindings = Bindings::default();
        bindings.insert(first, 1);
        bindings.insert(Names::COMMAND, 2);
        bindings.insert(first, 3);
        assert_eq!(bindings.get(first), Some(&3));
        assert_eq!(bindings.get(Names::COMMAND), Some(&2));
        assert_eq!(bindings.get(Names::DEPS), None);
    }
}
