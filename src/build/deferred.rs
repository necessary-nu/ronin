use super::{
    BuildError, BuildResult, Builder, EdgeId, FileTime, Graph, NodeId, Plan, RuntimeState,
};
use crate::graph::{edgeaddorderonly, nodestat_with};
use crate::util::{BString, ByteSlice};
use std::collections::BTreeSet;
use std::path::Path;

enum DeferredWork {
    Ordinary,
    Skip,
    Activate(Vec<NodeId>),
    Run,
}

/// Why an edge the build reached finished without a command having run.
#[derive(Clone, Copy)]
pub(super) enum Unrun {
    /// A deferred edge whose prerequisites turned out not to be newer than its
    /// outputs once they had all settled.
    Skipped,
    /// A deferred phony edge: work that happened, in the only sense a phony
    /// edge ever does work.
    Phony,
    /// The recipe was read as the edge launched and held no command line.
    NoCommand,
}

#[derive(Clone, Copy)]
enum NewInputsReferenceContext {
    /// The recipe is nested in the outer shell's double-quoted `-c` argument.
    InlineCommand,
    /// The recipe is written directly to a response script.
    ResponseFile,
}

fn escape_double_quoted_shell(bytes: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(bytes.len());
    for byte in bytes {
        if matches!(byte, b'"' | b'$' | b'\\' | b'`') {
            escaped.push(b'\\');
        }
        escaped.push(*byte);
    }
    escaped
}

fn replace_all(source: &[u8], needle: &[u8], replacement: &[u8]) -> BString {
    if needle.is_empty() || source.find(needle).is_none() {
        return BString::from(source);
    }
    let mut resolved = Vec::with_capacity(source.len() + replacement.len());
    let mut remaining = source;
    while let Some(at) = remaining.find(needle) {
        resolved.extend_from_slice(&remaining[..at]);
        resolved.extend_from_slice(replacement);
        remaining = &remaining[at + needle.len()..];
    }
    resolved.extend_from_slice(remaining);
    BString::from(resolved)
}

impl Plan {
    /// The edges this plan would report as work, in the order the graph holds
    /// them.
    pub(super) fn reportable_work_edges(
        &self,
        graph: &Graph,
        runtime: &RuntimeState,
    ) -> Vec<EdgeId> {
        self.wanted
            .iter()
            .zip(graph.edge_ids())
            .filter(|(wanted, edge)| {
                if !**wanted {
                    return false;
                }
                if graph.deferred_freshness(*edge).is_some() {
                    return runtime
                        .deferred(*edge)
                        .is_some_and(crate::runtime::DeferredRuntime::initial_run);
                }
                let rule = graph.edge(*edge).rule;
                rule.is_some() && !graph.is_phony_rule(rule)
            })
            .map(|(_, edge)| edge)
            .collect()
    }

    pub(super) fn reportable_work_count(&self, graph: &Graph, runtime: &RuntimeState) -> usize {
        self.reportable_work_edges(graph, runtime).len()
    }
}

impl Builder<'_> {
    /// Re-evaluate normal inputs after every prerequisite has completed but
    /// before the command starts. The real-output baseline was captured by the
    /// dirty walk before it descended into those prerequisites.
    fn deferred_work(&mut self, edge: EdgeId) -> DeferredWork {
        let Some(freshness) = self.graph.deferred_freshness(edge) else {
            return DeferredWork::Ordinary;
        };
        let always_new = freshness.always_new_inputs.clone();
        let excluded = freshness.excluded_new_inputs.clone();
        let activations = freshness.activations.to_vec();
        let normal_inputs = self.graph.edge(edge).non_order_only_inputs().to_vec();
        let state = self
            .runtime
            .deferred(edge)
            .expect("the initial dirty walk captured deferred freshness");
        if state.activation_attached() {
            return DeferredWork::Run;
        }
        let baseline = state.baseline();
        let all_inputs_new = state.all_inputs_new();
        let mut should_run = state.initial_run();
        let mut seen = BTreeSet::new();
        let mut new_inputs = Vec::new();
        for input in normal_inputs {
            let input_state = self.runtime.node(input);
            let is_new = all_inputs_new
                || always_new.contains(&input)
                || input_state.mtime().is_missing()
                || input_state.mtime() > baseline;
            if is_new {
                should_run = true;
                if seen.insert(input) && !excluded.contains(&input) {
                    new_inputs.push(input);
                }
            }
        }
        // A dry run cannot observe the changes prerequisite commands would
        // have made. Reaching a candidate means such work was planned, so the
        // deferred contract carries that hypothetical update across the same
        // boundary.
        //
        // Unless every prerequisite that would have made one turned out to
        // have no command. A recipe read at launch and found to hold nothing
        // updates its target under a dry run exactly as much as it does under
        // a build — not at all — so there is no hypothetical update left to
        // carry, and a dependent that was only a candidate on account of it is
        // as up to date as GNU Make says it is.
        if self.options.dryrun
            && self
                .runtime
                .deferred(edge)
                .is_some_and(crate::runtime::DeferredRuntime::candidate_only)
            && !self.every_input_ran_nothing(edge)
        {
            should_run = true;
        }
        self.runtime.deferred_mut(edge).set_new_inputs(new_inputs);
        if !should_run {
            return DeferredWork::Skip;
        }
        if !activations.is_empty() {
            edgeaddorderonly(self.graph, edge, &activations);
            self.runtime.deferred_mut(edge).attach_activations();
            return DeferredWork::Activate(activations);
        }
        DeferredWork::Run
    }

    /// Whether every one of this edge's ordinary prerequisites is written by an
    /// edge the build reached and found to have no command.
    ///
    /// Conservative in the one direction that matters: a prerequisite nothing
    /// generates, or one whose generator did have a command, answers no and
    /// leaves the dry run's assumption exactly where it was.
    fn every_input_ran_nothing(&self, edge: EdgeId) -> bool {
        let inputs = self.graph.edge(edge).non_order_only_inputs();
        !inputs.is_empty()
            && inputs.iter().all(|input| {
                self.graph
                    .node(*input)
                    .generator
                    .is_some_and(|generator| self.ran_nothing_edges.contains(&generator))
            })
    }

    /// The prerequisite names this edge's command is to be handed, spelt from
    /// where that command runs.
    ///
    /// GNU Make's recursive child answers `$?` with the names its own Makefile
    /// wrote — `a b`, never `sub/a sub/b` — and the recipe that reads them runs
    /// in `sub`, so a qualified name would point at a file that is not there
    /// from where it stands. A name that reaches outside the unit's directory
    /// keeps the only spelling it has, which is GNU Make's answer for an
    /// absolute prerequisite too.
    fn deferred_new_inputs_value(&self, edge: EdgeId) -> Vec<u8> {
        let directory = self
            .graph
            .deferred_freshness(edge)
            .map(|freshness| freshness.new_inputs_directory.as_bytes())
            .unwrap_or_default();
        let mut value = Vec::new();
        if let Some(state) = self.runtime.deferred(edge) {
            for input in state.new_inputs() {
                if !value.is_empty() {
                    value.push(b' ');
                }
                let path = self.graph.node_path(*input);
                value.extend_from_slice(&relative_to(path.as_bytes(), directory));
            }
        }
        value
    }

    fn resolve_deferred_new_inputs(
        &self,
        edge: EdgeId,
        source: &BString,
        context: NewInputsReferenceContext,
    ) -> BString {
        let Some(freshness) = self.graph.deferred_freshness(edge) else {
            return source.clone();
        };
        if freshness.new_inputs_variable.is_empty() {
            return source.clone();
        }
        let mut reference = Vec::with_capacity(freshness.new_inputs_variable.len() + 3);
        reference.extend_from_slice(b"${");
        reference.extend_from_slice(&freshness.new_inputs_variable);
        reference.push(b'}');
        let value = self.deferred_new_inputs_value(edge);
        match context {
            NewInputsReferenceContext::InlineCommand => {
                reference.insert(0, b'\\');
                replace_all(source, &reference, &escape_double_quoted_shell(&value))
            }
            NewInputsReferenceContext::ResponseFile => replace_all(source, &reference, &value),
        }
    }

    pub(super) fn deferred_launch_command(&self, edge: EdgeId, command: &BString) -> BString {
        self.resolve_deferred_new_inputs(edge, command, NewInputsReferenceContext::InlineCommand)
    }

    pub(super) fn deferred_response_file_content(
        &self,
        edge: EdgeId,
        content: &BString,
    ) -> BString {
        self.resolve_deferred_new_inputs(edge, content, NewInputsReferenceContext::ResponseFile)
    }

    /// Complete an edge the build reached and did not run.
    ///
    /// However it got here the outputs are whatever they already were: they
    /// are re-stat'd rather than assumed, and the edge is settled as clean,
    /// because nothing wrote them.
    ///
    /// What differs is whether the dependents are told. A dry run does not
    /// write the outputs a command would have written, so re-reading them
    /// there would say every dependent is up to date when the point of the
    /// exercise is to say what would run — which is why [`Unrun::Skipped`] and
    /// [`Unrun::Phony`] leave the dependents alone under `-n`. An edge with no
    /// command is the case where the two runs agree: nothing was going to be
    /// written either way, so the dependents are told in both.
    pub(super) fn finish_without_command(
        &mut self,
        edge: EdgeId,
        how: Unrun,
    ) -> BuildResult<(bool, Vec<NodeId>)> {
        let executed = matches!(how, Unrun::Phony);
        let deferred = self.graph.deferred_freshness(edge).is_some();
        let outputs = self.graph.deferred_freshness(edge).map_or_else(
            || self.graph.edge(edge).out.clone(),
            |freshness| freshness.outputs.clone(),
        );
        let disk = self.disk.clone();
        let mut logical_mtime = FileTime::MISSING;
        for output in outputs {
            let mut stat = |path: &Path| disk.stat(path);
            nodestat_with(self.graph, &mut self.runtime, output, &mut stat)?;
            logical_mtime = logical_mtime.max(self.runtime.node(output).mtime());
        }
        for output in &self.graph.edge(edge).out {
            let state = self.runtime.node_mut(*output);
            state.set_mtime(logical_mtime);
            state.set_dirty(false);
        }
        if deferred {
            self.runtime.deferred_mut(edge).settle();
        }
        self.runtime.edge_mut(edge).set_command_dirty(false);
        self.runtime.edge_mut(edge).set_restat_clean(!executed);
        let told = matches!(how, Unrun::NoCommand) || !self.options.dryrun;
        Ok((told, Vec::new()))
    }

    /// Take an edge the build reached and did not run out of the work it
    /// expects, the way a `restat` prune takes a consumer out of it.
    ///
    /// It was counted when the plan was made, because a plan cannot know what a
    /// recipe expands to or what a prerequisite's command will leave behind;
    /// counting it still would leave the progress line reaching for work that
    /// is not coming.
    pub(super) fn forget_unrun_edge(&mut self, edge: EdgeId) {
        let rule = self.graph.edge(edge).rule;
        let counted = rule.is_some() && !self.graph.is_phony_rule(rule);
        if self.plan.unwant(edge) && counted {
            super::status::forget_pruned_work(
                &mut self.progress,
                self.graph,
                self.build_log.as_deref(),
                &[edge],
            );
        }
    }

    /// Settle an edge whose recipe was read as it launched and held no command
    /// line, and say whether the build may carry on.
    pub(super) fn settle_unrun_edge(
        &mut self,
        edge: EdgeId,
        failures: &mut usize,
        failure_limit: usize,
        last_error: &mut Option<BuildError>,
    ) -> bool {
        self.ran_nothing_edges.insert(edge);
        self.forget_unrun_edge(edge);
        let result = self.finish_without_command(edge, Unrun::NoCommand);
        if let Err(error) = self.settle_edge(edge, result) {
            *failures += 1;
            *last_error = Some(error);
        }
        *failures < failure_limit
    }

    /// Resolve late work before the ordinary scheduler dispatches an edge.
    /// Returns whether the caller should continue with normal edge handling.
    pub(super) fn advance_deferred(
        &mut self,
        edge: EdgeId,
        failures: &mut usize,
        failure_limit: usize,
        last_error: &mut Option<BuildError>,
    ) -> bool {
        match self.deferred_work(edge) {
            DeferredWork::Skip => {
                self.forget_unrun_edge(edge);
                let result = self.finish_without_command(edge, Unrun::Skipped);
                if let Err(error) = self.settle_edge(edge, result) {
                    *failures += 1;
                    *last_error = Some(error);
                }
                false
            }
            DeferredWork::Activate(roots) => {
                self.plan.defer_work(self.graph, edge);
                let activated = (|| -> BuildResult<()> {
                    for root in roots {
                        self.add_target_node(root)?;
                    }
                    self.plan.refresh_dependencies(self.graph, &self.runtime)?;
                    self.progress.total = self.plan.command_edge_count(self.graph);
                    Ok(())
                })();
                if let Err(error) = activated {
                    *failures = failure_limit;
                    *last_error = Some(error);
                }
                false
            }
            DeferredWork::Run if self.graph.is_phony_rule(self.graph.edge(edge).rule) => {
                let result = self.finish_without_command(edge, Unrun::Phony);
                if let Err(error) = self.settle_edge(edge, result) {
                    *failures += 1;
                    *last_error = Some(error);
                }
                false
            }
            DeferredWork::Ordinary | DeferredWork::Run => true,
        }
    }
}

/// `path` spelt from `directory`, undoing the qualification a compilation unit
/// read elsewhere put on it.
///
/// The unit's Makefile wrote `a`, `d/a` or `../up`, and the graph holds
/// `sub/a`, `sub/d/a` and — once the `..` has met the prefix — `up`. Taking the
/// prefix off is enough for the first two and not for the third, so the answer
/// is the ordinary relative path between the two, which gives the `..` back.
///
/// Component work rather than a resolution against the filesystem: both names
/// come out of one graph and are already normalised against each other, and
/// nothing here should touch the disk. A name that cannot be spelt from the
/// directory at all — an absolute prerequisite against a relative unit — keeps
/// the only spelling it has, which is GNU Make's answer for one too.
fn relative_to(path: &[u8], directory: &[u8]) -> Vec<u8> {
    if directory.is_empty() {
        return path.to_vec();
    }
    let separator = std::path::MAIN_SEPARATOR as u8;
    let components = |name: &[u8]| {
        name.split(move |byte| *byte == separator)
            .filter(|component| !component.is_empty() && *component != b".")
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>()
    };
    let absolute = |name: &[u8]| name.first() == Some(&separator);
    if absolute(path) != absolute(directory) {
        return path.to_vec();
    }
    let from = components(directory);
    let to = components(path);
    let shared = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    // A `..` past the shared part would have to be resolved against the disk to
    // be spelt at all, which is exactly what this must not do.
    if from[shared..].iter().any(|component| component == b"..") {
        return path.to_vec();
    }
    let mut relative = Vec::with_capacity(path.len());
    for component in from[shared..]
        .iter()
        .map(|_| b"..".as_slice())
        .chain(to[shared..].iter().map(Vec::as_slice))
    {
        if !relative.is_empty() {
            relative.push(separator);
        }
        relative.extend_from_slice(component);
    }
    if relative.is_empty() {
        return path.to_vec();
    }
    relative
}

#[cfg(test)]
mod tests {
    use super::relative_to;

    fn spelt(path: &str, directory: &str) -> String {
        String::from_utf8(relative_to(path.as_bytes(), directory.as_bytes())).unwrap()
    }

    #[test]
    fn a_child_units_names_are_child_relative() {
        assert_eq!(spelt("sub/a", "sub"), "a");
        assert_eq!(spelt("sub/d/a", "sub"), "d/a");
        // The `..` the child wrote met the prefix in the graph and has to be
        // given back, which is the whole reason this is a relative path rather
        // than a prefix taken off the front.
        assert_eq!(spelt("sub/up", "sub/deep"), "../up");
        assert_eq!(spelt("up", "sub/deep"), "../../up");
    }

    #[test]
    fn a_name_outside_the_child_keeps_its_own() {
        // The root unit, which is every build that never recursed.
        assert_eq!(spelt("a", ""), "a");
        // GNU Make's answer for an absolute prerequisite is the absolute name.
        assert_eq!(spelt("/etc/hostname", "sub"), "/etc/hostname");
        assert_eq!(spelt("sub/a", "/elsewhere"), "sub/a");
        // The directory itself, which has no relative spelling at all.
        assert_eq!(spelt("sub", "sub"), "sub");
    }
}
