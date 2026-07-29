use std::error;
use std::fmt;
use std::io;

/// The subsystem that originated a Ronin error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ErrorKind {
    /// Command-line parsing or invocation setup failed.
    Cli,
    /// Manifest scanning, parsing, or dyndep loading failed.
    Manifest,
    /// Graph construction or evaluation failed.
    Graph,
    /// Build planning or execution failed.
    Build,
    /// A Ninja-compatible persistent state or filesystem operation failed.
    Persistence,
    /// Process supervision or jobserver integration failed.
    Process,
    /// A Ninja tool-mode operation failed.
    Tool,
}

/// A structured Ronin failure.
///
/// `Display` is the Ninja-compatible diagnostic text. `kind()` and
/// `Error::source()` retain machine-inspectable context without changing that
/// text.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
    source: Option<Box<dyn error::Error + Send + Sync>>,
}

impl Error {
    pub(crate) fn message(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn source(
        kind: ErrorKind,
        source: impl error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: source.to_string(),
            source: Some(Box::new(source)),
        }
    }

    pub(crate) fn context(
        kind: ErrorKind,
        context: impl fmt::Display,
        source: impl error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            kind,
            message: format!("{context}: {source}"),
            source: Some(Box::new(source)),
        }
    }

    /// Returns the subsystem that originated this failure.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn error::Error + 'static))
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::message(ErrorKind::Cli, message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::message(ErrorKind::Cli, message)
    }
}

impl From<io::Error> for Error {
    fn from(source: io::Error) -> Self {
        Self::source(ErrorKind::Persistence, source)
    }
}

macro_rules! domain_error {
    ($name:ident, $kind:ident) => {
        #[derive(Debug)]
        pub(crate) struct $name(Error);

        impl From<String> for $name {
            fn from(message: String) -> Self {
                Self(Error::message(ErrorKind::$kind, message))
            }
        }

        impl From<&str> for $name {
            fn from(message: &str) -> Self {
                Self(Error::message(ErrorKind::$kind, message))
            }
        }

        impl From<io::Error> for $name {
            fn from(source: io::Error) -> Self {
                Self(Error::source(ErrorKind::$kind, source))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl error::Error for $name {
            fn source(&self) -> Option<&(dyn error::Error + 'static)> {
                self.0.source()
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0.message == *other
            }
        }

        impl PartialEq<String> for $name {
            fn eq(&self, other: &String) -> bool {
                self.0.message == *other
            }
        }

        impl From<$name> for Error {
            fn from(error: $name) -> Self {
                error.0
            }
        }
    };
}

domain_error!(ManifestError, Manifest);
domain_error!(GraphError, Graph);
domain_error!(BuildError, Build);
domain_error!(PersistenceError, Persistence);
domain_error!(ProcessError, Process);
domain_error!(ToolError, Tool);

impl BuildError {
    pub(crate) fn source(source: impl error::Error + Send + Sync + 'static) -> Self {
        Self(Error::source(ErrorKind::Build, source))
    }
}

impl ProcessError {
    pub(crate) fn context(
        context: impl fmt::Display,
        source: impl error::Error + Send + Sync + 'static,
    ) -> Self {
        Self(Error::context(ErrorKind::Process, context, source))
    }
}

macro_rules! propagate_error {
    ($target:ident <- $($source:ident),+ $(,)?) => {
        $(
            impl From<$source> for $target {
                fn from(error: $source) -> Self {
                    Self(error.0)
                }
            }
        )+
    };
}

propagate_error!(ManifestError <- GraphError);
propagate_error!(BuildError <- ManifestError, GraphError, PersistenceError, ProcessError, ToolError);
propagate_error!(ToolError <- GraphError, ManifestError);

impl From<crate::dyndep::DyndepError> for ManifestError {
    fn from(source: crate::dyndep::DyndepError) -> Self {
        Self(Error::source(ErrorKind::Manifest, source))
    }
}

impl From<crate::dyndep::DyndepError> for BuildError {
    fn from(source: crate::dyndep::DyndepError) -> Self {
        Self(Error::source(ErrorKind::Manifest, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn preserves_kind_display_and_source() {
        let error = Error::context(
            ErrorKind::Persistence,
            "loading build log",
            io::Error::new(io::ErrorKind::InvalidData, "bad header"),
        );
        assert_eq!(error.kind(), ErrorKind::Persistence);
        assert_eq!(error.to_string(), "loading build log: bad header");
        assert_eq!(error.source().unwrap().to_string(), "bad header");

        let error: Error = ManifestError::from("unexpected EOF").into();
        assert_eq!(error.kind(), ErrorKind::Manifest);
        assert_eq!(error.to_string(), "unexpected EOF");
        assert!(error.source().is_none());
    }
}
