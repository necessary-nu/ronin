//! Recipes expanded when their edge is about to run.
//!
//! GNU Make expands a recipe in `new_job`, immediately before running it and
//! only for a target it decided to remake. Everything the expansion does
//! happens there: a `$(shell)` runs then, an `$(info)` prints then, an
//! `$(error)` stops the build then, and a `$(wildcard)` sees the files every
//! earlier recipe left behind. A target that turns out to be up to date has
//! none of it happen at all.
//!
//! kati compiles a whole graph before anything runs, which is what a manifest
//! needs and what this front end kept while the graph was the only product.
//! Now that the same process runs the graph, the recipes it can hold
//! unexpanded stay unexpanded — the compiler still reads the ones whose text
//! shapes the graph — and the engine asks for one as it launches its edge.

use super::sink::CommandLayout;
use crate::build::{LateCommand, LateCommands};
use crate::graph::EdgeId;
use crate::htab::RapidHashMap;
use crate::util::BString;
use kati::build_sink::DeferredRecipeId;
use kati::eval::Evaluator;
use kati::ninja::DeferredRecipes as KatiRecipes;

/// The evaluation session a build's unexpanded recipes belong to.
///
/// Held for as long as the build may still start one of them. A recipe is
/// expanded against the variables the session that read the Makefile holds,
/// which is the whole reason the session outlives compilation: an expansion
/// against anything else would be a different expansion.
// [spec:ronin:req:make.no-ambient-state]
pub(crate) struct PendingRecipes {
    session: Evaluator,
    recipes: KatiRecipes,
    layout: CommandLayout,
    edges: RapidHashMap<EdgeId, DeferredRecipeId>,
}

impl PendingRecipes {
    /// Retain `session` and the recipes it left unexpanded, keyed by the edge
    /// that runs each one.
    pub(crate) fn new(
        session: Evaluator,
        recipes: KatiRecipes,
        layout: CommandLayout,
        edges: &[(crate::frontend::Edge, DeferredRecipeId)],
    ) -> Self {
        Self {
            session,
            recipes,
            layout,
            edges: edges
                .iter()
                .map(|(edge, recipe)| (edge.id(), *recipe))
                .collect(),
        }
    }
}

impl LateCommands for PendingRecipes {
    fn command(&mut self, edge: EdgeId, output: &[u8]) -> Result<Option<LateCommand>, String> {
        let Some(recipe) = self.edges.get(&edge).copied() else {
            return Ok(None);
        };
        let expanded = self
            .recipes
            .expand(&mut self.session, recipe)
            .map_err(|failure| super::report::diagnostic_body(&failure))?;
        let Some(expanded) = expanded else {
            return Ok(None);
        };
        let launched = self.layout.launch(
            &expanded.shell,
            &expanded.shell_flags,
            &expanded.script,
            output,
            &expanded.recipe_environment,
        );
        // [spec:ronin:req:make.narration+1]
        // The same choice the sink makes for a recipe it expanded itself:
        // what the Makefile said, or the recipe's own text — never the shell
        // and environment wrapper needed to run it, and nothing at all for a
        // script too long to be a description.
        let description = match (&expanded.description, &launched.response_file) {
            (Some(text), _) => BString::from(text.to_vec()),
            (None, None) => BString::from(expanded.script.to_vec()),
            (None, Some(_)) => BString::default(),
        };
        let (rspfile, rspfile_content) = match launched.response_file {
            Some((path, content)) => (Some(BString::from(path)), BString::from(content)),
            None => (None, BString::default()),
        };
        Ok(Some(LateCommand {
            command: BString::from(launched.command),
            description,
            rspfile,
            rspfile_content,
            ignore_errors: expanded.ignore_errors,
        }))
    }
}
