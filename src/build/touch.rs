//! Bringing a target up to date without making it, which is GNU Make's `-t`.
//!
//! A run under `-t` plans exactly the work an ordinary run would, reaches every
//! edge in the same order, and reports each one with the same progress line.
//! Only what an edge DOES changes: no process starts, and the files it would
//! have written are given a fresh date instead. That is the whole of the
//! switch, and keeping it to that is what leaves the scheduler, the reporter
//! and the persistence policy with no idea the run is a touch at all.
//!
//! Nothing GNU-voiced is said about it. GNU Make prints `touch <file>` per
//! output; `[spec:ronin:req:make.narration+1]` puts Make mode's reporting in
//! the manifest front end's shape, and the edge has already been reported by
//! the ordinary progress line, so a second line naming the same work in another
//! tool's words would be narration and not information.

use super::command::Pretending;
use super::{BuildError, BuildOperation, BuildResult, Builder};
use crate::graph::{EdgeId, NodeId};
use bstr::ByteSlice as _;

impl Builder<'_> {
    /// Whether an edge reaching the supervisor is going through the motions
    /// rather than starting a process.
    ///
    /// `-n` is the familiar one: nothing runs and nothing changes. `-t` joins it
    /// because a touched edge has no process either — what makes its outputs
    /// current is [`Builder::touch_outputs`], and the recipe that would have
    /// made them is exactly what is not being run.
    pub(super) const fn pretending(&self) -> Pretending {
        // `-n` outranks `-t`, and it stands in for every step alike: a dry run
        // over a graph that already holds the recursive child's edges has
        // nothing to learn by starting one, which is what
        // `[dec:ronin:make-compiles-to-ninja]` settles and what
        // `make-recipe-dry-run` recorded. `-t` is the switch that steps aside,
        // because it decides what the run writes to disk.
        if self.options.dryrun {
            return Pretending::EveryStep;
        }
        if self.options.touch {
            return Pretending::AllButRunning;
        }
        Pretending::Nothing
    }

    /// Give this edge's outputs a fresh date instead of having made them, which
    /// is what GNU Make's `-t` puts in place of running a recipe.
    ///
    /// Two switches qualify it and each takes back a different half of what it
    /// does. `-n` outranks the touch: the edge is still reported, and the file
    /// is left alone, because a run told to change nothing changes nothing.
    /// `-s` withdraws the reporting and touches the file regardless — that one
    /// is the ordinary quiet build and needs nothing here.
    ///
    /// Nothing GNU-voiced is said about it. GNU Make prints `touch <file>` per
    /// output; `[spec:ronin:req:make.narration+1]` puts Make mode's reporting in
    /// the manifest front end's shape, and the edge has already been reported by
    /// the ordinary progress line, so a second line naming the same work in
    /// another tool's words would be narration and not information.
    // [spec:ronin:req:make.narration+1]
    pub(super) fn touch_outputs(&self, edge: EdgeId, stood_in_for_a_step: bool) -> BuildResult<()> {
        if !self.options.touch || self.options.dryrun {
            return Ok(());
        }
        // Touched in place of a line that was skipped, which is where GNU Make
        // touches: `start_job_command` (job.c) goes on to the next command for
        // a line it steps aside for, and only the ones it stood in for reach
        // the touch. A recipe every line of which runs anyway is therefore made
        // by running it, and the target is left with the date its own recipe
        // gave it.
        if !stood_in_for_a_step {
            return Ok(());
        }
        // A `.PHONY` target's name is not a file and giving it one would be
        // litter that then reads as up to date for every run after this. GNU
        // Make declines it in `update_file_1` for the same reason, and says
        // nothing about the target at all.
        if self.graph.edge(edge).always_dirty {
            return Ok(());
        }
        let disk = self.disk.clone();
        for output in self.touchable_outputs(edge) {
            // Where the build settled the name, which for a `::` chain an
            // earlier entry found current is the path the search returned:
            // GNU Make renames the file before it reaches the touch, so
            // `touch_file` (remake.c) is handed `file->name` already changed.
            // Touching the written name instead would make a second file
            // beside the one the build is standing in for.
            let path = match self.graph.searched_at(output) {
                Some(found) if crate::graph::found_name_stands(&self.runtime, output) => {
                    found.clone()
                }
                _ => self.graph.node_path(output).to_owned(),
            };
            disk.touch(path.to_path().expect("byte paths are valid on Unix"))
                .map_err(|source| {
                    BuildError::io(BuildOperation::TouchOutput, Some(path), Some(edge), source)
                })?;
        }
        Ok(())
    }

    /// The files this edge is really for, which is not always what it is
    /// declared to write.
    ///
    /// A `::` entry and a deferred-freshness rule both write to a name the
    /// graph invented to sequence them — `.ronin_grouped_join/N` — and the file
    /// the Makefile named is reached through the edge rather than off its
    /// output list. Touching what is declared would try to create a file in a
    /// directory that is not there and leave the real target as stale as it
    /// found it, so the indirection is followed here rather than trusted.
    fn touchable_outputs(&self, edge: EdgeId) -> Vec<NodeId> {
        if let Some(freshness) = self.graph.deferred_freshness(edge) {
            return freshness.outputs.to_vec();
        }
        if let Some(observed) = self.graph.completion_join_output(edge) {
            return vec![observed];
        }
        self.graph
            .edge(edge)
            .out
            .iter()
            .copied()
            .filter(|output| !self.graph.is_virtual_output(*output))
            .collect()
    }
}
