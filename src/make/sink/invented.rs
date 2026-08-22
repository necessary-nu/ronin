//! The names the compiler gives itself.
//!
//! Two of the graph's nodes stand for work rather than for a file the Makefile
//! ever mentioned, and each needs a name nothing else can already be using. The
//! loop is the same in both: take the next number, and take the one after it if
//! the graph has somehow been given that name already.

use super::GraphSink;
use crate::frontend::{FrontendError, Node};
use kati::anyhow;

impl GraphSink {
    /// A recursive child may build the real member named by its parent
    /// grouped action. The parent's public completion point therefore needs a
    /// graph-only identity while it continues to observe that real file.
    pub(super) fn completion_proxy(&mut self) -> Result<Node, anyhow::Error> {
        loop {
            let path = format!(".ronin_grouped_join/{}", self.completion_proxies);
            self.completion_proxies += 1;
            if self.graph.lookup(path.as_bytes()).is_some() {
                continue;
            }
            return self
                .graph
                .node(path.as_bytes())
                .map_err(|failure| self.refuse(failure));
        }
    }

    /// A run of a recipe's own lines makes no file the Makefile named, so it
    /// needs a name of its own for the compilation to ask for it by.
    ///
    /// Never written and never read: the edge under it is always dirty, and
    /// what makes the run count is the compiler seeing it finish rather than
    /// anything appearing on the disk. Said to the graph rather than left to be
    /// inferred, because the build cannot tell a handle from a file by looking
    /// at it and would otherwise create the directory this name appears to sit
    /// in.
    pub(super) fn recipe_stage_proxy(&mut self) -> Result<Node, FrontendError> {
        loop {
            let path = format!(".ronin_recipe_stage/{}", self.recipe_stages);
            self.recipe_stages += 1;
            if self.graph.lookup(path.as_bytes()).is_some() {
                continue;
            }
            let node = self.graph.node(path.as_bytes())?;
            self.graph.mark_invented_output(node);
            return Ok(node);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GraphSink;

    /// Each name is its own, and one the graph already holds is stepped over
    /// rather than handed out twice — which is the whole reason these are
    /// loops rather than a counter read once.
    #[test]
    fn an_invented_name_is_never_reused() {
        let mut sink = GraphSink::new();
        let first = sink.completion_proxy().unwrap();
        let second = sink.completion_proxy().unwrap();
        assert_ne!(first, second);
        assert_eq!(sink.graph.lookup(b".ronin_grouped_join/0"), Some(first));
        assert_eq!(sink.graph.lookup(b".ronin_grouped_join/1"), Some(second));

        // A build file that wrote the name the compiler was about to invent.
        let taken = sink.graph.node(b".ronin_recipe_stage/0").unwrap();
        let staged = sink.recipe_stage_proxy().unwrap();
        assert_ne!(staged, taken);
        assert_eq!(sink.graph.lookup(b".ronin_recipe_stage/1"), Some(staged));
    }
}
