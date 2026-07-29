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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ninja_explanations_records_multiple_reasons() {
        let mut explanations = Explanations::default();
        explanations.record(1, "first explanation");
        explanations.record(1, "second explanation");
        explanations.record(2, "third explanation");
        explanations.record(2, "fourth explanation");
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
}
