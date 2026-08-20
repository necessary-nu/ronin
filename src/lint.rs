//! What compiling a build already established about it, said out loud.
//!
//! Compiling a Makefile or reading a manifest settles a great many questions
//! that neither front end has any reason to answer afterwards, and both throw
//! the answers away as soon as the graph stands. This is the surface that asks
//! for them: one tool, run by name, that reads the file it was given exactly as
//! a build's read phase reads it and then builds nothing at all.
//!
//! Nothing here re-derives a fact. A finding is either something the compiler
//! recorded while it was deciding, or something a check reads off the finished
//! graph; a report that could disagree with the build it describes would be
//! describing a different build.
// [spec:ronin:req:tools.lint]

use crate::cli::{PRODUCT_NAME, RunResult};
use crate::error::{CliError, EncodingContext, ToolError};
use crate::util::{BString, ByteSlice};
use std::fmt::Write as _;
use std::path::Path;

/// The help `-t lint -h` prints.
///
/// It says what the read costs, because the read runs the Makefile's
/// `$(shell)` calls and remakes what the Makefile says to remake, and a tool
/// that let a user believe otherwise would be lying about what it just did.
pub(crate) const HELP: &str = concat!(
    "usage: ronin -t lint [--make|--ninja] [targets]\n",
    "\n",
    "Reports what compiling the build would establish about it, and builds\n",
    "nothing. The input is the file -f names, or build.ninja; a file named\n",
    "Makefile, makefile or GNUmakefile, or one suffixed .mk, is read as a\n",
    "Makefile and every other name as a Ninja manifest. --make and --ninja\n",
    "say which outright.\n",
    "\n",
    "Reading a Makefile evaluates it, exactly as a build's read phase does:\n",
    "$(shell) runs, $(warning) and $(info) print, and a makefile the read\n",
    "must remake is remade. A report gathered from a quieter read would be a\n",
    "report about a different build.\n",
    "\n",
    "Exits 0 when nothing above a note was found, 1 on a warning, and 2 on\n",
    "an error or an input that could not be read.\n",
);

/// Which build language the file lint was handed is written in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    /// A Makefile, read by the Make compiler.
    Makefile,
    /// A Ninja manifest, read by the manifest parser.
    Manifest,
}

/// The kind a file's own name declares it to be.
///
/// The name, and nothing else. Sniffing the directory for whichever of the two
/// happens to exist is exactly how `crate::multicall` declines to answer "which
/// build language is this", and lint has no business answering it a second way.
pub(crate) fn named_kind(path: &Path) -> Kind {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let dot_mk = Path::new(&name)
        .extension()
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case("mk"));
    if matches!(name.as_str(), "GNUmakefile" | "makefile" | "Makefile") || dot_mk {
        Kind::Makefile
    } else {
        Kind::Manifest
    }
}

/// How much a finding matters, which is also what the invocation exits with.
///
/// Three rather than two because a census is mostly notes: an invocation that
/// composed is not a problem, it is the report saying the build composes, and a
/// tool that exited nonzero for saying so would be no use in a pipeline.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Severity {
    /// Something worth knowing, including the line that says what would answer
    /// a warning above it.
    Note,
    /// Something named as a problem.
    Warning,
    /// Something that stops the build, or stopped the read.
    Error,
}

impl Severity {
    /// What an invocation whose worst finding was this one exits with.
    const fn status(self) -> i32 {
        match self {
            Self::Note => 0,
            Self::Warning => 1,
            Self::Error => 2,
        }
    }
}

/// One thing lint has to say, and where about.
pub(crate) struct Finding {
    /// `file:line`, rendered by whoever knows the location, or `None` for a
    /// finding about the whole input.
    location: Option<String>,
    severity: Severity,
    message: String,
}

impl Finding {
    /// A finding that names a problem.
    pub(crate) fn warning(message: impl Into<String>) -> Self {
        Self {
            location: None,
            severity: Severity::Warning,
            message: message.into(),
        }
    }

    /// Something worth saying that names no problem, including the closing
    /// summary.
    pub(crate) fn note(message: impl Into<String>) -> Self {
        Self {
            location: None,
            severity: Severity::Note,
            message: message.into(),
        }
    }

    /// A finding that stops a build, or that stopped the read.
    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self {
            location: None,
            severity: Severity::Error,
            message: message.into(),
        }
    }

    /// This finding in Ronin's shape for it.
    ///
    /// The four established shapes and no fifth one: an error wears the
    /// product prefix because it is the invocation reporting a failure, and a
    /// located warning or note wears the location because it is the compiler
    /// pointing at a line.
    fn render(&self, out: &mut String) {
        match (self.severity, self.location.as_deref()) {
            (Severity::Error, Some(location)) => {
                let _ = writeln!(out, "{PRODUCT_NAME}: {location}: {}", self.message);
            }
            (Severity::Warning, Some(location)) => {
                let _ = writeln!(out, "{location}: warning: {}", self.message);
            }
            (Severity::Note, Some(location)) => {
                let _ = writeln!(out, "{location}: note: {}", self.message);
            }
            (Severity::Warning, None) => {
                let _ = writeln!(out, "{PRODUCT_NAME}: warning: {}", self.message);
            }
            // An unlocated error and an unlocated note wear the same shape:
            // the product prefix is the invocation speaking, and the severity
            // word has no location to sit beside.
            (Severity::Error | Severity::Note, None) => {
                let _ = writeln!(out, "{PRODUCT_NAME}: {}", self.message);
            }
        }
    }
}

/// Everything one lint had to say, in the order it had to say it.
#[derive(Default)]
pub(crate) struct Report {
    rendered: Vec<u8>,
    worst: Option<Severity>,
}

impl Report {
    /// Add a finding.
    pub(crate) fn raise(&mut self, finding: &Finding) {
        let mut line = String::new();
        finding.render(&mut line);
        self.rendered.extend_from_slice(line.as_bytes());
        self.worst = Some(
            self.worst
                .map_or(finding.severity, |worst| worst.max(finding.severity)),
        );
    }

    /// Add every finding in an iterator, in order.
    pub(crate) fn raise_all(&mut self, findings: impl IntoIterator<Item = Finding>) {
        for finding in findings {
            self.raise(&finding);
        }
    }

    /// Pass on text a compiler already rendered, at the severity it carries.
    ///
    /// The words are the compiler's own and already point at their own source
    /// line, so nothing is added to them — the same rule the build path
    /// follows when it drains the same descriptor. `None` is text that is not
    /// a finding at all, which is what a read that had to remake a Makefile
    /// narrated while it did.
    pub(crate) fn pass_on(&mut self, rendered: &[u8], severity: Option<Severity>) {
        if rendered.is_empty() {
            return;
        }
        self.rendered.extend_from_slice(rendered);
        if !self.rendered.ends_with(b"\n") {
            self.rendered.push(b'\n');
        }
        if let Some(severity) = severity {
            self.worst = Some(self.worst.map_or(severity, |worst| worst.max(severity)));
        }
    }

    /// The report, closed with a summary line, and the status it leaves with.
    ///
    /// The summary is a note like any other, so a report that found nothing
    /// still leaves with zero and a caller still gets one line saying what was
    /// read rather than silence it has to interpret.
    pub(crate) fn finish(mut self, summary: &str) -> RunResult {
        self.raise(&Finding::note(summary));
        RunResult {
            stdout: self.rendered,
            stderr: Vec::new(),
            exit_code: self.worst.map_or(0, Severity::status),
        }
    }
}

/// What the tool's own arguments asked for.
pub(crate) struct Request {
    /// The kind named outright, when one was.
    kind: Option<Kind>,
    /// The goals or targets the report is about, which a Makefile read takes
    /// and a manifest read has no use for yet.
    operands: Vec<BString>,
}

/// Read `-t lint`'s own arguments.
fn request(arguments: &[BString]) -> Result<Option<Request>, ToolError> {
    let mut request = Request {
        kind: None,
        operands: Vec::new(),
    };
    for argument in arguments {
        match argument.as_bytes() {
            b"-h" | b"--help" => return Ok(None),
            b"--make" => request.kind = Some(Kind::Makefile),
            b"--ninja" => request.kind = Some(Kind::Manifest),
            option if option.starts_with(b"-") => {
                return Err(ToolError::UnknownOption {
                    tool: "lint",
                    option: argument.clone(),
                });
            }
            _ => request.operands.push(argument.clone()),
        }
    }
    Ok(Some(request))
}

/// Report what compiling the named build would establish about it.
///
/// # Errors
///
/// Returns an error when the tool's own arguments are wrong, or when reading
/// the input failed in a way that is not itself a finding about the build.
// [spec:ronin:req:tools.lint]
pub(crate) fn run(
    runner: &crate::cli::Runner,
    manifest: &BString,
    arguments: &[BString],
    working_directory: &crate::os::WorkingDirectory,
) -> Result<RunResult, crate::Error> {
    let Some(request) = request(arguments)? else {
        return Ok(RunResult {
            stdout: HELP.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: 1,
        });
    };
    let path = manifest.to_path().map_err(|_| CliError::InvalidEncoding {
        context: EncodingContext::ManifestPath,
    })?;
    match request.kind.unwrap_or_else(|| named_kind(path)) {
        Kind::Makefile => makefile(runner, manifest, &request.operands, working_directory),
        Kind::Manifest => manifest_report(manifest, working_directory),
    }
}

/// Report on a Makefile, by compiling it the way a build would.
#[cfg(all(unix, feature = "make"))]
fn makefile(
    runner: &crate::cli::Runner,
    manifest: &BString,
    operands: &[BString],
    working_directory: &crate::os::WorkingDirectory,
) -> Result<RunResult, crate::Error> {
    // The Make compiler reads the process directory, and Ninja's `-C` moved
    // only the invocation's idea of one, so where to read is named outright.
    let mut arguments = vec![
        BString::from(&b"make"[..]),
        BString::from(&b"-C"[..]),
        BString::from(working_directory.as_path().as_os_str().as_encoded_bytes()),
        BString::from(&b"-f"[..]),
        manifest.clone(),
    ];
    arguments.extend_from_slice(operands);
    let read = crate::make::cli::read_without_building(runner, &arguments)?;
    let mut report = Report::default();
    // What a read that had to remake a Makefile narrated while it did. It is
    // the build output of real work, not a finding, and it leads the report
    // because it happened before anything below it was learned.
    report.pass_on(read.reported.as_bytes(), None);
    report.pass_on(&read.raised, Some(Severity::Warning));
    if let Some(stopped) = read.stopped {
        report.pass_on(&stopped.stdout, None);
        report.pass_on(
            &stopped.stderr,
            (stopped.exit_code != 0).then_some(Severity::Error),
        );
        // A read can end without a graph and without a word about why — a
        // question answered rather than a refusal raised. Saying so is the
        // difference between a report that found nothing and no report.
        if stopped.exit_code != 0 && stopped.stderr.is_empty() {
            report.raise(&Finding::error("the read did not produce a graph"));
        }
        return Ok(report.finish("nothing further to report: the read did not finish"));
    }
    Ok(report.finish("read 1 makefile"))
}

/// The same, for a build with no Make front end compiled into it.
#[cfg(not(all(unix, feature = "make")))]
fn makefile(
    _runner: &crate::cli::Runner,
    _manifest: &BString,
    _operands: &[BString],
    _working_directory: &crate::os::WorkingDirectory,
) -> Result<RunResult, crate::Error> {
    let mut report = Report::default();
    report.raise(&Finding::error(
        "this build has no Make front end; it was compiled without the 'make' feature",
    ));
    Ok(report.finish("nothing to report: the input could not be read"))
}

/// Report on a Ninja manifest.
fn manifest_report(
    manifest: &BString,
    working_directory: &crate::os::WorkingDirectory,
) -> Result<RunResult, crate::Error> {
    let path = manifest.to_path().map_err(|_| CliError::InvalidEncoding {
        context: EncodingContext::ManifestPath,
    })?;
    let mut warnings = Vec::new();
    let parsed = crate::parse::load_manifest_reporting(
        path,
        working_directory.clone(),
        crate::frontend::ManifestOptions::default(),
        &mut warnings,
    );
    let mut report = Report::default();
    // Raised before the failure is reported, because the parser writes a
    // warning where it raises it and one raised by an earlier statement
    // outlives the later statement that stopped the parse.
    report.raise_all(warnings.into_iter().map(Finding::warning));
    let graph = match parsed {
        Ok(graph) => graph,
        // Everything the parser refuses outright — a duplicate output, an
        // unknown rule, an unexpected rule variable — arrives here. Lint does
        // not check those a second time; it reports the refusal as the finding
        // it already is, so one command still answers for every static failure
        // a manifest can have.
        Err(failure) => {
            // Already located, with the offending line and a caret under it:
            // that shape is what a manifest diagnostic wears everywhere else
            // in Ronin, and rewrapping it would take the caret off.
            report.pass_on(failure.to_string().as_bytes(), Some(Severity::Error));
            return Ok(report.finish("nothing further to report: the manifest did not parse"));
        }
    };
    let _ = graph;
    Ok(report.finish("read 1 manifest"))
}
