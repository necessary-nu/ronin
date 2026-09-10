//! The session and context one staging pass reads through.
//!
//! Fresh for every pass, because a pass reads the whole compilation again and
//! nothing a session accumulated belongs to the next one. What DOES carry
//! between passes is handed over separately — see [`crate::make::Groundwork`].

use super::{
    GNUMAKEFLAGS, Invocation, JobCounts, MAKE_RESTARTS, MAKELEVEL, PRODUCT_NAME, RootCompilation,
    STANDARD_INPUT, carried_switches, compiler_flag_variables, decode_makefile_makeflags,
    descendant_environment, propagated_makeflags, record_invocation, switch_table,
};
use crate::make::cli::Switch;
use kati::bytes::Bytes;
use kati::flags::Flags;
use kati::session::Session;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The evaluation session one Make invocation describes.
// [spec:ronin:req:make.recursive-invocation+4]
pub(super) fn session_for(
    invocation: &Invocation,
    makefiles: &[PathBuf],
    jobs: usize,
    invoked_as: &Path,
    diagnostics: &Arc<kati::diagnostics::Diagnostics>,
    census: &Arc<kati::census::Census>,
    scripts: &Arc<kati::scripts::Scripts>,
) -> Session {
    let mut session = Session::new();
    session.scripts = Arc::clone(scripts);
    // Every session of one invocation writes what it has to say to the same
    // descriptor, which is the invocation's rather than the process's: a
    // warning raised while a Makefile is read is part of what the compilation
    // answered, and a caller that collects a run's output has to be able to see
    // it. See [`crate::make::cli::run`], which drains it.
    session.diagnostics = Arc::clone(diagnostics);
    // And into the same ledger, for the same reason: a recursive `$(MAKE)`
    // composed into this graph is classified by a session of its own, and what
    // it classified belongs to the invocation that asked.
    session.census = Arc::clone(census);
    let compiler_flags = compiler_flag_variables(invocation);
    let carried = Bytes::from(carried_switches(&compiler_flags.base, invocation).into_bytes());
    // The same table, as a makefile's own write to `MAKEFLAGS` meets it: see
    // [`interface::switch_table`], which is where the one switch that differs
    // is written down.
    let protected = Bytes::from(
        carried_switches(&switch_table(invocation, invocation.jobs).base, invocation).into_bytes(),
    );
    // The switch table alone. What `MAKEFLAGS` reads back is this plus the two
    // references it names, which the evaluator assembles: GNU Make's
    // `define_makeflags` writes the fragments and the assignments as
    // `$(-*-eval-flags-*-)` and `$(MAKEOVERRIDES)` rather than inline.
    let makeflags = Bytes::from(compiler_flags.base.into_bytes());
    let eval_flags = Bytes::from(compiler_flags.eval_flags.into_bytes());
    let has_evals = !eval_flags.is_empty();
    let make_overrides = Bytes::from(compiler_flags.overrides.into_bytes());
    session.flags = Flags {
        makefiles: makefiles
            .iter()
            .map(|makefile| makefile.as_os_str().to_owned())
            .collect(),
        num_jobs: jobs,
        num_cpus: jobs,
        // The two the compiler reads for narration: under `-s` nothing is
        // narrated and under `-n` nothing is run, so in both the command line
        // is the whole of what the build shows and a recipe's own echo stays
        // inside it rather than becoming an edge's description.
        is_silent_mode: invocation.given(Switch::Silent),
        is_dry_run: invocation.given(Switch::DryRun),
        // The three options whose whole effect is on evaluation rather than on
        // the build: what the makefile starts with, what outranks it, and
        // whether a recipe line's status is worth stopping for.
        no_builtin_rules: invocation.given(Switch::NoBuiltinRules),
        // Nothing creates `$@`'s directory before the recipe runs — see
        // `BuildOptions::create_output_directories` — so a leading
        // `mkdir -p $(@D)` is the line that makes the recipe work rather than
        // one already paid for, and the compiler must keep it.
        recipes_own_output_directories: true,
        no_builtin_variables: invocation.given(Switch::NoBuiltinVariables),
        warn_undefined_variables: invocation.given(Switch::WarnUndefinedVariables),
        environment_overrides: invocation.given(Switch::EnvironmentOverrides),
        ignore_errors: invocation.given(Switch::IgnoreErrors),
        // A fourth, and its effect on evaluation is one thing only: whether the
        // first required makefile nothing can make is the last one the update
        // considers. `complain()` chooses `error` over `fatal` on it
        // (remake.c:422), so the update walks on and refuses over every one of
        // them rather than dying inside the first.
        keep_going: invocation.given(Switch::KeepGoing),
        // A parent's assignments and this invocation's own, in that order,
        // which is the order Make applies them.
        cl_vars: invocation.variables.clone(),
        makeflags: Some(makeflags.clone()),
        eval_flags,
        make_overrides: Some(make_overrides.clone()),
        makeflags_assignment: Some(kati::flags::MakeflagsAssignment {
            decoder: decode_makefile_makeflags,
            protected,
            effective: carried,
            has_overrides: !make_overrides.is_empty(),
            published: makeflags,
            has_evals,
        }),
        // One word, and that word is a path. GNU Make answers `$(MAKE)` this
        // way and a great deal of software execs the answer rather than running
        // it through a shell — upstream's own suite adopts it as the program for
        // every later invocation. Nothing has to ride along, because the path
        // already names Make mode: that is the whole point of selecting the
        // front end by name. Switches and assignments travel in MAKEFLAGS.
        subkati_args: vec![invoked_as.as_os_str().to_owned()],
        // Compiler diagnostics retain their Makefile source, but never acquire
        // a recursive Make runner identity.
        program_name: PRODUCT_NAME.to_owned(),
        // The evaluator declares what a Makefile may assume of the interface.
        // An inherited jobserver can still bound the outer Ninja scheduler.
        // Ninja execution publishes a command edge's captured output as one
        // unit, which is target-style output synchronization even though
        // Make's `-O` selector does not install a second reporting path.
        extra_features: vec![
            "archives".to_owned(),
            "jobserver".to_owned(),
            "jobserver-fifo".to_owned(),
            "output-sync".to_owned(),
        ],
        include_dirs: invocation.include_dirs.clone(),
        // The one thing `-o` decides that the read decides rather than a scan.
        // A name whose date the invocation asserted is a name `f_mtime` never
        // runs for, so the intermediate turn-off at the end of `f_mtime` is
        // never reached for it — and an `-o` name declared intermediate and
        // lying there already is swept up, where the same file without the
        // switch is kept. Everything else the switch decides belongs to a scan,
        // and a scan is answered where the build runs.
        old_files: invocation
            .assumed_old
            .iter()
            .map(|name| Bytes::from(name.to_vec()))
            .collect(),
        // The one thing `-W` decides that the read decides rather than a scan,
        // and it is a refusal: GNU Make stamps a `-W` name by entering it, and
        // for a double-colon target `enter_file` makes a fresh entry with a
        // date and no recipe, which the update then complains over. The record
        // that makes it one is the read's to see — by the time the graph
        // exists a `::` target is actions and a join, and the name the Makefile
        // wrote is not what either is called.
        new_files: invocation
            .assumed_new
            .iter()
            .map(|name| Bytes::from(name.to_vec()))
            .collect(),
        ..Flags::default()
    };
    session.flags.targets = invocation
        .goals
        .iter()
        .map(|goal| session.intern(goal.to_vec()))
        .collect();
    session
}

/// The session and context one staging pass reads through.
///
/// Fresh for every pass, because a pass reads the whole compilation again and
/// nothing a session accumulated belongs to the next one. What DOES carry
/// between passes is handed over separately — see [`crate::make::Groundwork`].
pub(super) fn pass_session(
    root: &RootCompilation<'_>,
    restarts: usize,
    scripts: &Arc<kati::scripts::Scripts>,
) -> (Session, crate::make::CompilationContext) {
    let mut session = session_for(
        root.invocation,
        root.makefiles,
        JobCounts::of(root.options).carried,
        root.invoked_as,
        root.diagnostics,
        root.census,
        scripts,
    );
    if let Some(contents) = root.makefile_contents {
        session.supply_makefile(STANDARD_INPUT.into(), contents.to_vec());
    }
    record_invocation_variables(&mut session, root.invocation, root.level, restarts);
    let compilation = compilation_context(
        root.invocation,
        root.directory.to_owned(),
        JobCounts::of(root.options),
        root.level,
        &session,
        root.reporting,
        restarts,
    );
    (session, compilation)
}

/// What the makefile is told about the invocation reading it.
///
/// The same switches the children are told, so a Makefile that branches on
/// `$(findstring s,$(MAKEFLAGS))` is asking about this invocation and not about
/// the one that spawned it, and the depth it sits at.
// [spec:ronin:req:make.recursive-invocation+4]
pub(super) fn record_invocation_variables(
    session: &mut Session,
    invocation: &Invocation,
    level: usize,
    restarts: usize,
) {
    record_invocation(session, MAKELEVEL, level.to_string());
    // GNU Make re-executes itself to read a remade Makefile and hands the new
    // process a `MAKE_RESTARTS` count in its environment, which is why the
    // variable's origin is the environment and why the first read has no such
    // variable at all rather than a zero. Ronin reads again in place, so the
    // count is recorded here instead of survived across an exec.
    if restarts > 0 {
        record_invocation(session, MAKE_RESTARTS, restarts.to_string());
    }
    // Kati installs MAKEFLAGS as a file-origin recursive compiler variable.
    // Leaving an inherited environment binding beside it would make `-e`
    // incorrectly outrank that built-in definition.
    let environment = std::sync::Arc::make_mut(
        session
            .invocation_environment
            .get_or_insert_with(|| std::sync::Arc::new(std::env::vars_os().collect())),
    );
    environment.retain(|(candidate, _)| candidate != "MAKEFLAGS");
    // GNU Make empties `GNUMAKEFLAGS` rather than withdrawing it: `main` writes
    // `define_variable_cname (GNUMAKEFLAGS_NAME, "", o_env, 0)` the instant its
    // switches have been decoded, so a Makefile reads an empty value at the
    // environment's own rank and a child is handed the name with nothing in it.
    // Emptying and withdrawing are two different things here, and the
    // difference is what upstream's own case asks about: a Make that was given
    // no `GNUMAKEFLAGS` does not invent one for its children.
    if environment.iter().any(|(name, _)| name == GNUMAKEFLAGS) {
        record_invocation(session, GNUMAKEFLAGS, String::new());
    }
    let flags = compiler_flag_variables(invocation);
    record_invocation(session, "MFLAGS", flags.mflags);
}

/// The compiler context that a recursive recipe inherits from this unit.
pub(super) fn compilation_context(
    invocation: &Invocation,
    directory: PathBuf,
    jobs: JobCounts,
    level: usize,
    session: &Session,
    reporting: bool,
    restarts: usize,
) -> crate::make::CompilationContext {
    let mut recipe_environment = vec![(
        OsString::from(MAKELEVEL),
        Some(OsString::from(level.saturating_add(1).to_string())),
    )];
    // A recipe's environment is a delta over the process's, so an emptied
    // `GNUMAKEFLAGS` has to be said again here: the process still holds the
    // switches this invocation already folded into `MAKEFLAGS`, and a child
    // reading them a second time would apply them twice. Taken from what the
    // invocation was recorded with, so a run that was given no second stream
    // says nothing rather than inventing an empty one.
    recipe_environment.extend(
        session
            .invocation_environment
            .as_deref()
            .into_iter()
            .flatten()
            .find(|(name, _)| name == GNUMAKEFLAGS)
            .map(|(name, value)| (name.clone(), Some(value.clone()))),
    );
    crate::make::CompilationContext {
        root_directory: directory.clone(),
        directory,
        path_prefix: PathBuf::new(),
        enclosing: Arc::default(),
        diagnostics: Arc::clone(&session.diagnostics),
        interrupts: crate::make::interrupts::ReadInterrupts::installed(),
        census: Arc::clone(&session.census),
        scripts: Arc::clone(&session.scripts),
        ground: Arc::default(),
        reporting,
        makeflags: propagated_makeflags(invocation),
        always_make: invocation.given(Switch::AlwaysMake),
        restarted: restarts > 0,
        assumed_new: invocation.assumed_new.clone(),
        assumed_old: invocation.assumed_old.clone(),
        level,
        jobs: jobs.carried,
        job_group: None,
        parallel_reads: jobs.parallel_reads,
        // Everything this unit was evaluated with except how many times it has
        // been read. GNU Make marks `MAKE_RESTARTS` no-export precisely so a
        // child never sees it, and here it would do more than be visible: a
        // recursive child's compilation is identified by the environment it
        // inherits, so a count that rises with every restart would give the
        // same child a new identity each time and the work staged for it would
        // never be recognised as done.
        environment: descendant_environment(session),
        recipe_environment,
    }
}
