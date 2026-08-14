//! What a command's outputs came to, once the command has run.
//!
//! A build that has just run a command has to say two things about it: whether
//! the targets reading its outputs must be looked at again, and whether the
//! edge itself is settled for the rest of the run. Ninja answers both from
//! `restat`, GNU Make answers both from the disk, and the deferred `$?`
//! machinery answers them from the outputs it was told to watch. The three
//! answers are decided here rather than inside the several hundred lines that
//! finish an edge.

use super::{Builder, EdgeId};

/// What a finished command's outputs came to.
pub(super) struct Settled {
    /// Whether what reads these outputs has to be scanned again. The build
    /// planned them dirty because the command was going to run; if the run did
    /// not leave the outputs where that assumption expected, the scan is what
    /// takes it back.
    pub(super) pruned: bool,
    /// Whether the edge is settled: it has run, and nothing it produced will
    /// make it dirty again on a later scan.
    pub(super) all_pruned: bool,
}

/// The timestamps a finished command left, beside the ones it started from.
pub(super) struct Outcome<'a> {
    pub(super) deferred: bool,
    pub(super) restat: bool,
    /// Every output's mtime as stat'd after the command, zero for one that is
    /// not there.
    pub(super) new_mtimes: &'a [i64],
    /// Which of them the command left where it found them.
    pub(super) unchanged: &'a [bool],
    /// The wall clock as the command was launched.
    pub(super) started: i64,
}

impl Builder<'_> {
    pub(super) fn settled(&self, edge: EdgeId, outcome: &Outcome<'_>) -> Settled {
        if self.options.dryrun {
            return Settled {
                pruned: false,
                all_pruned: false,
            };
        }
        if outcome.deferred {
            return Settled {
                pruned: true,
                all_pruned: !self
                    .graph
                    .deferred_freshness(edge)
                    .is_some_and(|freshness| freshness.always_dirty_output)
                    && outcome.made_every_output()
                    && outcome.unchanged.iter().all(|same| *same),
            };
        }
        if self.reobserves(edge, outcome) {
            return Settled {
                pruned: true,
                all_pruned: true,
            };
        }
        Settled {
            pruned: outcome.restat && outcome.unchanged.iter().any(|same| *same),
            all_pruned: outcome.restat && outcome.unchanged.iter().all(|same| *same),
        }
    }

    /// Whether what `edge`'s outputs came to is read off the disk rather than
    /// taken from the command having run.
    ///
    /// Two targets are never asked, because neither is a file an answer can be
    /// read off: a phony one, and one the recipe left absent, which GNU Make
    /// reads as infinitely new. Both are remade by definition, and everything
    /// behind them is out of date.
    ///
    /// The third exclusion is for wall time rather than for meaning. An output
    /// no older than the moment its command started is one the command really
    /// wrote, and nothing that reads it can have been made since — a consumer
    /// waits for every input, so it was made before this command or not at all,
    /// and either way it is older. Re-observing such an output can only reach
    /// the answer the build already holds, so the scan behind it is skipped
    /// instead of being paid for on every edge of every build. A coarse
    /// filesystem clock resolves the wrong way into doing the work, never into
    /// skipping it.
    // [spec:ronin:req:make.remade-target-re-observed]
    fn reobserves(&self, edge: EdgeId, outcome: &Outcome<'_>) -> bool {
        let edge = self.graph.edge(edge);
        edge.outputs_reobserved
            && !edge.always_dirty
            && outcome.made_every_output()
            && !outcome.wrote_every_output()
    }
}

impl Outcome<'_> {
    /// Whether every output is there to be read at all.
    fn made_every_output(&self) -> bool {
        self.new_mtimes.iter().all(|mtime| *mtime != 0)
    }

    /// Whether every output is at least as new as the command that made it.
    fn wrote_every_output(&self) -> bool {
        self.new_mtimes.iter().all(|mtime| *mtime >= self.started)
    }
}
