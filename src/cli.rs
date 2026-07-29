//! Ronin command-line parsing and runtime orchestration.

use crate::build::BuildOptions;
use crate::parse::ParseOptions;
use crate::util::{BString, ByteSlice, ByteVec};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

// [spec:samurai:req:product.ronin-identity]
pub const PRODUCT_NAME: &str = "ronin";

// [spec:samurai:req:compat.version-reporting]
pub const NINJA_COMPAT_VERSION: &str = "1.9.0";

// [spec:samurai:req:compat.ninja-owned-names]
const DEFAULT_MANIFEST: &str = "build.ninja";
const NINJA_STATUS_ENV: &str = "NINJA_STATUS";

// [spec:samurai:def:samu.usage-fn]
// [spec:samurai:sem:samu.usage-fn]
pub fn usage(program: &str) -> String {
    format!("usage: {program} [-C dir] [-f buildfile] [-j maxjobs] [-k maxfail] [-l maxload] [-n]")
}

// [spec:samurai:def:samu.getbuilddir-fn]
// [spec:samurai:sem:samu.getbuilddir-fn]
pub fn getbuilddir(builddir: Option<&Path>) -> Result<Option<PathBuf>, String> {
    let Some(builddir) = builddir else {
        return Ok(None);
    };
    std::fs::create_dir_all(builddir).map_err(|error| error.to_string())?;
    Ok(Some(builddir.to_path_buf()))
}

// [spec:samurai:def:samu.debugflag-fn]
// [spec:samurai:sem:samu.debugflag-fn]
pub fn debugflag(options: &mut BuildOptions, flag: &str) -> Result<(), String> {
    match flag {
        "explain" => options.explain = true,
        "keepdepfile" => options.keepdepfile = true,
        "keeprsp" => options.keeprsp = true,
        _ => return Err(format!("unknown debug flag '{flag}'")),
    }
    Ok(())
}

// [spec:samurai:def:samu.loadflag-fn]
// [spec:samurai:sem:samu.loadflag-fn]
pub fn loadflag(options: &mut BuildOptions, flag: &str) -> Result<(), String> {
    let value: f64 = flag
        .parse()
        .map_err(|_| "invalid -l parameter".to_owned())?;
    if value < 0.0 {
        return Err("invalid -l parameter".into());
    }
    options.maxload = value;
    Ok(())
}

// [spec:samurai:def:samu.warnflag-fn]
// [spec:samurai:sem:samu.warnflag-fn]
pub fn warnflag(options: &mut ParseOptions, flag: &str) -> Result<(), String> {
    match flag {
        "dupbuild=err" => options.dupbuildwarn = false,
        "dupbuild=warn" => options.dupbuildwarn = true,
        _ => return Err(format!("unknown warning flag '{flag}'")),
    }
    Ok(())
}

// [spec:samurai:def:samu.jobsflag-fn]
// [spec:samurai:sem:samu.jobsflag-fn]
pub fn jobsflag(options: &mut BuildOptions, flag: &str) -> Result<(), String> {
    let value: i64 = flag
        .parse()
        .map_err(|_| "invalid -j parameter".to_owned())?;
    if value < 0 {
        return Err("invalid -j parameter".into());
    }
    options.maxjobs = if value == 0 {
        usize::MAX
    } else {
        value as usize
    };
    Ok(())
}

// [spec:samurai:def:samu.progname-fn]
// [spec:samurai:sem:samu.progname-fn]
pub fn progname(argument: Option<&str>, default: &str) -> String {
    argument
        .and_then(|argument| argument.rsplit('/').next())
        .unwrap_or(default)
        .to_owned()
}

// [spec:samurai:def:samu.main-fn+1]
// [spec:samurai:sem:samu.main-fn+1]
pub fn main(arguments: &[String]) -> Result<(BuildOptions, ParseOptions, String), String> {
    let mut build = BuildOptions::default();
    let mut parse = ParseOptions::default();
    let mut manifest = DEFAULT_MANIFEST.to_owned();
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "-f" => {
                index += 1;
                manifest = arguments
                    .get(index)
                    .ok_or_else(|| "missing -f value".to_owned())?
                    .clone();
            }
            "-j" => {
                index += 1;
                jobsflag(
                    &mut build,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -j value".to_owned())?,
                )?;
            }
            "-l" => {
                index += 1;
                loadflag(
                    &mut build,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -l value".to_owned())?,
                )?;
            }
            "-k" => {
                index += 1;
                let value: usize = arguments
                    .get(index)
                    .ok_or_else(|| "missing -k value".to_owned())?
                    .parse()
                    .map_err(|_| "invalid -k parameter".to_owned())?;
                build.maxfail = if value == 0 { usize::MAX } else { value };
            }
            "-n" => build.dryrun = true,
            "-v" => build.verbose = true,
            "-d" => {
                index += 1;
                debugflag(
                    &mut build,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -d value".to_owned())?,
                )?;
            }
            "-w" => {
                index += 1;
                warnflag(
                    &mut parse,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -w value".to_owned())?,
                )?;
            }
            option => {
                return Err(format!(
                    "{}: {option}",
                    usage(&progname(
                        arguments.first().map(String::as_str),
                        PRODUCT_NAME
                    ))
                ))
            }
        }
        index += 1;
    }
    Ok((build, parse, manifest))
}

fn append_output(output: &mut String, addition: &str) {
    if addition.is_empty() {
        return;
    }
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(addition);
}

struct RunInvocation {
    build_options: BuildOptions,
    parse_options: ParseOptions,
    manifest: BString,
    targets: Vec<BString>,
    selected_tool: Option<crate::tool::Tool>,
    tool_arguments: Vec<BString>,
}

enum RunAction {
    Version,
    Execute(RunInvocation),
}

// [spec:samurai:def:samu.parseenvargs-fn+1]
// [spec:samurai:sem:samu.parseenvargs-fn+1]
// [spec:samurai:req:product.no-samuflags]
fn parse_run_arguments(arguments: &[BString]) -> Result<RunAction, String> {
    let mut invocation = RunInvocation {
        build_options: BuildOptions::default(),
        parse_options: ParseOptions::default(),
        manifest: DEFAULT_MANIFEST.into(),
        targets: Vec::new(),
        selected_tool: None,
        tool_arguments: Vec::new(),
    };
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_bytes() {
            b"--version" => return Ok(RunAction::Version),
            b"--verbose" | b"-v" => invocation.build_options.verbose = true,
            b"-C" => {
                index += 1;
                let directory = arguments
                    .get(index)
                    .ok_or_else(|| "missing -C value".to_owned())?;
                std::env::set_current_dir(
                    directory
                        .to_path()
                        .map_err(|_| "-C path is not representable on this platform")?,
                )
                .map_err(|error| error.to_string())?;
            }
            b"-f" => {
                index += 1;
                invocation.manifest = arguments
                    .get(index)
                    .ok_or_else(|| "missing -f value".to_owned())?
                    .clone();
            }
            b"-j" => {
                index += 1;
                jobsflag(
                    &mut invocation.build_options,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -j value".to_owned())?
                        .to_str()
                        .map_err(|_| "invalid -j parameter")?,
                )?;
            }
            b"-k" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| "missing -k value".to_owned())?
                    .to_str()
                    .map_err(|_| "invalid -k parameter")?
                    .parse::<usize>()
                    .map_err(|_| "invalid -k parameter".to_owned())?;
                invocation.build_options.maxfail = if value == 0 { usize::MAX } else { value };
            }
            b"-l" => {
                index += 1;
                loadflag(
                    &mut invocation.build_options,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -l value".to_owned())?
                        .to_str()
                        .map_err(|_| "invalid -l parameter")?,
                )?;
            }
            b"-n" => invocation.build_options.dryrun = true,
            b"-d" => {
                index += 1;
                debugflag(
                    &mut invocation.build_options,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -d value".to_owned())?
                        .to_str()
                        .map_err(|_| "invalid -d parameter")?,
                )?;
            }
            b"-w" => {
                index += 1;
                warnflag(
                    &mut invocation.parse_options,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -w value".to_owned())?
                        .to_str()
                        .map_err(|_| "invalid -w parameter")?,
                )?;
            }
            b"-t" => {
                index += 1;
                invocation.selected_tool = Some(crate::tool::toolget(
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -t value".to_owned())?
                        .to_str()
                        .map_err(|_| "invalid -t parameter")?,
                )?);
                invocation
                    .tool_arguments
                    .extend_from_slice(&arguments[index + 1..]);
                break;
            }
            option if option.starts_with(b"-") => {
                let option = option.to_str_lossy();
                return Err(format!(
                    "{}: {option}",
                    usage(&progname(
                        arguments
                            .first()
                            .map(|argument| argument.to_str_lossy())
                            .as_deref(),
                        PRODUCT_NAME
                    ))
                ));
            }
            target => invocation.targets.push(BString::from(target)),
        }
        index += 1;
    }
    Ok(RunAction::Execute(invocation))
}

// [spec:samurai:req:compat.process-integration]
fn normalize_runtime_options(
    options: &mut BuildOptions,
    makeflags: Option<&str>,
) -> Result<(), String> {
    if options.maxjobs == 0 {
        let jobserver = crate::jobserver::parse_makeflags_value(makeflags)?;
        if cfg!(unix)
            && matches!(
                jobserver.mode,
                crate::jobserver::JobserverMode::PosixFifo | crate::jobserver::JobserverMode::Pipe
            )
        {
            options.maxjobs = usize::MAX;
            options.jobserver = jobserver;
        } else {
            options.maxjobs = match crate::os::osnproc() {
                i64::MIN..=1 => 2,
                2 => 3,
                count => (count + 2) as usize,
            };
        }
    }
    if let Ok(status) = std::env::var(NINJA_STATUS_ENV) {
        options.statusfmt = status;
    }
    Ok(())
}

fn default_target_names(parser: &crate::parse::Parser, graph: &crate::graph::Graph) -> Vec<String> {
    crate::parse::defaultnodes(parser, graph)
        .into_iter()
        .map(|node| {
            let node = graph.node(node);
            String::from_utf8_lossy(node.path.as_bytes()).into_owned()
        })
        .collect()
}

fn default_target_paths(
    parser: &crate::parse::Parser,
    graph: &crate::graph::Graph,
) -> Vec<BString> {
    crate::parse::defaultnodes(parser, graph)
        .into_iter()
        .map(|node| graph.node(node).path.clone())
        .collect()
}

fn run_clean_tool(
    graph: &crate::graph::Graph,
    state: &crate::env::EnvState,
    arguments: &[String],
    dryrun: bool,
) -> Result<String, String> {
    let mut include_generators = false;
    let mut rule_mode = false;
    let mut names = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "-g" => include_generators = true,
            "-r" => rule_mode = true,
            option if option.starts_with('-') => {
                return Err(format!("unknown clean option '{option}'"));
            }
            name => names.push(name.to_owned()),
        }
    }
    if rule_mode && names.is_empty() {
        return Err("expected a rule to clean".into());
    }
    if rule_mode {
        for rule in &names {
            crate::env::envrule(graph, state.root, rule)
                .ok_or_else(|| format!("unknown rule '{rule}'"))?;
        }
    }
    let (targets, rules) = if rule_mode {
        (&[][..], names.as_slice())
    } else {
        (names.as_slice(), &[][..])
    };
    crate::tool::clean_with_options(graph, targets, rules, include_generators, dryrun)
        .map(|removed| removed.to_string())
        .map_err(|error| error.to_string())
}

fn run_compdb_tool(graph: &crate::graph::Graph, arguments: &[String]) -> Result<String, String> {
    let mut expand_rsp = false;
    let mut rules = Vec::new();
    for argument in arguments {
        match argument.as_str() {
            "-x" => expand_rsp = true,
            option if option.starts_with('-') => {
                return Err(format!("unknown compdb option '{option}'"));
            }
            rule => rules.push(rule.to_owned()),
        }
    }
    Ok(crate::tool::compdb(graph, &rules, expand_rsp))
}

fn run_selected_tool(
    tool: crate::tool::Tool,
    graph: &crate::graph::Graph,
    parser: &crate::parse::Parser,
    state: &crate::env::EnvState,
    arguments: &[String],
    dryrun: bool,
) -> Result<String, String> {
    match tool {
        crate::tool::Tool::Clean => run_clean_tool(graph, state, arguments, dryrun),
        crate::tool::Tool::Compdb => run_compdb_tool(graph, arguments),
        tool @ (crate::tool::Tool::Commands | crate::tool::Tool::Graph) => {
            let default_arguments;
            let arguments = if arguments.is_empty() {
                default_arguments = default_target_names(parser, graph);
                &default_arguments
            } else {
                arguments
            };
            crate::tool::run(tool, graph, arguments)
        }
        tool => crate::tool::run(tool, graph, arguments),
    }
}

pub fn run(arguments: &[String]) -> Result<String, String> {
    let arguments = arguments
        .iter()
        .cloned()
        .map(BString::from)
        .collect::<Vec<_>>();
    run_bytes(&arguments, None, None)
}

pub fn run_os(arguments: &[OsString]) -> Result<String, String> {
    let arguments = arguments
        .iter()
        .cloned()
        .map(|argument| {
            Vec::from_os_string(argument)
                .map(BString::from)
                .map_err(|_| "argument is not representable as bytes on this platform".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let stderr = std::io::stderr();
    let mut diagnostics = stderr.lock();
    run_bytes(&arguments, Some(&mut output), Some(&mut diagnostics))
}

fn run_bytes(
    arguments: &[BString],
    mut build_output: Option<&mut dyn std::io::Write>,
    mut build_diagnostics: Option<&mut dyn std::io::Write>,
) -> Result<String, String> {
    let RunAction::Execute(mut invocation) = parse_run_arguments(arguments)? else {
        return Ok(NINJA_COMPAT_VERSION.into());
    };
    let makeflags = std::env::var("MAKEFLAGS").ok();
    normalize_runtime_options(&mut invocation.build_options, makeflags.as_deref())?;

    let mut output = String::new();
    for _ in 0..100 {
        let mut graph = crate::graph::graphinit();
        let mut parser = crate::parse::parseinit();
        parser.options = invocation.parse_options;
        let mut state = crate::env::envinit(&mut graph);
        crate::parse::parse(
            invocation
                .manifest
                .to_path()
                .map_err(|_| "manifest path is not representable on this platform")?,
            &mut graph,
            &mut parser,
            state.root,
            &mut state,
        )?;

        if let Some(tool) = invocation.selected_tool.take() {
            let tool_arguments = invocation
                .tool_arguments
                .iter()
                .map(|argument| {
                    argument
                        .to_str()
                        .map(str::to_owned)
                        .map_err(|_| "tool arguments must be valid UTF-8".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?;
            return run_selected_tool(
                tool,
                &graph,
                &parser,
                &state,
                &tool_arguments,
                invocation.build_options.dryrun,
            );
        }

        let builddir = crate::env::envvar(&graph, state.root, "builddir")
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value.to_os_str().expect("byte strings are valid on Unix")));
        if let Some(directory) = &builddir {
            std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        }
        let mut build_log = crate::log::loginit(builddir.as_deref(), &mut graph)
            .map_err(|error| error.to_string())?;
        let deps_path = builddir.as_ref().map_or_else(
            || PathBuf::from(".ninja_deps"),
            |path| path.join(".ninja_deps"),
        );
        let (mut deps_log, warning) =
            crate::deps::depsloadlog(&deps_path, &mut graph).map_err(|error| error.to_string())?;
        if let Some(warning) = warning {
            append_output(&mut output, &warning);
        }

        let manifest_edge = crate::graph::nodeget(&graph, invocation.manifest.as_bytes())
            .and_then(|node| graph.node(node).gen);
        let manifest_result = if let Some(edge) = manifest_edge {
            let streaming = build_output.is_some();
            let mut builder = if let Some(output) = build_output.as_deref_mut() {
                if let Some(diagnostics) = build_diagnostics.as_deref_mut() {
                    crate::build::Builder::with_logs_and_sinks(
                        &mut graph,
                        invocation.build_options.clone(),
                        &mut build_log,
                        &mut deps_log,
                        output,
                        diagnostics,
                    )
                } else {
                    crate::build::Builder::with_logs_and_output(
                        &mut graph,
                        invocation.build_options.clone(),
                        &mut build_log,
                        &mut deps_log,
                        output,
                    )
                }
            } else {
                crate::build::Builder::with_logs(
                    &mut graph,
                    invocation.build_options.clone(),
                    &mut build_log,
                    &mut deps_log,
                )
            };
            let result: Result<bool, String> = (|| {
                builder.add_target(invocation.manifest.as_bytes())?;
                if builder.already_up_to_date() {
                    return Ok(false);
                }
                let result = builder.build();
                let rebuilt = builder.ran_edge_without_restat_pruning(edge);
                if !streaming {
                    append_output(&mut output, &String::from_utf8_lossy(&builder.build_output));
                }
                result?;
                Ok(rebuilt)
            })();
            drop(builder);
            result
        } else {
            Ok(false)
        };
        let manifest_rebuilt = match manifest_result {
            Ok(rebuilt) => rebuilt,
            Err(error) => {
                let _ = crate::log::logclose(build_log);
                let _ = crate::deps::depsclose(deps_log);
                return Err(error);
            }
        };
        if manifest_rebuilt {
            crate::log::logclose(build_log).map_err(|error| error.to_string())?;
            crate::deps::depsclose(deps_log).map_err(|error| error.to_string())?;
            if invocation.build_options.dryrun {
                return Ok(output);
            }
            continue;
        }

        let selected_targets = if invocation.targets.is_empty() {
            default_target_paths(&parser, &graph)
        } else {
            invocation.targets.clone()
        };
        let result = {
            let streaming = build_output.is_some();
            let mut builder = if let Some(output) = build_output.as_deref_mut() {
                if let Some(diagnostics) = build_diagnostics.as_deref_mut() {
                    crate::build::Builder::with_logs_and_sinks(
                        &mut graph,
                        invocation.build_options.clone(),
                        &mut build_log,
                        &mut deps_log,
                        output,
                        diagnostics,
                    )
                } else {
                    crate::build::Builder::with_logs_and_output(
                        &mut graph,
                        invocation.build_options.clone(),
                        &mut build_log,
                        &mut deps_log,
                        output,
                    )
                }
            } else {
                crate::build::Builder::with_logs(
                    &mut graph,
                    invocation.build_options.clone(),
                    &mut build_log,
                    &mut deps_log,
                )
            };
            let result: Result<String, String> = (|| {
                for target in &selected_targets {
                    builder.add_target(target.as_bytes())?;
                }
                let result = builder.build();
                let build_output = (!streaming)
                    .then(|| String::from_utf8_lossy(&builder.build_output).into_owned());
                result?;
                Ok(build_output.unwrap_or_default())
            })();
            result
        };
        let build_log_result = crate::log::logclose(build_log).map_err(|error| error.to_string());
        let deps_log_result = crate::deps::depsclose(deps_log).map_err(|error| error.to_string());
        append_output(&mut output, &result?);
        build_log_result?;
        deps_log_result?;
        return Ok(output);
    }
    Err(format!(
        "manifest '{}' dirty after 100 tries",
        invocation.manifest
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_RUN: AtomicUsize = AtomicUsize::new(0);

    #[cfg(unix)]
    #[test]
    fn rust_cli_uses_supported_make_jobserver() {
        let mut options = BuildOptions::default();
        normalize_runtime_options(
            &mut options,
            Some("-j --jobserver-auth=fifo:/tmp/ronin-jobserver"),
        )
        .unwrap();
        assert_eq!(options.maxjobs, usize::MAX);
        assert_eq!(
            options.jobserver,
            crate::jobserver::JobserverConfig {
                mode: crate::jobserver::JobserverMode::PosixFifo,
                path: "/tmp/ronin-jobserver".into(),
            }
        );

        let mut explicit = BuildOptions {
            maxjobs: 2,
            ..BuildOptions::default()
        };
        normalize_runtime_options(&mut explicit, Some("-j --jobserver-auth=fifo:/tmp/ignored"))
            .unwrap();
        assert_eq!(explicit.maxjobs, 2);
        assert_eq!(
            explicit.jobserver,
            crate::jobserver::JobserverConfig::default()
        );
    }

    #[test]
    fn rust_cli_builds_requested_target_with_logs() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("in");
        let output = directory.join("out");
        let manifest = directory.join("build.ninja");
        fs::write(&input, "cli").unwrap();
        fs::write(
            &manifest,
            format!(
                "builddir = {}\nrule copy\n  command = cp $in $out\nbuild {}: copy {}\ndefault {}\n",
                directory.display(),
                output.display(),
                input.display(),
                output.display()
            ),
        )
        .unwrap();
        let arguments = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments).unwrap();
        assert!(status.contains("cp "));
        assert_eq!(fs::read_to_string(&output).unwrap(), "cli");
        assert!(directory.join(".ninja_log").exists());
        assert!(directory.join(".ninja_deps").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_rebuilds_and_reloads_manifest_before_targets() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-manifest-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("build.ninja");
        let template = directory.join("next.ninja");
        let output = directory.join("out");
        let render_manifest = |value: &str| {
            format!(
                "builddir = {}\nrule regen\n  command = cp $in $out\nrule emit\n  command = printf {value} > $out\nbuild {}: regen {}\nbuild {}: emit\ndefault {}\n",
                directory.display(),
                manifest.display(),
                template.display(),
                output.display(),
                output.display()
            )
        };
        fs::write(&manifest, render_manifest("old")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&template, render_manifest("new")).unwrap();

        let arguments = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments).unwrap();
        assert!(status.contains("cp "));
        assert!(status.contains("printf new"));
        assert_eq!(fs::read_to_string(&output).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(&manifest).unwrap(),
            render_manifest("new")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_continues_when_manifest_restat_prunes_rebuild() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-manifest-restat-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("build.ninja");
        let trigger = directory.join("trigger");
        let output = directory.join("out");
        fs::write(&trigger, "").unwrap();
        fs::write(
            &manifest,
            format!(
                "builddir = {}\nrule steady\n  command = true\n  restat = 1\nrule emit\n  command = printf built > $out\nbuild {}: steady {}\nbuild {}: emit\ndefault {}\n",
                directory.display(),
                manifest.display(),
                trigger.display(),
                output.display(),
                output.display()
            ),
        )
        .unwrap();

        let arguments = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments).unwrap();
        assert!(status.lines().any(|line| line.ends_with("true")));
        assert!(status.contains("printf built"));
        assert_eq!(fs::read_to_string(&output).unwrap(), "built");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_clean_rule_and_generator_options() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-clean-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("build.ninja");
        let ordinary = directory.join("ordinary");
        let generated = directory.join("generated");
        fs::write(
            &manifest,
            format!(
                "rule emit\n  command = touch $out\nrule regen\n  command = touch $out\n  generator = 1\nbuild {}: emit\nbuild {}: regen\n",
                ordinary.display(),
                generated.display()
            ),
        )
        .unwrap();
        fs::write(&ordinary, "").unwrap();
        fs::write(&generated, "").unwrap();
        let base = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            "-t".into(),
            "clean".into(),
        ];
        let mut rule_arguments = base.clone();
        rule_arguments.extend(["-r".into(), "emit".into()]);
        assert_eq!(run(&rule_arguments).unwrap(), "1");
        assert!(!ordinary.exists() && generated.exists());

        fs::write(&ordinary, "").unwrap();
        assert_eq!(run(&base).unwrap(), "1");
        assert!(!ordinary.exists() && generated.exists());

        let mut generator_arguments = base;
        generator_arguments.push("-g".into());
        assert_eq!(run(&generator_arguments).unwrap(), "1");
        assert!(!generated.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_compdb_expands_response_files_without_rule_filter() {
        let directory = std::env::temp_dir().join(format!(
            "ronin-rust-cli-compdb-{}-{}",
            std::process::id(),
            NEXT_RUN.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("build.ninja");
        let input = directory.join("in");
        let output = directory.join("out");
        fs::write(
            &manifest,
            format!(
                "rule cc\n  command = cc @$rspfile -o $out\n  rspfile = $out.rsp\n  rspfile_content = -DCLI $in\nbuild {}: cc {}\n",
                output.display(),
                input.display()
            ),
        )
        .unwrap();
        let arguments = vec![
            "ronin".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            "-t".into(),
            "compdb".into(),
            "-x".into(),
        ];
        let database = run(&arguments).unwrap();
        assert!(database.contains("-DCLI"));
        assert!(!database.contains(&format!("@{}.rsp", output.display())));
        fs::remove_dir_all(directory).unwrap();
    }
}
