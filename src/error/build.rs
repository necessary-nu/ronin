//! What a build failure answers about itself.
//!
//! Beside [`BuildError`]'s definition rather than in it: the shared error
//! catalogue names every subsystem's failures, and the questions the build loop
//! asks of its own are the build subsystem's business. A child module of
//! `error` so the reach into [`Error`]'s representation below stays private to
//! the catalogue that owns it.

use super::{BuildError, BuildOperation, BuildStop, EdgeId, Error, ErrorKind, ErrorRepr};
use crate::util::BString;
use std::io;

impl Error {
    /// Whether this failure is a goal refused over a makefile the `-q` pass
    /// merely ASKED about.
    ///
    /// The one question the `-q` answer has to put to a planning failure. GNU
    /// Make leaves such a makefile `us_question` and one whose recipe ran and
    /// lost `us_failed`, refuses a goal over either, and turns the first into
    /// `MAKE_TROUBLE` and the second into `MAKE_FAILURE` — so this decides
    /// between answering 1 and answering 2. The verdict is read back off the
    /// refusal rather than recomputed, having been stamped where the graph
    /// still knew which it was.
    pub(crate) const fn refused_a_questioned_makefile(&self) -> bool {
        matches!(
            &self.0,
            ErrorRepr::Build(BuildError::MissingInput {
                questioned: true,
                ..
            })
        )
    }
}

impl BuildError {
    /// This failure's front-end diagnostic, through the summary the build loop
    /// wrapped it in.
    pub(crate) fn front_end_diagnostic(&self) -> Option<&str> {
        match self {
            Self::LateCommand { diagnostic } => Some(diagnostic),
            Self::Stopped {
                reason: BuildStop::Failed(inner),
                ..
            } => inner.front_end_diagnostic(),
            _ => None,
        }
    }

    /// The process exit status this failure carries out of the build.
    ///
    /// Ninja propagates a failing command's own status so a caller can tell a
    /// compile error from an out-of-memory kill. Anything that is not a command
    /// reporting for itself is a plain failure.
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::SubcommandFailed { status, .. } => crate::subprocess::exit_status_code(*status),
            Self::Interrupted { .. } => crate::subprocess::INTERRUPTED_EXIT_CODE,
            Self::Stopped { status, .. } => *status,
            _ => 1,
        }
    }

    pub(crate) const fn io(
        operation: BuildOperation,
        path: Option<BString>,
        edge: Option<EdgeId>,
        source: io::Error,
    ) -> Self {
        Self::Io {
            operation,
            path,
            edge,
            source,
        }
    }

    pub(crate) fn target_context(source: Self) -> Self {
        Self::TargetContext {
            source: Box::new(source),
        }
    }

    pub(super) const fn kind(&self) -> ErrorKind {
        match self {
            Self::Manifest(error) => error.kind(),
            Self::Graph(_) => ErrorKind::Graph,
            Self::Persistence(_) => ErrorKind::Persistence,
            Self::Process(_) => ErrorKind::Process,
            Self::Tool(error) => error.kind(),
            Self::TargetContext { source } => source.kind(),
            _ => ErrorKind::Build,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BuildError, BuildStop};
    use crate::error::{Error, ErrorRepr};
    use crate::util::BString;

    fn command_failure(raw: i32) -> BuildError {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt as _;
        BuildError::SubcommandFailed {
            edge: crate::graph::EdgeId::from_event_key(1).expect("one names a slot"),
            command: BString::from("exit"),
            status: std::process::ExitStatus::from_raw(raw),
        }
    }

    // [spec:ronin:req:compat.command-runtime/test]
    // [spec:ronin:req:product.build-outcome+1/test]
    #[test]
    fn stopped_build_reports_reason_and_status() {
        // Exhausting the allowance says the build was cut off; having allowance
        // left says it went as far as everything not behind a failure allowed.
        let cut_off = BuildStop::from_failure(command_failure(7 << 8), 1, 1, 1);
        assert_eq!(cut_off.to_string(), "subcommand failed");
        let plural = BuildStop::from_failure(command_failure(7 << 8), 2, 2, 2);
        assert_eq!(plural.to_string(), "subcommands failed");
        let exhausted = BuildStop::from_failure(command_failure(7 << 8), 1, usize::MAX, usize::MAX);
        assert_eq!(
            exhausted.to_string(),
            "cannot make progress due to previous errors"
        );
        let interrupted =
            BuildStop::from_failure(BuildError::Interrupted { status: None }, 1, 1, 1);
        assert_eq!(interrupted.to_string(), "interrupted by user");

        // An error that is not a command's own status keeps its description,
        // because a summary would be the only account of what went wrong.
        let internal = BuildError::UnsupportedDepsType {
            edge: crate::graph::EdgeId::from_event_key(1).expect("one names a slot"),
            deps_type: "clang".to_owned(),
        };
        let other = BuildStop::from_failure(internal, 1, 1, 1);
        assert_eq!(other.to_string(), "unsupported deps type 'clang'");

        let stopped = BuildError::Stopped {
            reason: cut_off,
            status: 7,
        };
        assert_eq!(stopped.to_string(), "build stopped: subcommand failed.");
        assert_eq!(stopped.exit_code(), 7);
        assert_eq!(command_failure(7 << 8).exit_code(), 7);
        assert_eq!(BuildError::Interrupted { status: None }.exit_code(), 130);
        assert_eq!(
            BuildError::InvalidDepsEncoding {
                edge: crate::graph::EdgeId::from_event_key(1).expect("one names a slot"),
            }
            .exit_code(),
            1
        );
    }

    /// The `-q` answer reads its status off the refusal, so a failure that is
    /// not a refusal at all must never be mistaken for the cheap one. The
    /// refusal's own two arms need a node the graph minted, so they are held by
    /// `keep_going_keeps_the_questions_status` in `tests/make_regressions.rs`,
    /// which puts the question to a real Makefile.
    #[test]
    fn only_a_refusal_carries_a_question() {
        for failure in [
            BuildError::Interrupted { status: None },
            BuildError::UnknownTarget {
                path: BString::from("one.mk"),
            },
            command_failure(7 << 8),
        ] {
            assert!(!Error(ErrorRepr::Build(failure)).refused_a_questioned_makefile());
        }
    }
}
