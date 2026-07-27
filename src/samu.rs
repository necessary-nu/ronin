//! Command-line option handling translated from `samu.c`.

use crate::build::BuildOptions;
use crate::parse::ParseOptions;
use std::path::{Path, PathBuf};

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

// [spec:samurai:def:samu.parseenvargs-fn]
// [spec:samurai:sem:samu.parseenvargs-fn]
pub fn parseenvargs(options: &mut BuildOptions, flags: Option<&str>) -> Result<(), String> {
    let Some(flags) = flags else { return Ok(()) };
    let mut arguments = flags.split(' ').filter(|argument| !argument.is_empty());
    while let Some(argument) = arguments.next() {
        match argument {
            "-j" => jobsflag(
                options,
                arguments
                    .next()
                    .ok_or_else(|| "missing -j value".to_owned())?,
            )?,
            "-l" => loadflag(
                options,
                arguments
                    .next()
                    .ok_or_else(|| "missing -l value".to_owned())?,
            )?,
            "-v" => options.verbose = true,
            _ => return Err("invalid option in SAMUFLAGS".into()),
        }
    }
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

// [spec:samurai:def:samu.main-fn]
// [spec:samurai:sem:samu.main-fn]
pub fn main(
    arguments: &[String],
    env_flags: Option<&str>,
) -> Result<(BuildOptions, ParseOptions, String), String> {
    let mut build = BuildOptions::default();
    let mut parse = ParseOptions::default();
    parseenvargs(&mut build, env_flags)?;
    let mut manifest = "build.ninja".to_owned();
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
                    usage(&progname(arguments.first().map(String::as_str), "samu"))
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

fn builder_output(builder: &crate::build::Builder<'_>) -> String {
    let mut output = builder.commands_ran.join("\n");
    if !builder.command_output.is_empty() {
        append_output(
            &mut output,
            &String::from_utf8_lossy(&builder.command_output),
        );
    }
    output
}

struct RunInvocation {
    build_options: BuildOptions,
    parse_options: ParseOptions,
    manifest: String,
    targets: Vec<String>,
    selected_tool: Option<crate::tool::Tool>,
    tool_arguments: Vec<String>,
}

enum RunAction {
    Version,
    Execute(RunInvocation),
}

fn parse_run_arguments(arguments: &[String], env_flags: Option<&str>) -> Result<RunAction, String> {
    let mut invocation = RunInvocation {
        build_options: BuildOptions::default(),
        parse_options: ParseOptions::default(),
        manifest: "build.ninja".to_owned(),
        targets: Vec::new(),
        selected_tool: None,
        tool_arguments: Vec::new(),
    };
    parseenvargs(&mut invocation.build_options, env_flags)?;
    let mut index = 1;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--version" => return Ok(RunAction::Version),
            "--verbose" | "-v" => invocation.build_options.verbose = true,
            "-C" => {
                index += 1;
                let directory = arguments
                    .get(index)
                    .ok_or_else(|| "missing -C value".to_owned())?;
                std::env::set_current_dir(directory).map_err(|error| error.to_string())?;
            }
            "-f" => {
                index += 1;
                invocation.manifest = arguments
                    .get(index)
                    .ok_or_else(|| "missing -f value".to_owned())?
                    .clone();
            }
            "-j" => {
                index += 1;
                jobsflag(
                    &mut invocation.build_options,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -j value".to_owned())?,
                )?;
            }
            "-k" => {
                index += 1;
                let value = arguments
                    .get(index)
                    .ok_or_else(|| "missing -k value".to_owned())?
                    .parse::<usize>()
                    .map_err(|_| "invalid -k parameter".to_owned())?;
                invocation.build_options.maxfail = if value == 0 { usize::MAX } else { value };
            }
            "-l" => {
                index += 1;
                loadflag(
                    &mut invocation.build_options,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -l value".to_owned())?,
                )?;
            }
            "-n" => invocation.build_options.dryrun = true,
            "-d" => {
                index += 1;
                debugflag(
                    &mut invocation.build_options,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -d value".to_owned())?,
                )?;
            }
            "-w" => {
                index += 1;
                warnflag(
                    &mut invocation.parse_options,
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -w value".to_owned())?,
                )?;
            }
            "-t" => {
                index += 1;
                invocation.selected_tool = Some(crate::tool::toolget(
                    arguments
                        .get(index)
                        .ok_or_else(|| "missing -t value".to_owned())?,
                )?);
                invocation
                    .tool_arguments
                    .extend_from_slice(&arguments[index + 1..]);
                break;
            }
            option if option.starts_with('-') => {
                return Err(format!(
                    "{}: {option}",
                    usage(&progname(arguments.first().map(String::as_str), "samu"))
                ));
            }
            target => invocation.targets.push(target.to_owned()),
        }
        index += 1;
    }
    Ok(RunAction::Execute(invocation))
}

fn normalize_runtime_options(options: &mut BuildOptions) {
    if options.maxjobs == 0 {
        options.maxjobs = match crate::os::osnproc() {
            i64::MIN..=1 => 2,
            2 => 3,
            count => (count + 2) as usize,
        };
    }
    if let Ok(status) = std::env::var("NINJA_STATUS") {
        options.statusfmt = status;
    }
}

fn default_target_names(parser: &crate::parse::Parser, graph: &crate::graph::Graph) -> Vec<String> {
    crate::parse::defaultnodes(parser, graph)
        .into_iter()
        .map(|node| {
            let node = node.borrow();
            String::from_utf8_lossy(&node.path.s[..node.path.n]).into_owned()
        })
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
            crate::env::envrule(&state.root, rule)
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

pub fn run(arguments: &[String], env_flags: Option<&str>) -> Result<String, String> {
    let RunAction::Execute(mut invocation) = parse_run_arguments(arguments, env_flags)? else {
        return Ok("1.9.0".into());
    };
    normalize_runtime_options(&mut invocation.build_options);

    let mut output = String::new();
    for _ in 0..100 {
        let mut graph = crate::graph::graphinit();
        let mut parser = crate::parse::parseinit();
        parser.options = invocation.parse_options.clone();
        let mut state = crate::env::envinit();
        crate::parse::parse(
            &invocation.manifest,
            &mut graph,
            &mut parser,
            state.root.clone(),
            &mut state,
        )?;

        if let Some(tool) = invocation.selected_tool.take() {
            return run_selected_tool(
                tool,
                &graph,
                &parser,
                &state,
                &invocation.tool_arguments,
                invocation.build_options.dryrun,
            );
        }

        let builddir = crate::env::envvar(&state.root, "builddir")
            .filter(|value| value.n != 0)
            .map(|value| PathBuf::from(String::from_utf8_lossy(&value.s[..value.n]).into_owned()));
        if let Some(directory) = &builddir {
            std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        }
        let mut build_log =
            crate::log::loginit(builddir.as_deref(), &graph).map_err(|error| error.to_string())?;
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
            .and_then(|node| node.borrow().gen.as_ref().and_then(|edge| edge.upgrade()));
        let manifest_result = if let Some(edge) = manifest_edge {
            let mut builder = crate::build::Builder::with_logs(
                &mut graph,
                invocation.build_options.clone(),
                &mut build_log,
                &mut deps_log,
            );
            let result: Result<bool, String> = (|| {
                builder.add_target(&invocation.manifest)?;
                if builder.already_up_to_date() {
                    return Ok(false);
                }
                builder.build()?;
                let rebuilt = builder.ran_edge(&edge) && !edge.borrow().restat_clean;
                append_output(&mut output, &builder_output(&builder));
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
            default_target_names(&parser, &graph)
        } else {
            invocation.targets.clone()
        };
        let result = {
            let mut builder = crate::build::Builder::with_logs(
                &mut graph,
                invocation.build_options.clone(),
                &mut build_log,
                &mut deps_log,
            );
            let result: Result<String, String> = (|| {
                for target in &selected_targets {
                    builder.add_target(target)?;
                }
                builder.build()?;
                Ok(builder_output(&builder))
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

    #[test]
    fn rust_cli_builds_requested_target_with_logs() {
        let directory = std::env::temp_dir().join(format!(
            "samurai-rust-cli-{}-{}",
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
            "samu".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments, None).unwrap();
        assert!(status.contains("cp "));
        assert_eq!(fs::read_to_string(&output).unwrap(), "cli");
        assert!(directory.join(".ninja_log").exists());
        assert!(directory.join(".ninja_deps").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_rebuilds_and_reloads_manifest_before_targets() {
        let directory = std::env::temp_dir().join(format!(
            "samurai-rust-cli-manifest-{}-{}",
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
            "samu".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments, None).unwrap();
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
            "samurai-rust-cli-manifest-restat-{}-{}",
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
            "samu".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
        ];
        let status = run(&arguments, None).unwrap();
        assert!(status.lines().any(|line| line == "true"));
        assert!(status.contains("printf built"));
        assert_eq!(fs::read_to_string(&output).unwrap(), "built");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_clean_rule_and_generator_options() {
        let directory = std::env::temp_dir().join(format!(
            "samurai-rust-cli-clean-{}-{}",
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
            "samu".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            "-t".into(),
            "clean".into(),
        ];
        let mut rule_arguments = base.clone();
        rule_arguments.extend(["-r".into(), "emit".into()]);
        assert_eq!(run(&rule_arguments, None).unwrap(), "1");
        assert!(!ordinary.exists() && generated.exists());

        fs::write(&ordinary, "").unwrap();
        assert_eq!(run(&base, None).unwrap(), "1");
        assert!(!ordinary.exists() && generated.exists());

        let mut generator_arguments = base;
        generator_arguments.push("-g".into());
        assert_eq!(run(&generator_arguments, None).unwrap(), "1");
        assert!(!generated.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rust_cli_compdb_expands_response_files_without_rule_filter() {
        let directory = std::env::temp_dir().join(format!(
            "samurai-rust-cli-compdb-{}-{}",
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
            "samu".into(),
            "-f".into(),
            manifest.to_string_lossy().into_owned(),
            "-t".into(),
            "compdb".into(),
            "-x".into(),
        ];
        let database = run(&arguments, None).unwrap();
        assert!(database.contains("-DCLI"));
        assert!(!database.contains(&format!("@{}.rsp", output.display())));
        fs::remove_dir_all(directory).unwrap();
    }
}
