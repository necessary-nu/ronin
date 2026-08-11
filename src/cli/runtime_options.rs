//! Ninja-facing diagnostics around runtime option normalization.

use super::{CliResult, PRODUCT_NAME, append_output, normalize_runtime_options};
use crate::build::{BuildOptions, JobLimit};
use crate::error::{CliError, ProcessError};

// [arch:ronin:cli]
/// The part of Ninja's inherited-jobserver interface that precedes a build.
pub(super) struct JobserverNotice {
    makeflags: String,
    failure: Option<String>,
}

impl JobserverNotice {
    pub(super) fn emit<'a, 'sink>(
        self,
        quiet: bool,
        output: Option<&'a mut (dyn std::io::Write + 'sink)>,
        diagnostics: Option<&'a mut (dyn std::io::Write + 'sink)>,
        buffered_output: &mut String,
        buffered_diagnostics: &mut Vec<u8>,
    ) -> CliResult<()> {
        if !quiet {
            let announcement = format!(
                "{PRODUCT_NAME}: Jobserver mode detected: {}",
                self.makeflags
            );
            if let Some(sink) = output {
                writeln!(sink, "{announcement}").map_err(CliError::write_output)?;
                sink.flush().map_err(CliError::write_output)?;
            } else {
                append_output(buffered_output, &announcement);
            }
        }
        if let Some(failure) = self.failure {
            let diagnostic =
                format!("{PRODUCT_NAME}: error: Could not initialize jobserver: {failure}\n");
            if let Some(sink) = diagnostics {
                sink.write_all(diagnostic.as_bytes())
                    .and_then(|()| sink.flush())
                    .map_err(CliError::write_output)?;
            } else {
                buffered_diagnostics.extend_from_slice(diagnostic.as_bytes());
            }
        }
        Ok(())
    }
}

fn ninja_jobserver_failure(error: &ProcessError) -> String {
    #[cfg(unix)]
    if let ProcessError::JobserverEnvironment { source } = error
        && matches!(source.kind(), jobserver::FromEnvErrorKind::CannotOpenPath)
        && let Some(source) = std::error::Error::source(source)
    {
        return format!(
            "Error opening fifo for reading: {}",
            crate::error::system_message(source)
        );
    }
    crate::error::system_message(error)
}

/// Normalize Ninja mode while retaining its non-fatal jobserver diagnostics.
pub(super) fn normalize_ninja_runtime_options(
    options: &mut BuildOptions,
    makeflags: Option<&str>,
    status_format: Option<&str>,
    terminal: crate::build::TerminalContext,
    connect_jobserver: impl FnOnce() -> Result<jobserver::Client, ProcessError>,
    unshared: JobLimit,
) -> CliResult<Option<JobserverNotice>> {
    let config = crate::jobserver::parse_makeflags_value(makeflags)?;
    let detected = options.jobs == JobLimit::Auto
        && !options.dryrun
        && config.has_mode()
        && config.is_native();
    let mut failure = None;
    normalize_runtime_options(
        options,
        makeflags,
        status_format,
        terminal,
        || {
            let connected = connect_jobserver();
            if let Err(error) = &connected {
                failure = Some(ninja_jobserver_failure(error));
            }
            connected
        },
        unshared,
    )?;
    Ok(detected.then(|| JobserverNotice {
        makeflags: makeflags.unwrap_or_default().to_owned(),
        failure,
    }))
}

#[cfg(test)]
mod tests {
    use super::JobserverNotice;

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn jobserver_notice_uses_ninja_streams() {
        let notice = JobserverNotice {
            makeflags: " -j2 --jobserver-auth=fifo:/tmp/outer".to_owned(),
            failure: Some("cannot connect".to_owned()),
        };
        let (mut output, mut diagnostics) = (Vec::new(), Vec::new());
        let (mut buffered_output, mut buffered_diagnostics) = (String::new(), Vec::new());
        notice
            .emit(
                false,
                Some(&mut output),
                Some(&mut diagnostics),
                &mut buffered_output,
                &mut buffered_diagnostics,
            )
            .unwrap();
        assert_eq!(
            output,
            b"ronin: Jobserver mode detected:  -j2 --jobserver-auth=fifo:/tmp/outer\n"
        );
        assert_eq!(
            diagnostics,
            b"ronin: error: Could not initialize jobserver: cannot connect\n"
        );
        assert!(buffered_output.is_empty());
        assert!(buffered_diagnostics.is_empty());
    }

    // [spec:ronin:req:compat.process-integration/test]
    #[test]
    fn quiet_jobserver_notice_keeps_the_error() {
        let notice = JobserverNotice {
            makeflags: " -j2".to_owned(),
            failure: Some("unavailable".to_owned()),
        };
        let (mut output, mut diagnostics) = (String::new(), Vec::new());
        notice
            .emit(true, None, None, &mut output, &mut diagnostics)
            .unwrap();
        assert!(output.is_empty());
        assert_eq!(
            diagnostics,
            b"ronin: error: Could not initialize jobserver: unavailable\n"
        );
    }
}
