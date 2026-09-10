//! What a composition asked the ground, merged across the passes that asked.
//!
//! A read's own journal is keyed by POSITION, because within one read that is
//! the only thing that identifies a question: the same command written twice
//! is two questions, and a `$(shell)` inside a `$(foreach)` is one per
//! iteration. See [`kati::session::GroundJournal`].
//!
//! This is keyed by the question and the text asked instead, because it is
//! used for something else. It is never replayed into a read. It is re-asked
//! at the start of a later invocation to decide whether that invocation would
//! read what this one read, and for that only the distinct questions matter.
//!
//! KEPT PER UNIT, and that is not a filing convenience. A unit is read with
//! the process in its own directory, so `$(wildcard *.c)` is the same text
//! asked in two places and answered about two directories. One map over the
//! whole composition would file both under one key, re-ask it once somewhere,
//! and call both units checked.
//!
//! A key answered two different ways by the passes that read one unit is a key
//! this record cannot speak for: that unit was composed against two grounds,
//! and there is no single one a later run could be compared against. The whole
//! record is refused rather than guessed at, and the next invocation reads the
//! makefiles again.
//!
//! An answer of "nothing" is an answer. A makefile that was not there when a
//! pass looked is exactly what makes a later run read something different once
//! it has arrived, so leaving it out would have the record call a moved tree
//! unchanged.

use crate::htab::RapidHashMap;
use kati::session::{GroundAnswer, GroundQuestion};
use std::path::{Path, PathBuf};

/// One question the composition asked, and the one answer it was given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct RecordedAnswer {
    /// The bytes the call put into the expansion that asked for it.
    pub(crate) answer: Vec<u8>,
    /// `.SHELLSTATUS`, for the one question that leaves one.
    pub(crate) status: Option<i32>,
}

/// What was asked, and of what.
pub(crate) type RecordedQuestion = (GroundQuestion, Vec<u8>);

/// What a unit's passes disagreed about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Divergent {
    Question(GroundQuestion),
    /// An environment variable the read looked up.
    Environment,
}

/// One thing a unit's passes answered two ways.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Divergence {
    pub(crate) about: Divergent,
    pub(crate) asked: Vec<u8>,
    pub(crate) first: Vec<u8>,
    pub(crate) then: Vec<u8>,
}

/// What one unit's read depended on outside its own text.
#[derive(Debug)]
pub(crate) struct UnitRecord {
    /// Where the process stood while this unit was read, and so where its
    /// questions have to be asked again to be asked at all.
    directory: PathBuf,
    answers: RapidHashMap<RecordedQuestion, RecordedAnswer>,
    /// Every environment variable the read depended on, with what it read.
    ///
    /// Not merged with any other unit's, because a composed child is read
    /// under an environment of its own: it is the same `MAKELEVEL` name and a
    /// different value, and that is the makefile working rather than the
    /// ground moving.
    environment: RapidHashMap<Vec<u8>, Option<Vec<u8>>>,
}

impl UnitRecord {
    fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            answers: RapidHashMap::default(),
            environment: RapidHashMap::default(),
        }
    }

    /// Where this unit's questions were asked.
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    /// How many distinct questions this unit asked.
    pub(crate) fn len(&self) -> usize {
        self.answers.len()
    }

    /// Every question this unit asked, with the answer it got.
    pub(crate) fn answers(&self) -> impl Iterator<Item = (&RecordedQuestion, &RecordedAnswer)> {
        self.answers.iter()
    }

    /// Every environment variable this unit read, with the value it read.
    pub(crate) fn environment(&self) -> impl Iterator<Item = (&Vec<u8>, &Option<Vec<u8>>)> {
        self.environment.iter()
    }

    fn record(
        &mut self,
        question: GroundQuestion,
        asked: &[u8],
        answer: &[u8],
        status: Option<i32>,
    ) -> Option<Divergence> {
        let key = (question, asked.to_vec());
        let value = RecordedAnswer {
            answer: answer.to_vec(),
            status,
        };
        match self.answers.get(&key) {
            Some(existing) if *existing == value => None,
            Some(existing) => Some(Divergence {
                about: Divergent::Question(question),
                asked: key.1,
                first: existing.answer.clone(),
                then: value.answer,
            }),
            None => {
                self.answers.insert(key, value);
                None
            }
        }
    }
}

/// Every question a whole composition asked, filed under the unit that asked.
#[derive(Default, Debug)]
pub(crate) struct CompositionRecord {
    units: RapidHashMap<Vec<u8>, UnitRecord>,
    /// The first key a unit answered two ways. Held rather than counted: one
    /// is enough to refuse the record, and it is what a diagnostic names.
    diverged: Option<Divergence>,
}

impl CompositionRecord {
    /// Merge one read of one unit into the record.
    ///
    /// Called once per read rather than once per unit, because a unit whose
    /// read cannot be carried is read again on every pass, and each of those
    /// reads is a pass that asked. A read that IS carried is asked nothing on
    /// the passes that repeat it, so it contributes once.
    ///
    /// The nesting count each answer carries — how many questions were asked
    /// while it was being answered — is what a replay walks the sequence by,
    /// and means nothing here. Every answer in the list was given, so every
    /// one is something the composition depended on.
    pub(crate) fn absorb(
        &mut self,
        unit: &[u8],
        directory: &Path,
        journalled: &[GroundAnswer],
        off_journal: &[GroundAnswer],
        environment: &[(kati::bytes::Bytes, Option<kati::bytes::Bytes>)],
    ) {
        let record = self
            .units
            .entry(unit.to_vec())
            .or_insert_with(|| UnitRecord::new(directory));
        for given in journalled.iter().chain(off_journal) {
            let diverged = record.record(given.question, &given.asked, &given.answer, given.status);
            if let Some(diverged) = diverged {
                self.diverged.get_or_insert(diverged);
            }
        }
        for (name, value) in environment {
            let value = value.as_ref().map(|value| value.to_vec());
            match record.environment.get(name.as_ref()) {
                Some(existing) if *existing == value => {}
                Some(existing) => {
                    let diverged = Divergence {
                        about: Divergent::Environment,
                        asked: name.to_vec(),
                        first: existing.clone().unwrap_or_default(),
                        then: value.unwrap_or_default(),
                    };
                    self.diverged.get_or_insert(diverged);
                }
                None => {
                    record.environment.insert(name.to_vec(), value);
                }
            }
        }
    }

    /// The key a unit answered two ways, if any did.
    ///
    /// A record with one of these describes no single ground, so it is not
    /// written.
    pub(crate) const fn divergence(&self) -> Option<&Divergence> {
        self.diverged.as_ref()
    }

    /// How many units the composition read.
    pub(crate) fn units(&self) -> usize {
        self.units.len()
    }

    /// Every unit's record, keyed by the unit's compilation key.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (&Vec<u8>, &UnitRecord)> {
        self.units.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(directory: &str) -> PathBuf {
        PathBuf::from(directory)
    }

    /// How many distinct questions the record holds, over every unit.
    fn questions(record: &CompositionRecord) -> usize {
        record.units.values().map(UnitRecord::len).sum()
    }

    fn absorb(
        record: &mut CompositionRecord,
        unit: &str,
        directory: &str,
        question: GroundQuestion,
        asked: &str,
        answer: &str,
    ) {
        let given = vec![kati::session::GroundAnswer::asked_and_told(
            question,
            kati::bytes::Bytes::from(asked.as_bytes().to_vec()),
            kati::bytes::Bytes::from(answer.as_bytes().to_vec()),
            None,
        )];
        record.absorb(unit.as_bytes(), &at(directory), &given, &[], &[]);
    }

    #[test]
    fn one_question_asked_twice_alike_records_once() {
        let mut record = CompositionRecord::default();
        absorb(
            &mut record,
            "a",
            ".",
            GroundQuestion::Wildcard,
            "*.c",
            "a.c",
        );
        absorb(
            &mut record,
            "a",
            ".",
            GroundQuestion::Wildcard,
            "*.c",
            "a.c",
        );
        assert_eq!(questions(&record), 1);
        assert!(record.divergence().is_none());
    }

    #[test]
    fn two_units_asking_alike_are_filed_apart() {
        let mut record = CompositionRecord::default();
        absorb(
            &mut record,
            "a",
            "src",
            GroundQuestion::Wildcard,
            "*.c",
            "a.c",
        );
        absorb(
            &mut record,
            "b",
            "lib",
            GroundQuestion::Wildcard,
            "*.c",
            "b.c",
        );
        assert_eq!(record.units(), 2);
        assert_eq!(questions(&record), 2);
        assert!(
            record.divergence().is_none(),
            "the same text answered about two directories is two questions"
        );
    }

    #[test]
    fn the_same_text_asked_two_ways_records_twice() {
        let mut record = CompositionRecord::default();
        absorb(
            &mut record,
            "a",
            ".",
            GroundQuestion::Wildcard,
            "g.h",
            "g.h",
        );
        absorb(&mut record, "a", ".", GroundQuestion::Include, "g.h", "g.h");
        assert_eq!(questions(&record), 2);
        assert!(record.divergence().is_none());
    }

    #[test]
    fn one_unit_answered_two_ways_diverges() {
        let mut record = CompositionRecord::default();
        absorb(&mut record, "a", ".", GroundQuestion::Include, "g.h", "");
        absorb(&mut record, "a", ".", GroundQuestion::Include, "g.h", "g.h");
        let diverged = record.divergence().expect("two answers is a divergence");
        assert_eq!(diverged.about, Divergent::Question(GroundQuestion::Include));
        assert_eq!(diverged.asked, b"g.h");
        assert!(diverged.first.is_empty());
        assert_eq!(diverged.then, b"g.h");
    }

    #[test]
    fn an_answer_of_nothing_is_still_recorded() {
        let mut record = CompositionRecord::default();
        absorb(
            &mut record,
            "a",
            ".",
            GroundQuestion::Wildcard,
            "gone.c",
            "",
        );
        assert_eq!(questions(&record), 1);
        let (_, unit) = record.entries().next().expect("the unit");
        let (key, value) = unit.answers().next().expect("the absence is recorded");
        assert_eq!(key.0, GroundQuestion::Wildcard);
        assert!(value.answer.is_empty());
    }

    #[test]
    fn a_suspended_question_joins_the_record() {
        let mut record = CompositionRecord::default();
        let suspended = vec![kati::session::GroundAnswer::asked_and_told(
            GroundQuestion::Shell,
            kati::bytes::Bytes::from_static(b"cat stamp"),
            kati::bytes::Bytes::from_static(b"ready"),
            None,
        )];
        record.absorb(b"a", &at("."), &[], &suspended, &[]);
        assert_eq!(
            questions(&record),
            1,
            "a recipe the compiler expanded for itself still asked the ground"
        );
    }

    #[test]
    fn one_environment_name_read_two_ways_diverges() {
        let mut record = CompositionRecord::default();
        let set = [(kati::bytes::Bytes::from_static(b"V"), Some("1".into()))];
        let unset = [(kati::bytes::Bytes::from_static(b"V"), None)];
        record.absorb(b"a", &at("."), &[], &[], &set);
        record.absorb(b"a", &at("."), &[], &[], &unset);
        assert!(record.divergence().is_some());
    }

    #[test]
    fn two_units_read_one_name_apart() {
        let mut record = CompositionRecord::default();
        let parent = [(
            kati::bytes::Bytes::from_static(b"MAKELEVEL"),
            Some("0".into()),
        )];
        let child = [(
            kati::bytes::Bytes::from_static(b"MAKELEVEL"),
            Some("1".into()),
        )];
        record.absorb(b"a", &at("."), &[], &[], &parent);
        record.absorb(b"b", &at("sub"), &[], &[], &child);
        assert!(
            record.divergence().is_none(),
            "a composed child reads its own environment, not the parent's"
        );
    }

    #[test]
    fn the_first_divergence_is_the_one_kept() {
        let mut record = CompositionRecord::default();
        absorb(&mut record, "a", ".", GroundQuestion::Shell, "date", "mon");
        absorb(&mut record, "a", ".", GroundQuestion::Shell, "date", "tue");
        absorb(&mut record, "a", ".", GroundQuestion::Shell, "date", "wed");
        let diverged = record.divergence().expect("a divergence");
        assert_eq!(diverged.then, b"tue");
    }
}
