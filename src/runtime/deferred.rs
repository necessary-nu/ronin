use super::FileTime;
use crate::graph::NodeId;

#[derive(Debug)]
pub(crate) struct DeferredRuntime {
    snapshot: DeferredSnapshot,
    decision: DeferredDecision,
    phase: DeferredPhase,
    new_inputs: Vec<NodeId>,
}

#[derive(Debug)]
struct DeferredSnapshot {
    baseline: FileTime,
    all_inputs_new: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DeferredDecision {
    #[default]
    Undecided,
    Clean,
    Run,
    Candidate,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DeferredPhase {
    #[default]
    Pending,
    ActivationsAttached,
    Settled,
}

impl Default for DeferredRuntime {
    fn default() -> Self {
        Self {
            snapshot: DeferredSnapshot {
                baseline: FileTime::UNOBSERVED,
                all_inputs_new: false,
            },
            decision: DeferredDecision::Undecided,
            phase: DeferredPhase::Pending,
            new_inputs: Vec::new(),
        }
    }
}

impl DeferredRuntime {
    pub(crate) const fn baseline(&self) -> FileTime {
        self.snapshot.baseline
    }

    pub(crate) const fn all_inputs_new(&self) -> bool {
        self.snapshot.all_inputs_new
    }

    pub(crate) const fn initial_run(&self) -> bool {
        matches!(self.decision, DeferredDecision::Run)
    }

    pub(crate) const fn initial_decided(&self) -> bool {
        !matches!(self.decision, DeferredDecision::Undecided)
    }

    pub(crate) const fn candidate_only(&self) -> bool {
        matches!(self.decision, DeferredDecision::Candidate)
    }

    pub(crate) const fn activation_attached(&self) -> bool {
        matches!(self.phase, DeferredPhase::ActivationsAttached)
    }

    pub(crate) const fn settled(&self) -> bool {
        matches!(self.phase, DeferredPhase::Settled)
    }

    pub(crate) const fn settle(&mut self) {
        self.phase = DeferredPhase::Settled;
    }

    pub(crate) const fn attach_activations(&mut self) {
        self.phase = DeferredPhase::ActivationsAttached;
    }

    pub(crate) fn capture(&mut self, baseline: FileTime, all_inputs_new: bool) {
        self.snapshot = DeferredSnapshot {
            baseline,
            all_inputs_new,
        };
        self.decision = DeferredDecision::Undecided;
        self.phase = DeferredPhase::Pending;
        self.new_inputs.clear();
    }

    pub(crate) const fn decide_initial(&mut self, initial_run: bool, candidate_only: bool) {
        self.decision = if initial_run {
            DeferredDecision::Run
        } else if candidate_only {
            DeferredDecision::Candidate
        } else {
            DeferredDecision::Clean
        };
    }

    pub(crate) fn new_inputs(&self) -> &[NodeId] {
        &self.new_inputs
    }

    pub(crate) fn set_new_inputs(&mut self, inputs: Vec<NodeId>) {
        self.new_inputs = inputs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{mknode, Graph};

    #[test]
    fn lifecycle_preserves_snapshot_and_inputs() {
        let mut graph = Graph::default();
        let input = mknode(&mut graph, b"input");
        let mut state = DeferredRuntime::default();

        state.capture(FileTime::observed(7), false);
        assert_eq!(state.baseline(), FileTime::observed(7));
        assert!(!state.all_inputs_new());
        assert!(!state.initial_decided());

        state.decide_initial(false, true);
        state.set_new_inputs(vec![input]);
        assert!(state.candidate_only());
        assert_eq!(state.new_inputs(), &[input]);

        state.attach_activations();
        assert!(state.activation_attached());
        state.settle();
        assert!(state.settled());

        state.capture(FileTime::MISSING, true);
        assert!(state.all_inputs_new());
        assert!(state.new_inputs().is_empty());
        assert!(!state.initial_decided());
    }
}
