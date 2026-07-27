//! Optional build explanations compatible with Ninja's explanations source.

use std::collections::BTreeMap;

#[derive(Default)]
pub struct Explanations {
    values: BTreeMap<usize, Vec<String>>,
}

impl Explanations {
    pub fn record(&mut self, item: usize, explanation: impl Into<String>) {
        self.values
            .entry(item)
            .or_default()
            .push(explanation.into());
    }

    pub fn lookup_and_append(&self, item: usize, output: &mut Vec<String>) {
        if let Some(explanations) = self.values.get(&item) {
            output.extend(explanations.iter().cloned());
        }
    }
}

pub struct OptionalExplanations<'a> {
    inner: Option<&'a mut Explanations>,
}

impl<'a> OptionalExplanations<'a> {
    pub fn new(inner: Option<&'a mut Explanations>) -> Self {
        Self { inner }
    }

    pub fn record(&mut self, item: usize, explanation: impl Into<String>) {
        if let Some(inner) = &mut self.inner {
            inner.record(item, explanation);
        }
    }

    pub fn lookup_and_append(&self, item: usize, output: &mut Vec<String>) {
        if let Some(inner) = &self.inner {
            inner.lookup_and_append(item, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record_examples(explanations: &mut impl ExplanationRecorder) {
        explanations.record(1, "first explanation");
        explanations.record(1, "second explanation");
        explanations.record(2, "third explanation");
        explanations.record(2, "fourth explanation");
    }

    trait ExplanationRecorder {
        fn record(&mut self, item: usize, explanation: &str);
    }

    impl ExplanationRecorder for Explanations {
        fn record(&mut self, item: usize, explanation: &str) {
            self.record(item, explanation);
        }
    }

    impl ExplanationRecorder for OptionalExplanations<'_> {
        fn record(&mut self, item: usize, explanation: &str) {
            self.record(item, explanation);
        }
    }

    #[test]
    fn ninja_explanations_records_multiple_reasons() {
        let mut explanations = Explanations::default();
        record_examples(&mut explanations);
        let mut list = Vec::new();
        explanations.lookup_and_append(0, &mut list);
        assert!(list.is_empty());
        explanations.lookup_and_append(1, &mut list);
        explanations.lookup_and_append(2, &mut list);
        assert_eq!(
            list,
            [
                "first explanation",
                "second explanation",
                "third explanation",
                "fourth explanation",
            ]
        );
    }

    #[test]
    fn ninja_optional_explanations_forwards_when_present() {
        let mut parent = Explanations::default();
        let mut explanations = OptionalExplanations::new(Some(&mut parent));
        record_examples(&mut explanations);
        let mut list = Vec::new();
        explanations.lookup_and_append(1, &mut list);
        explanations.lookup_and_append(2, &mut list);
        assert_eq!(list.len(), 4);
    }

    #[test]
    fn ninja_optional_explanations_ignores_when_absent() {
        let mut explanations = OptionalExplanations::new(None);
        record_examples(&mut explanations);
        let mut list = Vec::new();
        explanations.lookup_and_append(1, &mut list);
        explanations.lookup_and_append(2, &mut list);
        assert!(list.is_empty());
    }
}
